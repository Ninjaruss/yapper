pub mod audio;
pub mod error;
pub mod session;
pub mod store;

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{Emitter, Manager, State};

use audio::capture::Capture;
use error::YapperError;
use session::{ClockState, SessionClock};
use store::{Session, SessionStore};

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

struct ActiveSession {
    id: i64,
    clock: SessionClock,
    capture: Capture,
}

struct AppState {
    store: SessionStore,
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
    let mut active = state.active.lock().map_err(|_| YapperError::State("state lock poisoned".into()))?;
    if active.is_some() {
        return Err(YapperError::State("a session is already running".into()));
    }
    let started = now_ms();
    let id = state.store.create_session(started, &intent)?;

    let setup_result = (|| -> Result<Capture, YapperError> {
        let audio_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| YapperError::Audio(e.to_string()))?
            .join("audio");
        std::fs::create_dir_all(&audio_dir)?;
        let wav_path = audio_dir.join(format!("session-{id}.wav"));
        Capture::start(device_name.as_deref(), wav_path)
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

    *active = Some(ActiveSession { id, clock: SessionClock::start(started), capture });
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
        writer_failed: s.capture.writer_failed.load(std::sync::atomic::Ordering::Relaxed),
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
    let final_path = stop_result.as_ref().unwrap_or(&wav_path);
    state.store.end_session(
        s.id,
        totals.ended_at_ms,
        final_path.to_string_lossy().as_ref(),
        totals.paused_ms,
    )?;
    stop_result?;
    state.store.get_session(s.id)
}

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Result<Vec<Session>, YapperError> {
    state.store.list_sessions()
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
            app.manage(AppState { store, active: Mutex::new(None) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_input_devices,
            start_session,
            pause_listening,
            resume_listening,
            session_status,
            end_session,
            list_sessions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
