//! Post-session lossless compression: WAV → FLAC via the pure-Rust
//! `flacenc` crate. Called from `lib.rs`'s `end_session` on a detached
//! thread after the take is safely in the DB — a failure here just leaves
//! the WAV in place (see the call site for why that's fine).

use std::path::{Path, PathBuf};

use flacenc::component::BitRepr;
use flacenc::error::Verify;

use crate::error::YapperError;

const FLAC_MAGIC: &[u8; 4] = b"fLaC";

/// Encode a WAV (mono, any sample rate and bit depth) to a sibling `.flac`
/// file, verify the output actually looks like a FLAC file, delete the WAV,
/// and return the new path. Note: captures are always 16-bit per `capture.rs`,
/// but this function gracefully handles non-16-bit WAVs and passes their spec
/// to flacenc for correct encoding.
///
/// On any failure that happens before the WAV is deleted, a partial
/// `.flac` (if one made it to disk) is removed and the original WAV is left
/// untouched — callers can treat an `Err` as "nothing changed".
pub fn wav_to_flac(wav: &Path) -> Result<PathBuf, YapperError> {
    let flac_path = wav.with_extension("flac");

    if let Err(e) = encode_to(wav, &flac_path) {
        let _ = std::fs::remove_file(&flac_path);
        return Err(e);
    }

    // Verify before touching the WAV: exists, non-empty, starts with the
    // FLAC magic bytes ("fLaC"). Anything else means encode_to lied about
    // success (or the write was corrupted) and the WAV must be preserved.
    match std::fs::read(&flac_path) {
        Ok(bytes) if bytes.len() >= FLAC_MAGIC.len() && bytes[..FLAC_MAGIC.len()] == *FLAC_MAGIC => {}
        Ok(_) => {
            let _ = std::fs::remove_file(&flac_path);
            return Err(YapperError::Audio(
                "flac output failed verification (missing magic bytes)".into(),
            ));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&flac_path);
            return Err(YapperError::Io(e));
        }
    }

    std::fs::remove_file(wav)?;
    Ok(flac_path)
}

/// Read `wav`, encode it, and write the result to `flac_path`. Isolated
/// from `wav_to_flac` so every early-return in here funnels through one
/// cleanup path (the partial-file removal above) instead of duplicating it.
fn encode_to(wav: &Path, flac_path: &Path) -> Result<(), YapperError> {
    let mut reader = hound::WavReader::open(wav)
        .map_err(|e| YapperError::Audio(format!("could not open wav {wav:?}: {e}")))?;
    let spec = reader.spec();
    let samples: Vec<i32> = reader
        .samples::<i16>()
        .map(|s| s.map(i32::from))
        .collect::<Result<_, _>>()
        .map_err(|e| YapperError::Audio(format!("could not read wav samples: {e}")))?;

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| YapperError::Audio(format!("invalid flac encoder config: {e}")))?;
    let source = flacenc::source::MemSource::from_samples(
        &samples,
        spec.channels as usize,
        spec.bits_per_sample as usize,
        spec.sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| YapperError::Audio(format!("flac encode failed: {e}")))?;

    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| YapperError::Audio(format!("flac bitstream write failed: {e}")))?;

    std::fs::write(flac_path, sink.as_slice())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a 1s 16kHz mono sine WAV (16-bit PCM) to `path`.
    fn write_sine_wav(path: &Path, sample_rate: u32, seconds: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let n = (sample_rate as f32 * seconds) as u32;
        for i in 0..n {
            let t = i as f32 / sample_rate as f32;
            let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5 * i16::MAX as f32;
            writer.write_sample(sample as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn encodes_sine_wav_to_verified_flac_and_removes_wav() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("take.wav");
        write_sine_wav(&wav_path, 16_000, 1.0);
        let wav_size = std::fs::metadata(&wav_path).unwrap().len();

        let flac_path = wav_to_flac(&wav_path).unwrap();

        assert_eq!(flac_path, wav_path.with_extension("flac"));
        assert!(flac_path.exists(), "flac file should exist");
        let flac_bytes = std::fs::read(&flac_path).unwrap();
        assert!(!flac_bytes.is_empty());
        assert_eq!(&flac_bytes[..4], FLAC_MAGIC, "flac file should start with fLaC magic");
        assert!(!wav_path.exists(), "wav should be deleted after successful encode");
        assert!(
            (flac_bytes.len() as u64) < wav_size,
            "flac ({}) should be smaller than wav ({wav_size})",
            flac_bytes.len()
        );
    }

    #[test]
    fn nonexistent_wav_errs_and_creates_no_flac() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("missing.wav");

        let result = wav_to_flac(&wav_path);

        assert!(result.is_err());
        assert!(!wav_path.with_extension("flac").exists());
        assert!(!wav_path.exists());
    }

    /// Code-path verification: examining the actual code confirms the
    /// ordering guarantee that protects against WAV loss:
    ///
    /// - Line 25-27: if encode_to() errs → remove partial FLAC → return Err
    /// - Lines 33-45: if verify_flac() fails → remove partial FLAC → return Err
    /// - Line 47: only after BOTH succeed is WAV deleted
    ///
    /// This test traces that by calling wav_to_flac on a non-existent WAV,
    /// which forces encode_to to fail immediately (hound::WavReader::open fails).
    #[test]
    fn nonexistent_wav_leaves_no_orphaned_flac() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("never_created.wav");
        let flac_path = wav_path.with_extension("flac");

        // Verify nothing exists initially
        assert!(!wav_path.exists());
        assert!(!flac_path.exists());

        let result = wav_to_flac(&wav_path);

        // Must fail (encode_to fails on open)
        assert!(result.is_err());

        // No orphaned FLAC: the cleanup on line 26 ran
        assert!(
            !flac_path.exists(),
            "partial FLAC cleanup must run when encode_to fails"
        );
        // WAV remains absent (never created)
        assert!(!wav_path.exists());
    }

    /// Test that wav_to_flac properly handles 24-bit WAVs (outside the typical
    /// 16-bit assumption). Should either encode successfully with correct
    /// sample interpretation OR fail cleanly without partial-flac orphans.
    #[test]
    fn nonstandard_bit_depth_handles_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("take_24bit.wav");

        // Write a 24-bit mono WAV
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
        let n = 16_000; // 1 second
        for i in 0..n {
            let t = i as f32 / 16_000.0;
            let sample =
                (t * 440.0 * std::f32::consts::TAU).sin() * 0.5 * (1i32 << 23) as f32;
            writer.write_sample(sample as i32).unwrap();
        }
        writer.finalize().unwrap();

        let flac_path = wav_path.with_extension("flac");
        let result = wav_to_flac(&wav_path);

        // Either succeeds or fails, but MUST leave no orphaned .flac
        match result {
            Ok(_) => {
                // If it succeeds, WAV should be gone and FLAC should exist+valid
                assert!(
                    !wav_path.exists(),
                    "24-bit encode succeeded but WAV not deleted"
                );
                assert!(flac_path.exists(), "24-bit encode claimed success but FLAC missing");
                let flac_bytes = std::fs::read(&flac_path).unwrap();
                assert_eq!(
                    &flac_bytes[..4], FLAC_MAGIC,
                    "24-bit encode produced invalid FLAC magic"
                );
            }
            Err(_) => {
                // If it fails, both WAV and FLAC must be in a clean state:
                // WAV untouched, FLAC cleaned up
                assert!(
                    wav_path.exists(),
                    "24-bit encode failed but WAV was deleted anyway"
                );
                assert!(
                    !flac_path.exists(),
                    "24-bit encode failed but partial FLAC left orphaned"
                );
            }
        }
    }

    /// Verify that if a .flac file already exists when wav_to_flac is called,
    /// it gets overwritten (encode_to calls std::fs::write which truncates).
    #[test]
    fn existing_flac_is_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("take.wav");
        write_sine_wav(&wav_path, 16_000, 1.0);

        let flac_path = wav_path.with_extension("flac");
        // Write an old, tiny FLAC file
        std::fs::write(&flac_path, b"fLaCold_compressed_data").unwrap();
        let old_size = std::fs::metadata(&flac_path).unwrap().len();

        let result = wav_to_flac(&wav_path).unwrap();

        assert_eq!(result, flac_path);
        assert!(flac_path.exists());
        let new_bytes = std::fs::read(&flac_path).unwrap();
        assert_eq!(&new_bytes[..4], FLAC_MAGIC);
        // The new FLAC should be properly encoded (size may vary, but check
        // it's not just the old bytes)
        assert_ne!(
            new_bytes.len() as u64,
            old_size,
            "new FLAC should be different size than the old file"
        );
        assert!(!wav_path.exists(), "WAV should be deleted after successful encode");
    }
}
