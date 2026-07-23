//! Mic capture: cpal input stream → mono f32 buffers → WAV writer thread.
//! The stream stays open while paused (device warm); paused buffers are
//! dropped before they reach the channel, so the WAV contains speech time only.
//!
//! The cpal `Stream` is not `Send` on every platform, so it is never stored
//! directly on `Capture` (which needs to live inside `Mutex<...>` app state).
//! Instead it's owned entirely by a dedicated "stream thread" that starts the
//! device, reports success/failure once over a one-shot channel, then blocks
//! until told to stop. `Capture` only holds `Send` handles: join handles and
//! channel endpoints.

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
///
/// `level_tx` is expected to be a bounded channel; levels are best-effort
/// (`try_send`) so a UI that isn't draining the meter can never make this
/// thread block or grow memory. On any WAV I/O error, `writer_failed` is
/// set before the error is returned, so a caller polling that flag (rather
/// than joining the thread) can notice mid-session failures like disk full.
pub fn spawn_writer(
    path: PathBuf,
    sample_rate: u32,
    rx: Receiver<Vec<f32>>,
    level_tx: Sender<f32>,
    writer_failed: Arc<AtomicBool>,
) -> JoinHandle<Result<(), YapperError>> {
    std::thread::spawn(move || {
        let result = (|| -> Result<(), YapperError> {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&path, spec)
                .map_err(|e| YapperError::Audio(e.to_string()))?;
            for buffer in rx.iter() {
                // Empty buffer = shutdown sentinel from `Capture::stop()`.
                // The writer must NOT rely on channel disconnection alone:
                // macOS CoreAudio can leave a zombie input callback (and its
                // Sender clone) alive after the stream is dropped, which
                // would keep this loop parked forever.
                if buffer.is_empty() {
                    break;
                }
                let _ = level_tx.try_send(super::rms_level(&buffer));
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
        })();
        if result.is_err() {
            writer_failed.store(true, Ordering::Relaxed);
        }
        result
    })
}

/// A running capture. The cpal stream itself lives on `stream_thread`, not
/// on this struct, so `Capture` is `Send` and can sit inside `Mutex` app
/// state. See `stop()` for the shutdown ordering invariant.
pub struct Capture {
    pub paused: Arc<AtomicBool>,
    pub level_rx: Receiver<f32>,
    pub wav_path: PathBuf,
    /// Native sample rate of the input device, as reported by cpal at
    /// stream setup. The STT worker needs this to configure its resampler.
    pub sample_rate: u32,
    /// Set by the writer thread if a WAV I/O error occurs mid-session
    /// (e.g. disk full). The UI can poll this instead of joining.
    pub writer_failed: Arc<AtomicBool>,
    buffer_tx: Option<Sender<Vec<f32>>>,
    writer: Option<JoinHandle<Result<(), YapperError>>>,
    stop_tx: Option<Sender<()>>,
    stream_thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Start capturing from `device_name` (or the default input) into `wav_path`.
    ///
    /// `tee_tx`, if provided, receives a clone of every gated mono buffer in
    /// addition to the WAV writer — this feeds the STT worker. It must be a
    /// *bounded* channel: the callback uses `try_send`, so a worker that
    /// falls behind just drops audio for STT (never for the WAV, and never
    /// blocking the audio callback itself).
    pub fn start(
        device_name: Option<&str>,
        wav_path: PathBuf,
        tee_tx: Option<Sender<Vec<f32>>>,
    ) -> Result<Self, YapperError> {
        let paused = Arc::new(AtomicBool::new(false));
        let writer_failed = Arc::new(AtomicBool::new(false));
        let (buffer_tx, buffer_rx) = crossbeam_channel::unbounded::<Vec<f32>>();
        // Bounded so an undrained level meter can never grow memory; the
        // writer thread uses try_send, so a full channel just drops levels.
        let (level_tx, level_rx) = crossbeam_channel::bounded::<f32>(8);
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<u32, YapperError>>(1);
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);

        let device_name_owned = device_name.map(|s| s.to_string());
        let cb_tx = buffer_tx.clone();
        let cb_paused = paused.clone();
        let cb_tee = tee_tx.clone();

        // Owns the cpal Device/Stream for its entire lifetime; both are
        // dropped when this thread returns (after stop_rx signals/disconnects).
        let stream_thread = std::thread::spawn(move || {
            let setup = (|| -> Result<(cpal::Device, cpal::StreamConfig, u32, usize), YapperError> {
                let host = cpal::default_host();
                let device = match device_name_owned.as_deref() {
                    Some(wanted) => host
                        .input_devices()
                        .map_err(|e| YapperError::Audio(e.to_string()))?
                        .find(|d| d.name().map(|n| n == wanted).unwrap_or(false))
                        .ok_or_else(|| {
                            YapperError::Audio(format!("input device '{wanted}' not found"))
                        })?,
                    None => host
                        .default_input_device()
                        .ok_or_else(|| YapperError::Audio("no default input device".into()))?,
                };
                let config = device
                    .default_input_config()
                    .map_err(|e| YapperError::Audio(e.to_string()))?;
                let sample_rate = config.sample_rate().0;
                let channels = config.channels() as usize;
                Ok((device, config.into(), sample_rate, channels))
            })();

            let (device, stream_config, sample_rate, channels) = match setup {
                Ok(v) => v,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            let stream = match device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    if let Some(mono) = gate_and_downmix(data, channels, &cb_paused) {
                        if let Some(t) = &cb_tee {
                            let _ = t.try_send(mono.clone());
                        }
                        let _ = cb_tx.send(mono);
                    }
                },
                |err| eprintln!("audio stream error: {err}"),
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(YapperError::Audio(e.to_string())));
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(YapperError::Audio(e.to_string())));
                return;
            }

            let _ = ready_tx.send(Ok(sample_rate));

            // Block here, keeping the stream alive, until told to shut down.
            // Either an explicit signal or the sender disconnecting (stop_tx
            // dropped) unblocks this recv.
            let _ = stop_rx.recv();
            // Stop explicitly rather than relying on Drop: CoreAudio teardown
            // can silently fail (observed when the main thread is busy),
            // leaving a zombie callback. An explicit pause is better-defined,
            // and any error is at least visible in dev logs.
            if let Err(e) = stream.pause() {
                eprintln!("audio stream stop error: {e}");
            }
            // `stream` (and `device`) drop here, releasing the hardware.
        });

        let sample_rate = match ready_rx.recv() {
            Ok(Ok(sample_rate)) => sample_rate,
            Ok(Err(e)) => {
                let _ = stream_thread.join();
                return Err(e);
            }
            Err(_) => {
                let _ = stream_thread.join();
                return Err(YapperError::Audio(
                    "capture stream thread failed to start".into(),
                ));
            }
        };

        let writer = spawn_writer(
            wav_path.clone(),
            sample_rate,
            buffer_rx,
            level_tx,
            writer_failed.clone(),
        );

        Ok(Self {
            paused,
            level_rx,
            wav_path,
            sample_rate,
            writer_failed,
            buffer_tx: Some(buffer_tx),
            writer: Some(writer),
            stop_tx: Some(stop_tx),
            stream_thread: Some(stream_thread),
        })
    }

    /// Stop the stream, close the channel, wait for the WAV to finalize.
    ///
    /// Shutdown is deliberately redundant, because macOS CoreAudio teardown
    /// is not trustworthy (observed: a "zombie" input callback that outlives
    /// the dropped stream and keeps its Sender clone alive forever):
    /// 1. `paused` is set so even a zombie callback stops producing audio;
    /// 2. the stream thread is signaled, pauses the stream explicitly, and
    ///    is joined;
    /// 3. the writer is told to finish via an in-band empty-buffer sentinel —
    ///    it must never depend on every Sender clone being dropped;
    /// 4. only then is the writer joined.
    ///
    /// MUST NOT be called on the main thread: step 2's teardown can require
    /// the main run loop to be live (see async commands in lib.rs).
    pub fn stop(mut self) -> Result<PathBuf, YapperError> {
        self.paused.store(true, Ordering::Relaxed);
        self.stop_tx.take(); // drop → disconnects → stream thread's recv() returns
        if let Some(stream_thread) = self.stream_thread.take() {
            stream_thread
                .join()
                .map_err(|_| YapperError::Audio("stream thread panicked".into()))?;
        }
        if let Some(buffer_tx) = self.buffer_tx.take() {
            let _ = buffer_tx.send(Vec::new()); // sentinel: writer finishes even if a zombie sender survives
        }
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
        let (level_tx, level_rx) = crossbeam_channel::bounded::<f32>(8);
        let writer_failed = Arc::new(AtomicBool::new(false));

        let handle = spawn_writer(wav_path.clone(), 48_000, rx, level_tx, writer_failed.clone());

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

        // level channel is bounded(8); with nobody draining concurrently,
        // later try_sends may be dropped, so just check what did land.
        let levels: Vec<f32> = level_rx.try_iter().collect();
        assert!(!levels.is_empty());
        assert!(levels.iter().all(|l| (*l - 0.5).abs() < 0.05));
        assert!(!writer_failed.load(Ordering::Relaxed));
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

    // Regression test for the End-the-talk hang: on macOS, CoreAudio can keep
    // the input callback (and its Sender clone) alive after the stream is
    // dropped, so the writer must not depend on channel disconnection alone.
    #[test]
    fn writer_exits_on_sentinel_even_with_live_sender() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("take.wav");
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<f32>>();
        let (level_tx, _level_rx) = crossbeam_channel::bounded::<f32>(8);
        let writer_failed = Arc::new(AtomicBool::new(false));
        let handle = spawn_writer(wav_path.clone(), 48_000, rx, level_tx, writer_failed);

        tx.send(vec![0.5; 480]).unwrap();
        tx.send(Vec::new()).unwrap(); // shutdown sentinel; tx deliberately stays alive

        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);
        std::thread::spawn(move || {
            let _ = handle.join().unwrap();
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("writer did not exit on sentinel while a sender was still alive");

        let reader = hound::WavReader::open(&wav_path).unwrap();
        assert_eq!(reader.len(), 480);
    }

    #[test]
    fn capture_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Capture>();
    }

    // Extend the existing helper contract: `Capture::start` takes an
    // optional tee Sender. The pure test just re-checks gate behavior is
    // unchanged; the tee itself is wiring (worker test covers flow).
    #[test]
    fn gate_and_downmix_feeds_tee_when_provided() {
        let paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(gate_and_downmix(&[0.1, 0.1], 1, &paused).is_some());
    }
}
