//! Mic capture: cpal input stream → mono f32 buffers → WAV writer thread.
//! The stream stays open while paused (device warm); paused buffers are
//! dropped before they reach the channel, so the WAV contains speech time only.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender};

use crate::error::YapperError;

/// Average interleaved frames down to mono; return None while paused.
pub fn gate_and_downmix(
    interleaved: &[f32],
    channels: usize,
    paused: &Arc<AtomicBool>,
) -> Option<Vec<f32>> {
    if paused.load(Ordering::Relaxed) {
        return None;
    }
    Some(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect(),
    )
}

/// Writer thread: drains mono buffers into a 16-bit WAV, emitting an RMS
/// level per drained buffer. Returns when the sending side closes.
pub fn spawn_writer(
    path: PathBuf,
    sample_rate: u32,
    rx: Receiver<Vec<f32>>,
    level_tx: Sender<f32>,
) -> JoinHandle<Result<(), YapperError>> {
    std::thread::spawn(move || {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec)
            .map_err(|e| YapperError::Audio(e.to_string()))?;
        for buffer in rx.iter() {
            let _ = level_tx.send(super::rms_level(&buffer));
            for s in &buffer {
                let clamped = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                writer
                    .write_sample(clamped)
                    .map_err(|e| YapperError::Audio(e.to_string()))?;
            }
            // Keep headers/data recoverable if we die mid-session.
            writer.flush().map_err(|e| YapperError::Audio(e.to_string()))?;
        }
        writer
            .finalize()
            .map_err(|e| YapperError::Audio(e.to_string()))?;
        Ok(())
    })
}

/// A running capture. Dropping `_stream` stops the device; closing `buffer_tx`
/// (by dropping this struct) lets the writer finalize the WAV.
pub struct Capture {
    pub paused: Arc<AtomicBool>,
    pub level_rx: Receiver<f32>,
    pub wav_path: PathBuf,
    buffer_tx: Option<Sender<Vec<f32>>>,
    writer: Option<JoinHandle<Result<(), YapperError>>>,
    _stream: cpal::Stream,
}

impl Capture {
    /// Start capturing from `device_name` (or the default input) into `wav_path`.
    pub fn start(device_name: Option<&str>, wav_path: PathBuf) -> Result<Self, YapperError> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(wanted) => host
                .input_devices()
                .map_err(|e| YapperError::Audio(e.to_string()))?
                .find(|d| d.name().map(|n| n == wanted).unwrap_or(false))
                .ok_or_else(|| YapperError::Audio(format!("input device '{wanted}' not found")))?,
            None => host
                .default_input_device()
                .ok_or_else(|| YapperError::Audio("no default input device".into()))?,
        };
        let config = device
            .default_input_config()
            .map_err(|e| YapperError::Audio(e.to_string()))?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let paused = Arc::new(AtomicBool::new(false));
        let (buffer_tx, buffer_rx) = crossbeam_channel::unbounded::<Vec<f32>>();
        let (level_tx, level_rx) = crossbeam_channel::unbounded::<f32>();
        let writer = spawn_writer(wav_path.clone(), sample_rate, buffer_rx, level_tx);

        let cb_paused = paused.clone();
        let cb_tx = buffer_tx.clone();
        let stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    if let Some(mono) = gate_and_downmix(data, channels, &cb_paused) {
                        let _ = cb_tx.send(mono);
                    }
                },
                |err| eprintln!("audio stream error: {err}"),
                None,
            )
            .map_err(|e| YapperError::Audio(e.to_string()))?;
        stream.play().map_err(|e| YapperError::Audio(e.to_string()))?;

        Ok(Self {
            paused,
            level_rx,
            wav_path,
            buffer_tx: Some(buffer_tx),
            writer: Some(writer),
            _stream: stream,
        })
    }

    /// Stop the stream, close the channel, wait for the WAV to finalize.
    pub fn stop(mut self) -> Result<PathBuf, YapperError> {
        drop(self.buffer_tx.take()); // close channel → writer finalizes
        if let Some(writer) = self.writer.take() {
            writer
                .join()
                .map_err(|_| YapperError::Audio("writer thread panicked".into()))??;
        }
        Ok(self.wav_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn writer_writes_wav_and_reports_levels() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("take.wav");
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<f32>>();
        let (level_tx, level_rx) = crossbeam_channel::unbounded::<f32>();

        let handle = spawn_writer(wav_path.clone(), 48_000, rx, level_tx);

        // 48k samples = 1 second of audio at half scale
        for _ in 0..100 {
            tx.send(vec![0.5; 480]).unwrap();
        }
        drop(tx); // closes channel; writer finalizes
        handle.join().unwrap().unwrap();

        let reader = hound::WavReader::open(&wav_path).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.len(), 48_000);

        let levels: Vec<f32> = level_rx.try_iter().collect();
        assert!(!levels.is_empty());
        assert!(levels.iter().all(|l| (*l - 0.5).abs() < 0.05));
    }

    #[test]
    fn paused_flag_drops_buffers() {
        let paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        // The downmix+gate helper is what the cpal callback uses.
        let out = gate_and_downmix(&[0.5, 0.5, 0.5, 0.5], 2, &paused);
        assert!(out.is_none());
        paused.store(false, Ordering::Relaxed);
        let out = gate_and_downmix(&[0.5, 0.3, 0.5, 0.3], 2, &paused).unwrap();
        assert_eq!(out, vec![0.4, 0.4]); // stereo averaged to mono
    }
}
