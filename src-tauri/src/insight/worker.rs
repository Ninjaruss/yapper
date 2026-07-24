//! Slow-lane worker thread that drives the insight engine.
//!
//! Consumes transcript segments on an independent tee from the STT worker
//! (see `stt::worker`'s `insight_tx`), accumulates them into a full
//! session-transcript buffer, and on a relaxed cadence (~45s in production,
//! injectable for tests) builds a prompt from the current outline + recent
//! (last ~90s) transcript, calls the engine, and applies the result:
//! outline replacement, a spaced curious-listener question, a converging-
//! evidence-gated wrap-up, and an occasional shine moment. Engine failures
//! (network/model trouble) set `insight_failed` and are retried next
//! cadence; unparseable model output is silently skipped (it is not an
//! engine failure — the mirror keeps listening either way).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::insight::guard;
use crate::insight::prompt::{build_prompt, parse_update};
use crate::insight::{InsightEngine, OutlineEntry, OutlineStatus};
use crate::store::SessionStore;
use crate::stt::Segment;

/// How far back "recent" transcript reaches for the prompt, measured from
/// the newest buffered segment's `start_ms` (the speech clock — pause time
/// excluded, matching the WAV timeline).
const RECENT_WINDOW_MS: i64 = 90_000;

/// Minimum spacing between two questions shown to the user, on the speech
/// clock. "At most ONE question visible; replaced no earlier than 120s
/// after shown" (spec).
const QUESTION_SPACING_MS: i64 = 120_000;

/// Earliest a question may be asked at all, on the speech clock.
const FIRST_QUESTION_MIN_ELAPSED_MS: i64 = 60_000;

/// Minimum spacing between two shine moments, on the speech clock. The
/// first shine is allowed at any elapsed time.
const SHINE_SPACING_MS: i64 = 180_000;

/// Typical session length (ms) used for the wrap-up gate when no session
/// history exists yet (`typical_ms` is `None`).
const DEFAULT_TYPICAL_MS: i64 = 600_000;

/// Fraction of the typical session length that must have elapsed before
/// wrap-up may fire at all — "late in the session" (spec).
const WRAPUP_ELAPSED_FRACTION: f64 = 0.7;

/// How often the worker polls its channel while waiting for the next
/// cadence tick or a disconnect. Small enough that shutdown (channel close)
/// is noticed promptly; independent of `cadence_ms`.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Everything the worker needs beyond the engine/channels — session
/// identity, the user's stated intent (for the wrap-up intent-coverage
/// gate), and this installation's typical session length (for the wrap-up
/// elapsed-time gate).
pub struct InsightDeps {
    pub store: Arc<SessionStore>,
    pub session_id: i64,
    pub intent: String,
    pub typical_ms: Option<i64>,
}

/// Slow-lane events published to the UI forwarder. Each variant maps 1:1 to
/// an `insight:*` Tauri event (lib.rs wiring).
#[derive(Debug, Clone)]
pub enum InsightEvent {
    Outline(Vec<OutlineEntry>),
    Question(String),
    WrapupReady,
    Shine,
}

/// Spawn the insight worker thread. `rx` is the insight tee of transcribed
/// segments (independent of the analysis worker's tee — see
/// `stt::worker::spawn_stt_worker`'s `insight_tx` parameter); the thread
/// exits once `rx` disconnects, after running one final pass if segments
/// arrived since the last call.
///
/// `cadence_ms` is injectable so tests can run in milliseconds instead of
/// the production 45s; callers pass a real duration in lib.rs wiring.
pub fn spawn_insight_worker(
    mut engine: Box<dyn InsightEngine>,
    rx: Receiver<(i64, Segment)>,
    deps: InsightDeps,
    event_tx: Sender<InsightEvent>,
    insight_failed: Arc<AtomicBool>,
    cadence_ms: u64,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let InsightDeps {
            store,
            session_id,
            intent,
            typical_ms,
        } = deps;

        // The full session transcript, retained for the thread's lifetime;
        // "recent" (what actually goes in the prompt) is a filtered slice
        // computed fresh on each pass — see `run_insight_pass`.
        let mut buffer: Vec<(i64, Segment)> = Vec::new();
        let mut state = PassState::default();

        let cadence = Duration::from_millis(cadence_ms);
        let mut last_call_at = Instant::now();
        let mut new_since_last_call = false;

        loop {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok((id, seg)) => {
                    buffer.push((id, seg));
                    new_since_last_call = true;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    if new_since_last_call {
                        run_insight_pass(
                            engine.as_mut(),
                            &store,
                            session_id,
                            &intent,
                            typical_ms,
                            &buffer,
                            &mut state,
                            &event_tx,
                            &insight_failed,
                        );
                    }
                    break;
                }
            }

            if new_since_last_call && last_call_at.elapsed() >= cadence {
                run_insight_pass(
                    engine.as_mut(),
                    &store,
                    session_id,
                    &intent,
                    typical_ms,
                    &buffer,
                    &mut state,
                    &event_tx,
                    &insight_failed,
                );
                last_call_at = Instant::now();
                new_since_last_call = false;
            }
        }
    })
}

/// Mutable state carried between passes — the in-memory outline mirror plus
/// the spacing bookkeeping for questions/wrap-up/shine. All timestamps are
/// on the speech clock (a segment's `start_ms`), never wall-clock time, so
/// spacing rules hold regardless of how long the insight call itself took
/// or how long the user paused.
#[derive(Default)]
struct PassState {
    current_outline: Vec<OutlineEntry>,
    last_question: Option<String>,
    last_question_elapsed_ms: Option<i64>,
    last_shine_elapsed_ms: Option<i64>,
    wrapup_fired: bool,
}

/// Runs one insight pass: builds the prompt from `buffer`'s recent window
/// and `state.current_outline`, calls the engine, parses the result, and
/// applies whichever of outline/question/wrapup/shine the update contains.
/// Every persistence/emit step is log-and-continue — a failure in one
/// sub-step (e.g. a DB error persisting the outline) must not block the
/// others in the same pass.
#[allow(clippy::too_many_arguments)]
fn run_insight_pass(
    engine: &mut dyn InsightEngine,
    store: &Arc<SessionStore>,
    session_id: i64,
    intent: &str,
    typical_ms: Option<i64>,
    buffer: &[(i64, Segment)],
    state: &mut PassState,
    event_tx: &Sender<InsightEvent>,
    insight_failed: &Arc<AtomicBool>,
) {
    let Some((_, newest)) = buffer.last() else {
        return;
    };
    let elapsed_ms = newest.start_ms;

    let recent: Vec<(i64, String)> = buffer
        .iter()
        .filter(|(_, seg)| seg.start_ms >= elapsed_ms - RECENT_WINDOW_MS)
        .map(|(_, seg)| (seg.start_ms, seg.text.clone()))
        .collect();

    let prompt = build_prompt(intent, &state.current_outline, &recent, elapsed_ms);

    let raw = match engine.insight(&prompt) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("insight worker: engine call failed: {e}");
            insight_failed.store(true, Ordering::Relaxed);
            return;
        }
    };

    let Some(update) = parse_update(&raw, state.last_question.as_deref()) else {
        eprintln!("insight: unparseable output, skipping");
        return;
    };

    apply_outline(
        store,
        session_id,
        elapsed_ms,
        state,
        event_tx,
        &update.outline,
    );
    apply_question(
        store,
        session_id,
        elapsed_ms,
        state,
        event_tx,
        update.question.as_deref(),
        update.sparked_by.as_deref(),
        &recent,
    );
    apply_wrapup(
        store,
        session_id,
        elapsed_ms,
        intent,
        typical_ms,
        state,
        event_tx,
        update.wrapup_ready,
    );
    apply_shine(store, session_id, elapsed_ms, state, event_tx, update.shine);
}

fn outline_status_str(status: OutlineStatus) -> &'static str {
    match status {
        OutlineStatus::Covered => "covered",
        OutlineStatus::Current => "current",
        OutlineStatus::IntentUntouched => "intent_untouched",
    }
}

/// A non-empty outline that differs from the current in-memory snapshot is
/// a full-replacement persist + publish; an empty or unchanged outline is a
/// no-op (the model has nothing new to say this pass).
fn apply_outline(
    store: &Arc<SessionStore>,
    session_id: i64,
    elapsed_ms: i64,
    state: &mut PassState,
    event_tx: &Sender<InsightEvent>,
    outline: &[OutlineEntry],
) {
    if outline.is_empty() {
        return;
    }
    // Damp renames BEFORE the no-change comparison: a pure reword must
    // resolve to the existing outline and become a no-op.
    let damped = guard::damp_labels(&state.current_outline, outline);
    if damped == state.current_outline {
        return;
    }
    state.current_outline = damped;
    let entries: Vec<(&str, &str)> = state
        .current_outline
        .iter()
        .map(|e| (e.label.as_str(), outline_status_str(e.status)))
        .collect();
    if let Err(e) = store.replace_outline(session_id, &entries, elapsed_ms) {
        eprintln!("insight worker: replace_outline failed: {e}");
    }
    let _ = event_tx.send(InsightEvent::Outline(state.current_outline.clone()));
}

/// A question is only shown if it clears BOTH gates: the grounding gate
/// (sparked_by must quote the recent transcript — hallucinated citations
/// die here) and the spacing gate (60s first-question floor, then 120s
/// between questions, on the speech clock). Failing either simply drops
/// the question this pass.
#[allow(clippy::too_many_arguments)]
fn apply_question(
    store: &Arc<SessionStore>,
    session_id: i64,
    elapsed_ms: i64,
    state: &mut PassState,
    event_tx: &Sender<InsightEvent>,
    question: Option<&str>,
    sparked_by: Option<&str>,
    recent: &[(i64, String)],
) {
    let Some(q) = question else {
        return;
    };
    let Some(spark) = sparked_by else {
        return; // no citation, no question
    };
    if !guard::is_grounded(spark, recent) {
        return; // citation not found in the recent window
    }
    let allowed = match state.last_question_elapsed_ms {
        Some(last_elapsed) => elapsed_ms - last_elapsed >= QUESTION_SPACING_MS,
        None => elapsed_ms >= FIRST_QUESTION_MIN_ELAPSED_MS,
    };
    if !allowed {
        return;
    }
    if let Err(e) = store.add_event(session_id, elapsed_ms, "question", q) {
        eprintln!("insight worker: add_event(question) failed: {e}");
    }
    let _ = event_tx.send(InsightEvent::Question(q.to_string()));
    state.last_question = Some(q.to_string());
    state.last_question_elapsed_ms = Some(elapsed_ms);
}

/// Wrap-up fires at most once per session ("silence alone never means
/// wrap-up" — the model's `wrapup_ready` vote is only one of two required
/// signals): elapsed time must clear 70% of the typical session length
/// (default 10 minutes with no history), AND the intent must be either
/// unset or fully covered (no `IntentUntouched` entries left in the
/// current outline).
#[allow(clippy::too_many_arguments)]
fn apply_wrapup(
    store: &Arc<SessionStore>,
    session_id: i64,
    elapsed_ms: i64,
    intent: &str,
    typical_ms: Option<i64>,
    state: &mut PassState,
    event_tx: &Sender<InsightEvent>,
    wrapup_ready: bool,
) {
    if state.wrapup_fired || !wrapup_ready {
        return;
    }
    let gate_ms =
        (WRAPUP_ELAPSED_FRACTION * typical_ms.unwrap_or(DEFAULT_TYPICAL_MS) as f64) as i64;
    if elapsed_ms < gate_ms {
        return;
    }
    let intent_covered = intent.trim().is_empty()
        || !state
            .current_outline
            .iter()
            .any(|e| e.status == OutlineStatus::IntentUntouched);
    if !intent_covered {
        return;
    }
    if let Err(e) = store.add_event(session_id, elapsed_ms, "wrapup", "") {
        eprintln!("insight worker: add_event(wrapup) failed: {e}");
    }
    let _ = event_tx.send(InsightEvent::WrapupReady);
    state.wrapup_fired = true;
}

/// Shine fires when the model flags the most recent stretch as notably
/// personal/deep, gated only by a minimum spacing since the last shine (the
/// very first shine is allowed at any elapsed time).
fn apply_shine(
    store: &Arc<SessionStore>,
    session_id: i64,
    elapsed_ms: i64,
    state: &mut PassState,
    event_tx: &Sender<InsightEvent>,
    shine: bool,
) {
    if !shine {
        return;
    }
    let allowed = match state.last_shine_elapsed_ms {
        Some(last_elapsed) => elapsed_ms - last_elapsed >= SHINE_SPACING_MS,
        None => true,
    };
    if !allowed {
        return;
    }
    if let Err(e) = store.add_event(session_id, elapsed_ms, "shine", "") {
        eprintln!("insight worker: add_event(shine) failed: {e}");
    }
    let _ = event_tx.send(InsightEvent::Shine);
    state.last_shine_elapsed_ms = Some(elapsed_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insight::MockInsight;
    use crate::store::SessionStore;

    fn join_with_watchdog(handle: JoinHandle<()>) {
        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);
        std::thread::spawn(move || {
            handle.join().unwrap();
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("insight worker did not exit in time");
    }

    fn seg(start_ms: i64, text: &str) -> Segment {
        Segment {
            start_ms,
            end_ms: start_ms + 1_000,
            text: text.to_string(),
        }
    }

    fn deps(store: Arc<SessionStore>, session_id: i64, intent: &str) -> InsightDeps {
        InsightDeps {
            store,
            session_id,
            intent: intent.to_string(),
            typical_ms: None,
        }
    }

    /// A tiny test double that always errors — `MockInsight` has no way to
    /// script a failure, so engine-error handling needs its own type.
    struct FailingInsight;
    impl InsightEngine for FailingInsight {
        fn insight(&mut self, _prompt: &str) -> Result<String, crate::error::YapperError> {
            Err(crate::error::YapperError::State("engine boom".into()))
        }
    }

    #[test]
    fn outline_persisted_and_published() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "explore topic a and b").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let script = vec![r#"{"outline":[{"label":"Topic A","status":"current"},{"label":"Topic B","status":"intent_untouched"}],"question":null,"wrapup_ready":false,"shine":false}"#.to_string()];
        let engine = Box::new(MockInsight::new(script));

        let handle = spawn_insight_worker(
            engine,
            rx,
            deps(store.clone(), sid, "explore topic a and b"),
            event_tx,
            insight_failed.clone(),
            50,
        );

        tx.send((1, seg(65_000, "talking about topic a"))).unwrap();
        drop(tx);
        join_with_watchdog(handle);

        let outline = store.list_outline(sid).unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].label, "Topic A");
        assert_eq!(outline[0].status, "current");

        match event_rx.try_recv().expect("expected an Outline event") {
            InsightEvent::Outline(entries) => assert_eq!(entries.len(), 2),
            other => panic!("expected Outline event, got {other:?}"),
        }
        assert!(!insight_failed.load(Ordering::Relaxed));
    }

    #[test]
    fn question_spacing_enforced_on_speech_clock() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let script = vec![
            r#"{"outline":[],"question":"What did the quiet feel like?","sparked_by":"one","wrapup_ready":false,"shine":false}"#.to_string(),
            r#"{"outline":[],"question":"What surprised you most about that?","sparked_by":"two","wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));

        let handle = spawn_insight_worker(
            engine,
            rx,
            deps(store.clone(), sid, ""),
            event_tx,
            insight_failed.clone(),
            50,
        );

        // First pass: elapsed 65s clears the 60s first-question floor.
        tx.send((1, seg(65_000, "one"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        // Second pass: elapsed 115s is only 50s after the first question
        // shown — under the 120s spacing floor, so it must be suppressed.
        tx.send((2, seg(115_000, "two"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        drop(tx);
        join_with_watchdog(handle);

        let question_events = event_rx
            .try_iter()
            .filter(|e| matches!(e, InsightEvent::Question(_)))
            .count();
        assert_eq!(question_events, 1, "only the first question should fire");

        let question_db_events = store
            .list_events(sid)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "question")
            .count();
        assert_eq!(question_db_events, 1);
        assert!(!insight_failed.load(Ordering::Relaxed));
    }

    #[test]
    fn malformed_output_is_skipped() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let script = vec![
            "not json at all, sorry can't help with that".to_string(),
            r#"{"outline":[{"label":"Valid Topic","status":"current"}],"question":null,"wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));

        let handle = spawn_insight_worker(
            engine,
            rx,
            deps(store.clone(), sid, ""),
            event_tx,
            insight_failed.clone(),
            50,
        );

        tx.send((1, seg(65_000, "one"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        tx.send((2, seg(130_000, "two"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        drop(tx);
        join_with_watchdog(handle);

        let outline = store.list_outline(sid).unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].label, "Valid Topic");

        let outline_events = event_rx
            .try_iter()
            .filter(|e| matches!(e, InsightEvent::Outline(_)))
            .count();
        assert_eq!(outline_events, 1, "garbage pass must not publish anything");

        // Unparseable output is not an engine failure.
        assert!(!insight_failed.load(Ordering::Relaxed));
    }

    #[test]
    fn engine_error_sets_flag_keeps_running() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let engine = Box::new(FailingInsight);

        let handle = spawn_insight_worker(
            engine,
            rx,
            deps(store.clone(), sid, ""),
            event_tx,
            insight_failed.clone(),
            50,
        );

        tx.send((1, seg(65_000, "one"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        drop(tx);

        // The worker must exit cleanly (no panic) even though the engine
        // errored on every call.
        join_with_watchdog(handle);

        assert!(insight_failed.load(Ordering::Relaxed));
        assert!(
            event_rx.try_recv().is_err(),
            "an engine failure must not publish any insight event"
        );
        assert!(store.list_outline(sid).unwrap().is_empty());
    }

    #[test]
    fn wrapup_gate_blocks_early() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        // Model insists it's ready to wrap up on every single pass; the
        // Rust-side gate must be the one actually deciding.
        let script = vec![
            r#"{"outline":[],"question":null,"wrapup_ready":true,"shine":false}"#.to_string(),
            r#"{"outline":[],"question":null,"wrapup_ready":true,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));

        // typical_ms = 100_000 -> gate = 70_000ms (70%).
        let mut d = deps(store.clone(), sid, "");
        d.typical_ms = Some(100_000);

        let handle = spawn_insight_worker(engine, rx, d, event_tx, insight_failed.clone(), 50);

        // First pass: elapsed 50s, well under the 70s gate -> must not fire.
        tx.send((1, seg(50_000, "one"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        // Second pass: elapsed 80s clears the gate, empty intent counts as
        // covered -> fires exactly once.
        tx.send((2, seg(80_000, "two"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));

        drop(tx);
        join_with_watchdog(handle);

        let wrapup_events = event_rx
            .try_iter()
            .filter(|e| matches!(e, InsightEvent::WrapupReady))
            .count();
        assert_eq!(
            wrapup_events, 1,
            "wrapup must fire exactly once, and only late"
        );

        let wrapup_db_events = store
            .list_events(sid)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "wrapup")
            .count();
        assert_eq!(wrapup_db_events, 1);
        assert!(!insight_failed.load(Ordering::Relaxed));
    }

    #[test]
    fn ungrounded_question_is_dropped() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        // sparked_by does not appear in the transcript -> must be dropped.
        let script = vec![
            r#"{"outline":[],"question":"What was the hardest part?","sparked_by":"my biggest takeaway","wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));
        let handle = spawn_insight_worker(
            engine, rx, deps(store.clone(), sid, ""), event_tx, insight_failed.clone(), 50,
        );

        tx.send((1, seg(65_000, "talking about the empty apartment"))).unwrap();
        drop(tx);
        join_with_watchdog(handle);

        let question_events = event_rx
            .try_iter()
            .filter(|e| matches!(e, InsightEvent::Question(_)))
            .count();
        assert_eq!(question_events, 0, "ungrounded question must not surface");
        assert!(store.list_events(sid).unwrap().iter().all(|e| e.kind != "question"));
    }

    #[test]
    fn grounded_question_passes_the_gate() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let script = vec![
            r#"{"outline":[],"question":"What did the empty apartment feel like?","sparked_by":"the empty apartment","wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));
        let handle = spawn_insight_worker(
            engine, rx, deps(store.clone(), sid, ""), event_tx, insight_failed.clone(), 50,
        );

        tx.send((1, seg(65_000, "talking about the empty apartment"))).unwrap();
        drop(tx);
        join_with_watchdog(handle);

        let question_events = event_rx
            .try_iter()
            .filter(|e| matches!(e, InsightEvent::Question(_)))
            .count();
        assert_eq!(question_events, 1);
    }

    #[test]
    fn renamed_outline_label_is_damped() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let script = vec![
            r#"{"outline":[{"label":"Moving to Austin","status":"current"}],"question":null,"wrapup_ready":false,"shine":false}"#.to_string(),
            r#"{"outline":[{"label":"The Austin move","status":"covered"}],"question":null,"wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));
        let handle = spawn_insight_worker(
            engine, rx, deps(store.clone(), sid, ""), event_tx, insight_failed.clone(), 50,
        );

        tx.send((1, seg(65_000, "one"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        tx.send((2, seg(130_000, "two"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        drop(tx);
        join_with_watchdog(handle);

        let outline = store.list_outline(sid).unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].label, "Moving to Austin", "label text must survive the rename");
        assert_eq!(outline[0].status, "covered", "status change must still apply");

        // Both passes changed something (status), so two Outline events is
        // fine — what matters is the label never changed.
        for e in event_rx.try_iter() {
            if let InsightEvent::Outline(entries) = e {
                assert_eq!(entries[0].label, "Moving to Austin");
            }
        }
    }
}
