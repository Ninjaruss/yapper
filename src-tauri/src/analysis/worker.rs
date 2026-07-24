//! Analysis worker: segments in → signals out.
//!
//! Thread-spawned analysis loop consuming STT segments, feeding them through
//! RhythmTracker and RepetitionDetector, persisting signals to events table,
//! and forwarding to UI. Accumulates session totals and computes baseline on shutdown.

use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};

use super::repetition::RepetitionDetector;
use super::rhythm::RhythmTracker;
use super::text::{count_fillers, word_count};
use super::{Signal, SignalKind};
use crate::store::{Baseline, SessionStore};
use crate::stt::Segment;

/// Map a SignalKind to the snake_case string persisted in `events.kind`.
/// Mirrors the `#[serde(rename_all = "snake_case")]` on SignalKind so the DB
/// value matches what the UI's ipc layer expects on the wire too.
fn kind_str(kind: SignalKind) -> &'static str {
    match kind {
        SignalKind::RhythmFiller => "rhythm_filler",
        SignalKind::RhythmPace => "rhythm_pace",
        SignalKind::Repetition => "repetition",
    }
}

/// Spawn the analysis worker thread. Consumes `(segment_id, Segment)` pairs
/// from `rx` until it disconnects (i.e. the STT worker has exited and
/// dropped its sender), running each segment through the rhythm tracker and
/// repetition detector, persisting + forwarding any resulting signal, and
/// finally recording session-wide filler/word totals before returning.
pub fn spawn_analysis_worker(
    rx: Receiver<(i64, Segment)>,
    store: Arc<SessionStore>,
    session_id: i64,
    baseline: Option<Baseline>,
    signal_tx: Sender<Signal>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut rhythm = RhythmTracker::new(baseline);
        let mut repetition = RepetitionDetector::new();
        let mut total_words: i64 = 0;
        let mut total_fillers: i64 = 0;

        for (segment_id, segment) in rx.iter() {
            let words = word_count(&segment.text);
            let fillers = count_fillers(&segment.text);
            total_words += words as i64;
            total_fillers += fillers as i64;

            // Rhythm first: if both trackers fire on the same segment, keep
            // only the rhythm signal — one cue at a time. This permanently
            // drops that repetition signal (it is not queued or deferred),
            // which is intended: a point the user is truly repeating recurs
            // in later segments and will fire on its own once rhythm isn't
            // also hot, rather than the two cues competing for the user's
            // attention in the same instant.
            let rhythm_signal = rhythm.push(segment.start_ms, words, fillers);
            let repetition_signal = repetition.push(segment_id, segment.start_ms, &segment.text);
            let signal = rhythm_signal.or(repetition_signal);

            if let Some(signal) = signal {
                if let Err(e) = store.add_event(
                    session_id,
                    signal.at_ms,
                    kind_str(signal.kind),
                    &signal.note,
                ) {
                    eprintln!("analysis worker: add_event failed: {e}");
                }
                let _ = signal_tx.send(signal);
            }
        }

        // Channel disconnected: the STT worker has exited. Record final
        // session stats — log-and-continue, never let this take down
        // anything (end_session still needs to complete regardless).
        if let Err(e) = store.set_session_stats(session_id, total_fillers, total_words) {
            eprintln!("analysis worker: set_session_stats failed: {e}");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionStore;
    use std::time::Duration;

    fn base() -> Baseline {
        Baseline {
            fillers_per_min: 3.0,
            words_per_min: 150.0,
            sessions_counted: 5,
        }
    }

    fn join_with_watchdog(handle: JoinHandle<()>) {
        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);
        std::thread::spawn(move || {
            handle.join().unwrap();
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("analysis worker did not exit after channel close");
    }

    #[test]
    fn worker_persists_and_publishes_rhythm_signal() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let session_id = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (signal_tx, signal_rx) = crossbeam_channel::unbounded::<Signal>();

        let handle = spawn_analysis_worker(rx, store.clone(), session_id, Some(base()), signal_tx);

        // Calm dense history: distinct 10-word, filler-free segments spaced
        // 5s apart, establishing >=30 words / >=20s span in the rhythm
        // window. Deliberately all-different topics (not the same sentence
        // repeated) so the repetition detector's shingle overlap never
        // crosses its own threshold and fires ahead of the rhythm signal
        // this test is targeting.
        let calm = [
            "quiet morning walk through the local park today felt nice",
            "grocery shopping took much longer than planned this weekend somehow",
            "the printer finally started working again after that firmware update",
            "watched a documentary about deep sea creatures last night alone",
            "cleaned out the garage and found some old photo albums",
            "cooked a new pasta recipe that turned out pretty good",
        ];
        for (i, text) in calm.iter().enumerate() {
            let i = i as i64;
            tx.send((
                i,
                Segment {
                    start_ms: i * 5_000,
                    end_ms: i * 5_000 + 4_000,
                    text: (*text).into(),
                },
            ))
            .unwrap();
        }

        // Two consecutive hot-filler segments (real filler words, via the
        // actual count_fillers path) — sustained hysteresis should fire.
        let hot = "um uh like you know um uh yes it happened yesterday fine";
        tx.send((
            6,
            Segment {
                start_ms: 30_000,
                end_ms: 34_000,
                text: hot.into(),
            },
        ))
        .unwrap();
        tx.send((
            7,
            Segment {
                start_ms: 35_000,
                end_ms: 39_000,
                text: hot.into(),
            },
        ))
        .unwrap();

        drop(tx);
        join_with_watchdog(handle);

        let events = store.list_events(session_id).unwrap();
        assert!(
            events.iter().any(|e| e.kind == "rhythm_filler"),
            "expected a persisted rhythm_filler event, got: {events:?}"
        );

        let received = signal_rx
            .try_recv()
            .expect("signal must also be published on signal_tx");
        assert_eq!(received.kind, SignalKind::RhythmFiller);
    }

    #[test]
    fn worker_records_session_stats_on_close() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let session_id = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (signal_tx, _signal_rx) = crossbeam_channel::unbounded::<Signal>();

        let handle = spawn_analysis_worker(rx, store.clone(), session_id, None, signal_tx);

        let texts = [
            "um so I think this is fine yes",
            "totally different words here about coffee",
            "like you know actually it works",
        ];
        let mut expected_words = 0i64;
        let mut expected_fillers = 0i64;
        for (i, text) in texts.iter().enumerate() {
            expected_words += word_count(text) as i64;
            expected_fillers += count_fillers(text) as i64;
            tx.send((
                i as i64,
                Segment {
                    start_ms: i as i64 * 5_000,
                    end_ms: i as i64 * 5_000 + 2_000,
                    text: (*text).into(),
                },
            ))
            .unwrap();
        }
        drop(tx);
        join_with_watchdog(handle);

        let session = store.get_session(session_id).unwrap();
        assert_eq!(session.filler_count, Some(expected_fillers));
        assert_eq!(session.word_count, Some(expected_words));
    }
}
