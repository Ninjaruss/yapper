//! The STT worker thread: drains tee'd mono audio, resamples to 16 kHz,
//! chunks it into utterances (energy VAD), transcribes each with the engine,
//! and persists + publishes the resulting segments.
//!
//! Engine and store errors are logged and flagged via `stt_failed` but never
//! kill the thread — the mirror must keep listening even if transcription is
//! broken (recording never depends on STT succeeding).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};

use crate::store::SessionStore;
use crate::stt::resample::Resampler;
use crate::stt::vad::UtteranceChunker;
use crate::stt::{Segment, TranscribeEngine};

/// Spawn the STT worker thread. `rx` is the tee of mono audio at
/// `input_rate` fed by the capture callback; the thread exits once `rx`
/// disconnects (after flushing any trailing audio).
pub fn spawn_stt_worker(
    mut engine: Box<dyn TranscribeEngine>,
    input_rate: u32,
    rx: Receiver<Vec<f32>>,
    store: Arc<SessionStore>,
    session_id: i64,
    seg_tx: Sender<Segment>,
    stt_failed: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // If the resampler itself fails to construct, there is nothing this
        // thread can usefully do; flag it and drain the channel so the tee
        // sender (the audio callback) never blocks on a full bounded queue.
        let mut resampler = match Resampler::new(input_rate) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("stt worker: resampler init failed: {e}");
                stt_failed.store(true, Ordering::Relaxed);
                for _ in rx.iter() {}
                return;
            }
        };
        let mut vad = UtteranceChunker::new();

        let handle_utterance =
            |engine: &mut Box<dyn TranscribeEngine>,
             store: &Arc<SessionStore>,
             seg_tx: &Sender<Segment>,
             stt_failed: &Arc<AtomicBool>,
             utterance: crate::stt::vad::Utterance| {
                let text = match engine.transcribe(&utterance.samples) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("stt worker: transcribe failed: {e}");
                        stt_failed.store(true, Ordering::Relaxed);
                        return;
                    }
                };
                if text.is_empty() {
                    return;
                }
                // end_ms includes up to ~600ms trailing silence padding
                // (the VAD's END_SILENCE_MS); acceptable for glanceable UI,
                // revisit for edit markers in Plan 5.
                let end_ms =
                    utterance.start_ms + (utterance.samples.len() * 1000 / 16_000) as i64;
                match store.add_segment(session_id, utterance.start_ms, end_ms, &text) {
                    Ok(_) => {
                        let _ = seg_tx.send(Segment {
                            start_ms: utterance.start_ms,
                            end_ms,
                            text,
                        });
                    }
                    Err(e) => {
                        eprintln!("stt worker: add_segment failed: {e}");
                        stt_failed.store(true, Ordering::Relaxed);
                    }
                }
            };

        for chunk in rx.iter() {
            let sixteen_k = resampler.process(&chunk);
            for utterance in vad.push(&sixteen_k) {
                handle_utterance(&mut engine, &store, &seg_tx, &stt_failed, utterance);
            }
        }

        // End of stream: flush the resampler's pending tail through the
        // VAD, then flush whatever utterance the VAD had in flight.
        let tail = resampler.flush();
        for utterance in vad.push(&tail) {
            handle_utterance(&mut engine, &store, &seg_tx, &stt_failed, utterance);
        }
        if let Some(utterance) = vad.flush() {
            handle_utterance(&mut engine, &store, &seg_tx, &stt_failed, utterance);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::MockEngine;

    #[test]
    fn worker_turns_audio_into_stored_segments() {
        let store = Arc::new(crate::store::SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(256);
        let (seg_tx, seg_rx) = crossbeam_channel::unbounded();
        let engine = Box::new(MockEngine::new(vec!["first utterance".into()]));
        let stt_failed = Arc::new(AtomicBool::new(false));
        let handle = spawn_stt_worker(engine, 16_000, rx, store.clone(), sid, seg_tx, stt_failed.clone());

        tx.send(vec![0.3; 16_000]).unwrap(); // 1s speech
        tx.send(vec![0.0; 16_000]).unwrap(); // 1s silence -> utterance closes
        drop(tx);
        handle.join().unwrap();

        let segs = store.list_segments(sid).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "first utterance");
        assert!(
            seg_rx.try_recv().is_ok(),
            "segment must also be published for the UI"
        );
        assert!(!stt_failed.load(Ordering::Relaxed));
    }

    #[test]
    fn worker_flushes_trailing_utterance_with_no_closing_silence() {
        // No trailing silence at all: the utterance only surfaces via the
        // end-of-stream flush path (vad.flush()), which is the behavior the
        // carry-forward review requires.
        let store = Arc::new(crate::store::SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(256);
        let (seg_tx, seg_rx) = crossbeam_channel::unbounded();
        let engine = Box::new(MockEngine::new(vec!["tail utterance".into()]));
        let stt_failed = Arc::new(AtomicBool::new(false));
        let handle = spawn_stt_worker(engine, 48_000, rx, store.clone(), sid, seg_tx, stt_failed.clone());

        // 700ms of speech at 48k, never followed by silence.
        tx.send(vec![0.3; 48_000 * 700 / 1000]).unwrap();
        drop(tx);
        handle.join().unwrap();

        let segs = store.list_segments(sid).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "tail utterance");
        assert!(seg_rx.try_recv().is_ok());
        assert!(!stt_failed.load(Ordering::Relaxed));
    }

    #[test]
    fn engine_error_sets_flag_and_keeps_draining() {
        struct FailingEngine;
        impl TranscribeEngine for FailingEngine {
            fn transcribe(&mut self, _samples_16k: &[f32]) -> Result<String, crate::error::YapperError> {
                Err(crate::error::YapperError::Audio("boom".into()))
            }
        }

        let store = Arc::new(crate::store::SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(256);
        let (seg_tx, seg_rx) = crossbeam_channel::unbounded();
        let engine = Box::new(FailingEngine);
        let stt_failed = Arc::new(AtomicBool::new(false));
        let handle = spawn_stt_worker(engine, 16_000, rx, store.clone(), sid, seg_tx, stt_failed.clone());

        tx.send(vec![0.3; 16_000]).unwrap();
        tx.send(vec![0.0; 16_000]).unwrap();
        drop(tx);
        handle.join().unwrap();

        assert!(stt_failed.load(Ordering::Relaxed));
        assert!(store.list_segments(sid).unwrap().is_empty());
        assert!(seg_rx.try_recv().is_err());
    }
}
