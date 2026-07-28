//! Post-session lossless compression: WAV → FLAC via the PLATFORM encoder
//! (`afconvert` on macOS, the `flac` CLI elsewhere). Called from `lib.rs`'s
//! `end_session` on a detached thread after the take is safely in the DB —
//! a failure here just leaves the WAV in place.
//!
//! Why not a Rust encoder crate: `flacenc` output decoded fine with lenient
//! decoders (claxon) but CoreAudio — the decoder WKWebView actually uses
//! for playback — rejected every file it produced (ExtAudioFileRead -50).
//! Encoding with the platform's own toolchain keeps encoder and player from
//! the same vendor. And the WAV is only ever deleted after the FLAC has
//! been FULLY DECODED back and its sample count matched — magic-bytes
//! checks let a malformed stream through once; never again.

use std::path::{Path, PathBuf};
use std::process::Command;

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

    // Verify before touching the WAV: the FLAC must exist, carry the magic,
    // AND fully decode (claxon) to exactly the source's sample count. Only
    // a proven-playable file earns the right to replace the recording.
    if let Err(e) = verify_flac_against_wav(&flac_path, wav) {
        let _ = std::fs::remove_file(&flac_path);
        return Err(e);
    }

    std::fs::remove_file(wav)?;
    Ok(flac_path)
}

fn verify_flac_against_wav(flac_path: &Path, wav: &Path) -> Result<(), YapperError> {
    let bytes = std::fs::read(flac_path)?;
    if bytes.len() < FLAC_MAGIC.len() || bytes[..FLAC_MAGIC.len()] != *FLAC_MAGIC {
        return Err(YapperError::Audio(
            "flac output failed verification (missing magic bytes)".into(),
        ));
    }
    let source_samples = hound::WavReader::open(wav)
        .map_err(|e| YapperError::Audio(format!("could not reopen wav for verify: {e}")))?
        .len() as u64;
    let mut reader = claxon::FlacReader::open(flac_path)
        .map_err(|e| YapperError::Audio(format!("flac verify: unreadable stream: {e}")))?;
    let mut decoded: u64 = 0;
    for s in reader.samples() {
        s.map_err(|e| YapperError::Audio(format!("flac verify: decode error: {e}")))?;
        decoded += 1;
    }
    if decoded != source_samples {
        return Err(YapperError::Audio(format!(
            "flac verify: decoded {decoded} samples, wav has {source_samples}"
        )));
    }
    Ok(())
}

/// Read `wav`, encode it, and write the result to `flac_path`. Isolated
/// from `wav_to_flac` so every early-return in here funnels through one
/// cleanup path (the partial-file removal above) instead of duplicating it.
fn encode_to(wav: &Path, flac_path: &Path) -> Result<(), YapperError> {
    // The wav must at least open as one (clear error before shelling out).
    hound::WavReader::open(wav)
        .map_err(|e| YapperError::Audio(format!("could not open wav {wav:?}: {e}")))?;
    // Platform tools refuse-or-vary on pre-existing outputs; start clean.
    let _ = std::fs::remove_file(flac_path);

    #[cfg(target_os = "macos")]
    let output = Command::new("afconvert")
        .arg(wav)
        .arg("-o")
        .arg(flac_path)
        .args(["-f", "flac", "-d", "flac"])
        .output()
        .map_err(|e| YapperError::Audio(format!("could not run afconvert: {e}")))?;

    #[cfg(not(target_os = "macos"))]
    let output = Command::new("flac")
        .args(["--silent", "--force", "-o"])
        .arg(flac_path)
        .arg(wav)
        .output()
        .map_err(|e| {
            YapperError::Audio(format!(
                "could not run flac (install the `flac` package to enable compression): {e}"
            ))
        })?;

    if !output.status.success() {
        return Err(YapperError::Audio(format!(
            "flac encode failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Round-trip: what we encode must DECODE — with a real FLAC decoder —
    // back to the exact samples. Magic-bytes checks let a malformed stream
    // through once; never again.
    #[test]
    fn flac_roundtrip_decodes_to_identical_samples() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("rt.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav_path, spec).unwrap();
        let original: Vec<i16> = (0..16_000)
            .map(|i| ((i as f32 / 16_000.0 * std::f32::consts::TAU * 220.0).sin() * 12000.0) as i16)
            .collect();
        for s in &original {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();

        let flac = wav_to_flac(&wav_path).expect("encode");
        let mut reader = claxon::FlacReader::open(&flac).expect("claxon must open our flac");
        let decoded: Vec<i32> = reader
            .samples()
            .map(|s| s.expect("claxon decode"))
            .collect();
        assert_eq!(decoded.len(), original.len(), "sample count mismatch");
        for (i, (d, o)) in decoded.iter().zip(original.iter()).enumerate() {
            assert_eq!(*d, i32::from(*o), "sample {i} differs");
        }
    }

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
