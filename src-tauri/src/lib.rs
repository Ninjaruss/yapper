pub mod audio;
pub mod error;
pub mod export;
pub mod models;
pub mod session;
pub mod store;
pub mod stt;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{Emitter, Manager, State};

use audio::capture::Capture;
use error::YapperError;
use session::{ClockState, SessionClock};
use store::{Session, SessionStore, TranscriptSegment};
use stt::{Segment, TranscribeEngine};

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

struct ActiveSession {
    id: i64,
    clock: SessionClock,
    capture: Capture,
    stt_worker: Option<JoinHandle<()>>,
    stt_failed: Arc<AtomicBool>,
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
        let active = state.active.lock().map_err(|_| YapperError::State("state lock poisoned".into()))?;
        if active.is_some() {
            return Err(YapperError::State("a session is already running".into()));
        }
    }

    // If the Moonshine model is present, load it off the async worker via
    // `spawn_blocking` (model init does blocking file I/O) *before* touching
    // any session state. If models are missing or init fails, recording must
    // still proceed without STT (zero-setup / mirror-never-stops principle).
    let model_dir = models::model_dir(&app).ok();
    let engine: Option<Box<dyn TranscribeEngine>> = match model_dir {
        Some(dir) if models::models_present(&dir) => {
            match tauri::async_runtime::spawn_blocking(move || stt::moonshine::MoonshineEngine::new(&dir))
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

    // Second check: another session may have started while we were awaiting
    // model init above. Re-acquire the lock and re-check before touching any
    // session state; if one snuck in, bail out (the just-loaded `engine`, if
    // any, is simply dropped here).
    let mut active = state.active.lock().map_err(|_| YapperError::State("state lock poisoned".into()))?;
    if active.is_some() {
        return Err(YapperError::State("a session is already running".into()));
    }

    let started = now_ms();
    let id = state.store.create_session(started, &intent)?;

    let tee = engine.is_some().then(|| crossbeam_channel::bounded::<Vec<f32>>(256));
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
    let stt_worker = if let (Some(engine), Some((_tee_tx, tee_rx))) = (engine, tee) {
        let (seg_tx, seg_rx) = crossbeam_channel::unbounded::<Segment>();
        // Forward transcribed segments to the UI, same pattern as the level
        // forwarder above; exits when the segment channel closes (i.e. once
        // the worker thread finishes and drops its sender).
        let emit_app = app.clone();
        std::thread::spawn(move || {
            while let Ok(seg) = seg_rx.recv() {
                let _ = emit_app.emit("transcript:segment", seg);
            }
        });
        Some(stt::worker::spawn_stt_worker(
            engine,
            capture.sample_rate,
            tee_rx,
            state.store.clone(),
            id,
            seg_tx,
            stt_failed.clone(),
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
    });
    Ok(id)
}

#[tauri::command]
fn pause_listening(state: State<'_, AppState>) -> Result<(), YapperError> {
    let mut active = state.active.lock().map_err(|_| YapperError::State("state lock poisoned".into()))?;
    let s = active.as_mut().ok_or_else(|| YapperError::State("no session".into()))?;
    s.clock.pause(now_ms())?;
    s.capture.paused.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn resume_listening(state: State<'_, AppState>) -> Result<(), YapperError> {
    let mut active = state.active.lock().map_err(|_| YapperError::State("state lock poisoned".into()))?;
    let s = active.as_mut().ok_or_else(|| YapperError::State("no session".into()))?;
    s.clock.resume(now_ms())?;
    s.capture.paused.store(false, std::sync::atomic::Ordering::Relaxed);
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
}

#[tauri::command]
fn session_status(state: State<'_, AppState>) -> Result<Option<SessionStatus>, YapperError> {
    let active = state.active.lock().map_err(|_| YapperError::State("state lock poisoned".into()))?;
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
    }))
}

// Async for the same main-thread reason as start_session — see comment there.
#[tauri::command]
async fn end_session(state: State<'_, AppState>) -> Result<Session, YapperError> {
    let mut active = state.active.lock().map_err(|_| YapperError::State("state lock poisoned".into()))?;
    let mut s = active.take().ok_or_else(|| YapperError::State("no session".into()))?;
    let totals = s.clock.end(now_ms());
    // Capture the known WAV path before stop() consumes self, so the DB
    // record can still be written even if stop() itself errors out — the
    // WAV is flushed per buffer, so the file is still largely recoverable.
    let wav_path = s.capture.wav_path.clone();
    let stop_result = s.capture.stop();
    // Join the STT worker immediately after capture.stop(), and before the
    // fallible DB write below: stop() has already sent the tee's shutdown
    // sentinel (and disconnected the channel), so the worker flushes
    // trailing audio and exits here. Joining unconditionally — rather than
    // after a `?` that might return early — ensures the handle is never
    // silently dropped on the DB-error path.
    if let Some(handle) = s.stt_worker.take() {
        let _ = handle.join();
    }
    let final_path = stop_result.as_ref().unwrap_or(&wav_path);
    state.store.end_session(
        s.id,
        totals.ended_at_ms,
        final_path.to_string_lossy().as_ref(),
        totals.paused_ms,
    )?;
    stop_result?;
    state.store.get_session(s.id).map(with_audio_exists)
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
fn list_segments(state: State<'_, AppState>, session_id: i64) -> Result<Vec<TranscriptSegment>, YapperError> {
    state.store.list_segments(session_id)
}

#[tauri::command]
fn models_ready(app: tauri::AppHandle) -> Result<bool, YapperError> {
    Ok(models::models_present(&models::model_dir(&app)?))
}

// Async: `models::ensure_models` is a blocking call (synchronous network I/O
// and archive extraction), so it must run on a blocking-friendly thread
// rather than stalling an async worker or, worse, the main thread.
#[tauri::command]
async fn ensure_models(app: tauri::AppHandle) -> Result<(), YapperError> {
    tauri::async_runtime::spawn_blocking(move || models::ensure_models(&app))
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = SessionStore::open(&data_dir.join("yapper.db"))
                .expect("failed to open session store");
            app.manage(AppState { store: Arc::new(store), active: Mutex::new(None) });
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
            models_ready,
            ensure_models,
            export_transcript
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
