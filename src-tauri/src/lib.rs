pub mod analysis;
pub mod audio;
pub mod error;
pub mod export;
pub mod insight;
pub mod models;
pub mod session;
pub mod store;
pub mod stt;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{Emitter, Manager, State};

use analysis::Signal;
use analysis::rhythm::ratio_bonus;
use audio::capture::Capture;
use error::YapperError;
use insight::worker::{InsightDeps, InsightEvent};
use insight::InsightEngine;
use session::{ClockState, SessionClock};
use store::{Event, OutlineRow, RetroRow, Session, SessionStore, TranscriptSegment};
use stt::{Segment, TranscribeEngine};

/// Insight worker cadence in production (injectable for tests — see
/// `insight::worker::spawn_insight_worker`'s `cadence_ms` parameter).
const INSIGHT_CADENCE_MS: u64 = 45_000;

/// A completed session must have at least this much actual speaking time
/// (duration minus paused time) before it's trusted to contribute to the
/// baseline — short sessions produce noisy per-minute rates.
const MIN_SPEAKING_MS_FOR_BASELINE: i64 = 120_000;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

struct ActiveSession {
    id: i64,
    clock: SessionClock,
    capture: Capture,
    stt_worker: Option<JoinHandle<()>>,
    stt_failed: Arc<AtomicBool>,
    analysis_worker: Option<JoinHandle<()>>,
    insight_worker: Option<JoinHandle<()>>,
    insight_failed: Arc<AtomicBool>,
}

struct AppState {
    store: Arc<SessionStore>,
    active: Mutex<Option<ActiveSession>>,
}

#[tauri::command]
fn list_input_devices() -> Result<Vec<audio::InputDevice>, YapperError> {
    audio::list_input_devices()
}

// ASYNC IS LOAD-BEARING on this command and on end_session: Tauri runs
// non-async commands on the MAIN thread. Capture start/stop block on audio
// threads, and macOS CoreAudio teardown misbehaves while the main run loop
// is parked (observed via thread sample: zombie input callback → writer
// join never returns → app beachball). Async commands run on the runtime's
// worker threads, keeping the main loop live. Do not remove `async`.
#[tauri::command]
async fn start_session(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    intent: String,
    device_name: Option<String>,
) -> Result<i64, YapperError> {
    // First check: fail fast if a session is already running, without
    // holding the guard across the `.await` below (a std MutexGuard is not
    // Send, so holding one across an await point would break this future's
    // Send bound — a compile error on a multi-threaded async runtime).
    {
        let active = state
            .active
            .lock()
            .map_err(|_| YapperError::State("state lock poisoned".into()))?;
        if active.is_some() {
            return Err(YapperError::State("a session is already running".into()));
        }
    }

    // If the Moonshine model is present, load it off the async worker via
    // `spawn_blocking` (model init does blocking file I/O) *before* touching
    // any session state. If models are missing or init fails, recording must
    // still proceed without STT (zero-setup / mirror-never-stops principle).
    let stt_dir = models::model_dir_for(&app, &models::STT_MODEL).ok();
    let engine: Option<Box<dyn TranscribeEngine>> = match stt_dir {
        Some(dir) if models::files_present(&dir, models::STT_MODEL.files) => {
            match tauri::async_runtime::spawn_blocking(move || {
                stt::moonshine::MoonshineEngine::new(&dir)
            })
            .await
            {
                Ok(Ok(e)) => Some(Box::new(e) as Box<dyn TranscribeEngine>),
                Ok(Err(e)) => {
                    eprintln!("stt: MoonshineEngine::new failed, continuing without STT: {e}");
                    None
                }
                Err(join_err) => {
                    eprintln!("stt: model init task panicked, continuing without STT: {join_err}");
                    None
                }
            }
        }
        _ => None,
    };

    // Same pattern for the insight (LLM) engine, gated on both the model
    // being present AND STT actually being active this session — an LLM
    // with no transcript to read is useless, so there is nothing to spawn
    // it for. Loaded before the second lock check for the same reason as
    // the STT engine above: model init does blocking file I/O and must not
    // happen while holding the state lock.
    let insight_engine: Option<Box<dyn InsightEngine>> = if engine.is_some()
        && models::model_present(&app, &models::LLM_MODEL)
    {
        match models::model_dir_for(&app, &models::LLM_MODEL) {
            Ok(dir) => {
                match tauri::async_runtime::spawn_blocking(move || {
                    insight::llama::LlamaEngine::new(&dir)
                })
                .await
                {
                    Ok(Ok(e)) => Some(Box::new(e) as Box<dyn InsightEngine>),
                    Ok(Err(e)) => {
                        eprintln!(
                            "insight: LlamaEngine::new failed, continuing without insight: {e}"
                        );
                        None
                    }
                    Err(join_err) => {
                        eprintln!(
                            "insight: model init task panicked, continuing without insight: {join_err}"
                        );
                        None
                    }
                }
            }
            Err(e) => {
                eprintln!("insight: could not resolve model dir, continuing without insight: {e}");
                None
            }
        }
    } else {
        None
    };

    // Second check: another session may have started while we were awaiting
    // model init above. Re-acquire the lock and re-check before touching any
    // session state; if one snuck in, bail out (the just-loaded `engine` and
    // `insight_engine`, if any, are simply dropped here).
    let mut active = state
        .active
        .lock()
        .map_err(|_| YapperError::State("state lock poisoned".into()))?;
    if active.is_some() {
        return Err(YapperError::State("a session is already running".into()));
    }

    let started = now_ms();
    // Stamp the take with the experiment it carries in (the most recent
    // retro's try_next) so the recap can echo it even after newer retros
    // exist. Best-effort: a lookup failure must never block recording.
    let focus = state.store.latest_try_next().unwrap_or(None);
    let id = state
        .store
        .create_session_with_focus(started, &intent, focus.as_deref())?;

    let tee = engine
        .is_some()
        .then(|| crossbeam_channel::bounded::<Vec<f32>>(256));
    let tee_tx = tee.as_ref().map(|(tx, _)| tx.clone());

    let setup_result = (|| -> Result<Capture, YapperError> {
        // Recordings live somewhere the user can actually find them
        // (~/Music/Yapper on macOS); the hidden app-data dir is only a fallback.
        let audio_dir = app
            .path()
            .audio_dir()
            .map(|d| d.join("Yapper"))
            .or_else(|_| app.path().app_data_dir().map(|d| d.join("audio")))
            .map_err(|e| YapperError::Audio(e.to_string()))?;
        std::fs::create_dir_all(&audio_dir)?;
        let wav_path = audio_dir.join(format!("session-{id}.wav"));
        Capture::start(device_name.as_deref(), wav_path, tee_tx)
    })();

    let capture = match setup_result {
        Ok(capture) => capture,
        Err(e) => {
            // Session row was created above; without a working capture it's
            // an orphan, so clean it up before surfacing the error.
            let _ = state.store.delete_session(id);
            return Err(e);
        }
    };

    // Forward levels to the UI. The forwarder exits when the capture's level
    // channel disconnects at stop(); levels are best-effort (bounded producer).
    let level_rx = capture.level_rx.clone();
    let emit_app = app.clone();
    std::thread::spawn(move || {
        while let Ok(level) = level_rx.recv() {
            let _ = emit_app.emit("audio:level", level);
        }
    });

    let stt_failed = Arc::new(AtomicBool::new(false));
    let insight_failed = Arc::new(AtomicBool::new(false));
    // Analysis and insight only ever run alongside STT — both consume the
    // segments STT produces, so they have nothing to do on the no-STT path.
    let mut analysis_worker = None;
    let mut insight_worker = None;
    let stt_worker = if let (Some(engine), Some((_tee_tx, tee_rx))) = (engine, tee) {
        let (seg_tx, seg_rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        // Forward transcribed segments to the UI, same pattern as the level
        // forwarder above; exits when the segment channel closes (i.e. once
        // the worker thread finishes and drops its sender). The DB id rides
        // along so the UI can tag transcript lines with `data-segment-id`
        // for the repetition-echo glow.
        let emit_app = app.clone();
        std::thread::spawn(move || {
            while let Ok((id, seg)) = seg_rx.recv() {
                let payload = serde_json::json!({
                    "id": id,
                    "start_ms": seg.start_ms,
                    "end_ms": seg.end_ms,
                    "text": seg.text,
                });
                let _ = emit_app.emit("transcript:segment", payload);
            }
        });

        let (analysis_tx, analysis_rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (signal_tx, signal_rx) = crossbeam_channel::unbounded::<Signal>();

        // Cold start = silence: only feed a baseline into the rhythm tracker
        // once at least MIN_BASELINE_SESSIONS have been completed.
        let baseline = state
            .store
            .get_baseline()?
            .filter(|b| b.sessions_counted >= 3);

        // Fetch wrong-feedback counts to widen rhythm thresholds adaptively.
        // Log-and-default-0 on error to never fail session startup over this.
        let filler_wrong_count = state
            .store
            .count_wrong_feedback("rhythm_filler")
            .unwrap_or_else(|e| {
                eprintln!("start_session: count_wrong_feedback(rhythm_filler) failed: {e}");
                0
            });
        let pace_wrong_count = state
            .store
            .count_wrong_feedback("rhythm_pace")
            .unwrap_or_else(|e| {
                eprintln!("start_session: count_wrong_feedback(rhythm_pace) failed: {e}");
                0
            });
        let filler_bonus = ratio_bonus(filler_wrong_count);
        let pace_bonus = ratio_bonus(pace_wrong_count);

        analysis_worker = Some(analysis::worker::spawn_analysis_worker(
            analysis_rx,
            state.store.clone(),
            id,
            baseline,
            signal_tx,
            filler_bonus,
            pace_bonus,
        ));

        // Forward signals to the UI, same pattern as levels/segments; exits
        // when the analysis worker finishes and drops its sender.
        let emit_app = app.clone();
        std::thread::spawn(move || {
            while let Ok(sig) = signal_rx.recv() {
                let _ = emit_app.emit("analysis:signal", &sig);
            }
        });

        // Insight only spawns when the LLM engine actually loaded above
        // (gated on model presence + STT being active). `insight_seg_tx`
        // becomes the STT worker's third tee; `None` here means the STT
        // worker simply never sends on that tee (see `stt::worker`'s
        // `insight_tx` parameter).
        let insight_seg_tx = if let Some(insight_engine) = insight_engine {
            let (insight_seg_tx, insight_seg_rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
            let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();

            let typical_ms = state.store.typical_session_ms().ok().flatten();
            let deps = InsightDeps {
                store: state.store.clone(),
                session_id: id,
                intent: intent.clone(),
                typical_ms,
            };
            insight_worker = Some(insight::worker::spawn_insight_worker(
                insight_engine,
                insight_seg_rx,
                deps,
                event_tx,
                insight_failed.clone(),
                INSIGHT_CADENCE_MS,
            ));

            // Forward insight events to the UI, same pattern as levels/
            // segments/signals; exits when the insight worker finishes and
            // drops its sender.
            let emit_app = app.clone();
            std::thread::spawn(move || {
                while let Ok(event) = event_rx.recv() {
                    match event {
                        InsightEvent::Outline(entries) => {
                            let _ = emit_app.emit("insight:outline", entries);
                        }
                        InsightEvent::Question(q) => {
                            let _ = emit_app.emit("insight:question", q);
                        }
                        InsightEvent::WrapupReady => {
                            let _ = emit_app.emit("insight:wrapup", true);
                        }
                        InsightEvent::Shine => {
                            let _ = emit_app.emit("insight:shine", true);
                        }
                    }
                }
            });

            Some(insight_seg_tx)
        } else {
            None
        };

        Some(stt::worker::spawn_stt_worker(
            engine,
            capture.sample_rate,
            tee_rx,
            state.store.clone(),
            id,
            seg_tx,
            stt_failed.clone(),
            Some(analysis_tx),
            insight_seg_tx,
        ))
    } else {
        None
    };

    *active = Some(ActiveSession {
        id,
        clock: SessionClock::start(started),
        capture,
        stt_worker,
        stt_failed,
        analysis_worker,
        insight_worker,
        insight_failed,
    });
    Ok(id)
}

#[tauri::command]
fn pause_listening(state: State<'_, AppState>) -> Result<(), YapperError> {
    let mut active = state
        .active
        .lock()
        .map_err(|_| YapperError::State("state lock poisoned".into()))?;
    let s = active
        .as_mut()
        .ok_or_else(|| YapperError::State("no session".into()))?;
    s.clock.pause(now_ms())?;
    s.capture
        .paused
        .store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn resume_listening(state: State<'_, AppState>) -> Result<(), YapperError> {
    let mut active = state
        .active
        .lock()
        .map_err(|_| YapperError::State("state lock poisoned".into()))?;
    let s = active
        .as_mut()
        .ok_or_else(|| YapperError::State("no session".into()))?;
    s.clock.resume(now_ms())?;
    s.capture
        .paused
        .store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[derive(serde::Serialize)]
struct SessionStatus {
    id: i64,
    state: String,
    elapsed_ms: i64,
    writer_failed: bool,
    stt_active: bool,
    stt_failed: bool,
    insight_active: bool,
    insight_failed: bool,
}

#[tauri::command]
fn session_status(state: State<'_, AppState>) -> Result<Option<SessionStatus>, YapperError> {
    let active = state
        .active
        .lock()
        .map_err(|_| YapperError::State("state lock poisoned".into()))?;
    Ok(active.as_ref().map(|s| SessionStatus {
        id: s.id,
        state: match s.clock.state() {
            ClockState::Recording => "recording",
            ClockState::Paused => "paused",
            ClockState::Ended => "ended",
        }
        .to_string(),
        elapsed_ms: s.clock.elapsed_ms(now_ms()),
        writer_failed: s.capture.writer_failed.load(Ordering::Relaxed),
        stt_active: s.stt_worker.is_some(),
        stt_failed: s.stt_failed.load(Ordering::Relaxed),
        insight_active: s.insight_worker.is_some(),
        insight_failed: s.insight_failed.load(Ordering::Relaxed),
    }))
}

// Async for the same main-thread reason as start_session — see comment there.
#[tauri::command]
async fn end_session(state: State<'_, AppState>) -> Result<Session, YapperError> {
    let mut active = state
        .active
        .lock()
        .map_err(|_| YapperError::State("state lock poisoned".into()))?;
    let mut s = active
        .take()
        .ok_or_else(|| YapperError::State("no session".into()))?;
    let totals = s.clock.end(now_ms());
    // Capture the known WAV path before stop() consumes self, so the DB
    // record can still be written even if stop() itself errors out — the
    // WAV is flushed per buffer, so the file is still largely recoverable.
    let wav_path = s.capture.wav_path.clone();
    let stop_result = s.capture.stop();
    // Guard dropped here so status/pause polls stay live during the
    // (LLM-bounded) worker joins: the mic is already released by this point
    // (Capture::stop joins the stream thread internally), `s` is owned by
    // this function from here on, and SessionStore self-synchronizes — so
    // nothing below needs `state.active` held.
    drop(active);
    // Join the STT worker immediately after capture.stop(), and before the
    // fallible DB write below: stop() has already sent the tee's shutdown
    // sentinel (and disconnected the channel), so the worker flushes
    // trailing audio and exits here. Joining unconditionally — rather than
    // after a `?` that might return early — ensures the handle is never
    // silently dropped on the DB-error path.
    if let Some(handle) = s.stt_worker.take() {
        let _ = handle.join();
    }
    // The STT worker's exit (above) drops its analysis_tx clone, which
    // disconnects the analysis channel and lets this worker drain and
    // return in bounded time — safe to join unconditionally right here,
    // same reasoning as the STT join above.
    if let Some(handle) = s.analysis_worker.take() {
        let _ = handle.join();
    }
    // Same reasoning again, one more link down the chain: the STT worker's
    // exit also dropped its insight_tx clone (its third tee), which
    // disconnects the insight channel and lets the insight worker run its
    // final pass (if segments arrived since its last cadence tick) and
    // return. Joined last — and after the analysis join above, not
    // concurrently with it — so the shutdown order is always
    // stt -> analysis -> insight -> DB write, matching every other worker
    // join in this function.
    if let Some(handle) = s.insight_worker.take() {
        let _ = handle.join();
    }
    let final_path = stop_result.as_ref().unwrap_or(&wav_path);
    // If both capture.stop() and store.end_session fail, log the stop error
    // before surfacing the store error (since the ? on stop_result below
    // would otherwise drop the stop error, making debugging harder).
    if let Err(store_err) = state.store.end_session(
        s.id,
        totals.ended_at_ms,
        final_path.to_string_lossy().as_ref(),
        totals.paused_ms,
    ) {
        if let Err(stop_err) = &stop_result {
            eprintln!("end_session: capture stop also failed: {stop_err}");
        }
        return Err(store_err);
    }
    // Clone the just-written path before `stop_result?` below consumes
    // `stop_result` (and with it, `final_path`'s borrow) — the spawned
    // encode thread needs an owned copy regardless of which branch this
    // takes next.
    let recorded_path = final_path.clone();
    stop_result?;

    // Baseline learning: after the DB end write so duration/paused_ms exist.
    // Never fail end_session over this — log and move on.
    if let Err(e) = update_baseline_after_session(&state.store, s.id) {
        eprintln!("end_session: baseline update failed: {e}");
    }

    // Compress the WAV to FLAC on a detached, fire-and-forget thread: the DB
    // end-write above already succeeded and capture.stop() didn't error (we
    // only get here past the `?` on stop_result), so the take is safely
    // recorded either way. Encoding a long WAV can take a while, so this
    // must not block end_session (the UI awaits this command) or hold any
    // lock — the thread only touches the filesystem and, on success, calls
    // the store's own self-synchronizing `set_audio_path`.
    //
    // Staleness note: the recap screen (and Past Talks) may briefly show the
    // old .wav path until their next poll. That's fine in practice — trace:
    // setup.ts's `refreshPast` builds `pastKey` from
    // `id:duration_ms:audio_exists:segment_count` (not audio_path), so a
    // WAV→FLAC swap alone won't force a DOM re-render. But the row's "Show
    // file" / "Export transcript" buttons don't carry a cached path at all —
    // their onclick handlers call `reveal_session`/`export_transcript` by
    // session id, and those Tauri commands re-fetch `session.audio_path`
    // from the DB fresh on every invocation (see lib.rs below). So the very
    // next click after encoding finishes reveals/exports the FLAC, not a
    // stale WAV reference — no pastKey change needed for correctness, only
    // for how soon the row's own display would reflect it (it doesn't
    // display the path at all).
    {
        let store_for_encode = state.store.clone();
        let session_id = s.id;
        std::thread::spawn(move || match audio::encode::wav_to_flac(&recorded_path) {
            Ok(flac_path) => {
                let flac_str = flac_path.to_string_lossy().into_owned();
                match store_for_encode.set_audio_path(session_id, &flac_str) {
                    Ok(()) => eprintln!(
                        "end_session: compressed session {session_id} audio to {flac_str}"
                    ),
                    Err(e) => eprintln!(
                        "end_session: flac encode ok but set_audio_path failed for session {session_id}: {e}"
                    ),
                }
            }
            Err(e) => {
                eprintln!(
                    "end_session: flac encode failed for session {session_id} (wav kept): {e}"
                );
            }
        });
    }

    state.store.get_session(s.id).map(with_audio_exists)
}

/// Pure gate for whether a session's stats should be folded into the
/// baseline: must have an actual transcript (`word_count` present AND
/// nonzero — a session with STT active but nobody speaking still writes
/// `Some(0)`, and 0 wpm would drag the rolling mean toward 0 and eventually
/// make RhythmPace fire on any speech at all) and at least
/// `MIN_SPEAKING_MS_FOR_BASELINE` of actual speaking time (duration minus
/// paused time) — short sessions produce noisy per-minute rates.
fn should_update_baseline(
    word_count: Option<i64>,
    duration_ms: Option<i64>,
    paused_ms: i64,
) -> bool {
    let Some(duration_ms) = duration_ms else {
        return false;
    };
    match word_count {
        Some(w) if w > 0 => (duration_ms - paused_ms) >= MIN_SPEAKING_MS_FOR_BASELINE,
        _ => false,
    }
}

/// Roll this session's filler/word rates into the running baseline; see
/// `should_update_baseline` for the gating conditions.
fn update_baseline_after_session(
    store: &Arc<SessionStore>,
    session_id: i64,
) -> Result<(), YapperError> {
    let session = store.get_session(session_id)?;
    if !should_update_baseline(session.word_count, session.duration_ms, session.paused_ms) {
        return Ok(());
    }
    // Gated above, so these are safe to unwrap: `should_update_baseline`
    // only returns true when both are present and word_count > 0.
    let word_count = session.word_count.unwrap();
    let duration_ms = session.duration_ms.unwrap();
    let speaking_ms = duration_ms - session.paused_ms;
    let filler_count = session.filler_count.unwrap_or(0);
    let minutes = speaking_ms as f64 / 60_000.0;
    let session_fpm = filler_count as f64 / minutes;
    let session_wpm = word_count as f64 / minutes;

    let existing = store.get_baseline()?;
    let (prior_fpm, prior_wpm, n) = match existing {
        Some(b) => (b.fillers_per_min, b.words_per_min, b.sessions_counted),
        None => (0.0, 0.0, 0),
    };
    let new_n = n + 1;
    let new_fpm = (prior_fpm * n as f64 + session_fpm) / new_n as f64;
    let new_wpm = (prior_wpm * n as f64 + session_wpm) / new_n as f64;
    store.upsert_baseline(new_fpm, new_wpm, new_n)
}

fn with_audio_exists(mut session: Session) -> Session {
    session.audio_exists = session
        .audio_path
        .as_ref()
        .map(|p| std::path::Path::new(p).exists())
        .unwrap_or(false);
    session
}

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Result<Vec<Session>, YapperError> {
    state
        .store
        .list_sessions()?
        .into_iter()
        .map(with_audio_exists)
        .map(|mut session| {
            session.segment_count = state.store.count_segments(session.id)?;
            Ok(session)
        })
        .collect()
}

/// Remove a session row whose recording the user no longer wants tracked.
/// Never touches the audio file itself.
#[tauri::command]
fn forget_session(state: State<'_, AppState>, id: i64) -> Result<(), YapperError> {
    state.store.delete_session(id)
}

#[tauri::command]
fn list_segments(
    state: State<'_, AppState>,
    session_id: i64,
) -> Result<Vec<TranscriptSegment>, YapperError> {
    state.store.list_segments(session_id)
}

#[tauri::command]
fn list_events(state: State<'_, AppState>, session_id: i64) -> Result<Vec<Event>, YapperError> {
    state.store.list_events(session_id)
}

#[tauri::command]
fn list_outline(
    state: State<'_, AppState>,
    session_id: i64,
) -> Result<Vec<OutlineRow>, YapperError> {
    state.store.list_outline(session_id)
}

/// Recap screen's one-click "that was wrong" — no-shame feedback on a fired
/// signal, recorded and otherwise ceremony-free (see spec's no-shame recap
/// rule).
#[tauri::command]
fn set_event_feedback(
    state: State<'_, AppState>,
    event_id: i64,
    feedback: String,
) -> Result<(), YapperError> {
    state.store.set_event_feedback(event_id, &feedback)
}

/// Back-compat shape for the frontend: which of the two models are already
/// downloaded. Cheap, no network — safe to poll from a non-async command.
#[derive(serde::Serialize)]
struct ModelsReady {
    stt: bool,
    llm: bool,
}

#[tauri::command]
fn models_ready(app: tauri::AppHandle) -> Result<ModelsReady, YapperError> {
    Ok(ModelsReady {
        stt: models::model_present(&app, &models::STT_MODEL),
        llm: models::model_present(&app, &models::LLM_MODEL),
    })
}

// Async: `models::ensure_model` is a blocking call (synchronous network I/O
// and, for the STT archive, synchronous extraction), so it must run on a
// blocking-friendly thread rather than stalling an async worker or, worse,
// the main thread. Downloads both models sequentially — STT first, then LLM
// — one at a time, so the frontend only ever needs to track a single
// progress bar (`model:progress`'s `model` field says which one is active).
// Each `ensure_model` call short-circuits instantly if its model is already
// present, so re-invoking this after a partial success (e.g. STT succeeded,
// LLM failed) only re-downloads what's still missing.
/// Cached story-shape retrospective for a session, if one was generated.
#[tauri::command]
fn get_retro(state: State<'_, AppState>, session_id: i64) -> Result<Option<RetroRow>, YapperError> {
    state.store.get_retro(session_id)
}

/// The most recent retro's `try_next` — the focus the setup screen carries
/// into the next take.
#[tauri::command]
fn latest_focus(state: State<'_, AppState>) -> Result<Option<String>, YapperError> {
    state.store.latest_try_next()
}

/// Generates (or returns the cached) story-shape retrospective: one local-LLM
/// pass over the whole transcript, per the Moth rubric (stakes / opening /
/// landing) plus one `try_next` experiment. Runs on a blocking thread —
/// model load takes seconds and must never freeze the webview. The row is
/// only written on success, so a failed pass is retriable on the next
/// recap open.
#[tauri::command]
async fn generate_retro(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: i64,
) -> Result<RetroRow, YapperError> {
    if let Some(existing) = state.store.get_retro(session_id)? {
        return Ok(existing);
    }

    let store = state.store.clone();
    let segments = store.list_segments(session_id)?;
    if segments.is_empty() {
        return Err(YapperError::State("no transcript to look back on".into()));
    }
    let intent = store
        .list_sessions()?
        .into_iter()
        .find(|s| s.id == session_id)
        .map(|s| s.intent)
        .unwrap_or_default();
    let lines: Vec<(i64, String)> = segments.into_iter().map(|s| (s.start_ms, s.text)).collect();

    let model_dir = models::model_dir_for(&app, &models::LLM_MODEL)?;
    if !models::model_present(&app, &models::LLM_MODEL) {
        return Err(YapperError::State("thinking model not downloaded".into()));
    }

    let created_at = now_ms();
    let retro = tauri::async_runtime::spawn_blocking(move || -> Result<RetroRow, YapperError> {
        let mut engine = insight::llama::LlamaEngine::new(&model_dir)?;
        let prompt = insight::prompt::build_retro_prompt(&intent, &lines);
        let raw = engine.insight(&prompt)?;
        let fields = insight::prompt::parse_retro(&raw)
            .ok_or_else(|| YapperError::State("retro output unparseable".into()))?;
        Ok(RetroRow {
            session_id,
            stakes: fields.stakes,
            opening: fields.opening,
            landing: fields.landing,
            try_next: fields.try_next,
        })
    })
    .await
    .map_err(|e| YapperError::State(format!("retro task panicked: {e}")))??;

    store.save_retro(&retro, created_at)?;
    Ok(retro)
}

#[tauri::command]
async fn ensure_models(app: tauri::AppHandle) -> Result<(), YapperError> {
    let stt_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        models::ensure_model(&stt_app, &models::STT_MODEL)
    })
    .await
    .map_err(|e| YapperError::State(format!("model download task panicked: {e}")))??;

    tauri::async_runtime::spawn_blocking(move || models::ensure_model(&app, &models::LLM_MODEL))
        .await
        .map_err(|e| YapperError::State(format!("model download task panicked: {e}")))??;
    Ok(())
}

#[tauri::command]
fn export_transcript(state: State<'_, AppState>, id: i64) -> Result<String, YapperError> {
    let session = state.store.get_session(id)?;
    let segments = state.store.list_segments(id)?;
    if segments.is_empty() {
        return Err(YapperError::State("no transcript for this session".into()));
    }
    let audio_path = session
        .audio_path
        .ok_or_else(|| YapperError::State("no audio file for this session".into()))?;
    let base = std::path::Path::new(&audio_path).with_extension("");
    let srt_path = base.with_extension("srt");
    std::fs::write(&srt_path, export::to_srt(&segments))?;
    std::fs::write(base.with_extension("txt"), export::to_txt(&segments))?;
    let srt_str = srt_path.to_string_lossy().into_owned();
    tauri_plugin_opener::reveal_item_in_dir(&srt_str)
        .map_err(|e| YapperError::State(format!("could not show file: {e}")))?;
    Ok(srt_str)
}

#[tauri::command]
fn reveal_session(state: State<'_, AppState>, id: i64) -> Result<(), YapperError> {
    let session = state.store.get_session(id)?;
    let path = session
        .audio_path
        .ok_or_else(|| YapperError::State("no audio file recorded for this session".into()))?;
    tauri_plugin_opener::reveal_item_in_dir(&path)
        .map_err(|e| YapperError::State(format!("could not show file: {e}")))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Dev-only: surface tauri-internal `log::error!` output (e.g. the asset
    // protocol's "not configured to allow the path" denials, which are
    // otherwise silently swallowed without a logger).
    #[cfg(debug_assertions)]
    {
        let _ = env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("error"),
        )
        .try_init();
    }

    let mut builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());
    // Self-updater (desktop only): checks the release manifest, downloads and
    // installs a newer signed build. `process` provides relaunch after install.
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
    }

    builder
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = SessionStore::open(&data_dir.join("yapper.db"))
                .expect("failed to open session store");
            app.manage(AppState {
                store: Arc::new(store),
                active: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_input_devices,
            start_session,
            pause_listening,
            resume_listening,
            session_status,
            end_session,
            list_sessions,
            reveal_session,
            forget_session,
            list_segments,
            list_events,
            list_outline,
            set_event_feedback,
            get_retro,
            latest_focus,
            generate_retro,
            models_ready,
            ensure_models,
            export_transcript
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod baseline_gate_tests {
    use super::should_update_baseline;

    #[test]
    fn no_transcript_never_updates() {
        assert!(!should_update_baseline(None, Some(300_000), 0));
    }

    #[test]
    fn zero_word_count_never_updates() {
        // STT ran but nobody spoke: word_count is Some(0), not None. Must
        // still be rejected or a silent session drags words_per_min to 0.
        assert!(!should_update_baseline(Some(0), Some(300_000), 0));
    }

    #[test]
    fn enough_speaking_time_updates() {
        // 300 words over a 2-minute session with no pauses: exactly at the
        // MIN_SPEAKING_MS_FOR_BASELINE floor.
        assert!(should_update_baseline(Some(300), Some(120_000), 0));
    }

    #[test]
    fn too_little_speaking_time_never_updates() {
        // 90s of actual speaking (under the 2-minute floor) never updates,
        // even with a healthy transcript.
        assert!(!should_update_baseline(Some(300), Some(90_000), 0));
    }

    #[test]
    fn paused_time_is_excluded_from_speaking_time() {
        // 3 minutes of wall-clock duration but 100s paused leaves only 80s
        // of actual speaking — under the floor.
        assert!(!should_update_baseline(Some(300), Some(180_000), 100_000));
    }

    #[test]
    fn no_duration_never_updates() {
        assert!(!should_update_baseline(Some(300), None, 0));
    }
}
