# Yapper Plan 1: Recorder Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A working Tauri v2 desktop app that records a mic session to WAV with live level metering, pause/resume, session intent, and a SQLite session history — the skeleton every later Yapper phase hangs off.

**Architecture:** Rust core owns audio (cpal → incremental WAV via hound) and persistence (rusqlite); a pure state machine tracks session lifecycle; the vanilla-TS webview UI (Vite) renders Setup and Live screens in the Candlelit Study theme and talks to Rust only via Tauri commands + events.

**Tech Stack:** Tauri 2.x, Rust (cpal, hound, rusqlite bundled, serde, thiserror), Vite + vanilla TypeScript, no frontend framework.

**Spec:** `docs/superpowers/specs/2026-07-23-yapper-design.md`. This plan implements the audio-capture + session-store units and the UI shell. STT, analysis, LLM, wisp animation, recap are Plans 2–5.

**Phase roadmap (written just-in-time, one plan per phase):**
1. **This plan** — recorder skeleton (working local session recorder)
2. Ears — `TranscribeEngine` trait, Moonshine streaming, transcript + .srt export
3. Fast lane + Wisp — rhythm/baseline/repetition, animated wisp, margin notes
4. Slow lane — llama.cpp `InsightEngine`: outline, questions, wrap-up, recap
5. Polish — history/trends, model auto-download, opus compression, hardening

---

## File structure this plan creates

```
yapper/
├── package.json / vite.config.ts / tsconfig.json   (Vite vanilla-ts scaffold)
├── index.html
├── src/                        # webview UI
│   ├── main.ts                 # screen router + Tauri wiring
│   ├── screens/setup.ts        # mic picker, level meter, intent field, start
│   ├── screens/live.ts         # elapsed, pause/resume, end
│   ├── ipc.ts                  # typed wrappers for commands/events
│   └── styles.css              # Candlelit Study tokens
└── src-tauri/
    ├── tauri.conf.json
    ├── Cargo.toml
    └── src/
        ├── main.rs             # entry
        ├── lib.rs              # tauri builder, command registration, AppState
        ├── error.rs            # YapperError
        ├── store/mod.rs        # SessionStore (SQLite)
        ├── audio/mod.rs        # device listing, rms
        ├── audio/capture.rs    # Capture: cpal stream → WAV + level events
        └── session/mod.rs      # SessionClock state machine (pure, no I/O)
```

---

### Task 1: Scaffold Tauri v2 app into the existing repo

**Files:**
- Create: entire scaffold (see structure above) at repo root `/Users/ninjaruss/Documents/GitHub/yapper`

- [ ] **Step 1: Generate scaffold in a sibling temp dir**

```bash
cd /Users/ninjaruss/Documents/GitHub
npm create tauri-app@latest yapper-scaffold -- --template vanilla-ts --manager npm --yes
```

- [ ] **Step 2: Move scaffold contents into the repo (docs/ and .git/ already exist there)**

```bash
rsync -a --exclude .git yapper-scaffold/ yapper/
rm -rf yapper-scaffold
cd yapper && npm install
```

- [ ] **Step 3: Update identity fields**

In `src-tauri/tauri.conf.json` set:
```json
{
  "productName": "Yapper",
  "identifier": "net.ninjaruss.yapper",
  "app": { "windows": [{ "title": "Yapper", "width": 1100, "height": 720 }] }
}
```
(keep other generated fields as scaffolded)

- [ ] **Step 4: Verify dev build launches**

Run: `npm run tauri dev` — expect the template window to open. Close it.

- [ ] **Step 5: Extend .gitignore and commit**

Append to `.gitignore`:
```
node_modules/
dist/
src-tauri/target/
```

```bash
git add -A
git commit -m "chore: scaffold Tauri v2 app (vanilla-ts template)"
```

---

### Task 2: Rust deps, error type, and module skeleton

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/error.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add dependencies to `src-tauri/Cargo.toml`**

```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-opener = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
rusqlite = { version = "0.32", features = ["bundled"] }
cpal = "0.15"
hound = "3.5"
crossbeam-channel = "0.5"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create `src-tauri/src/error.rs`**

```rust
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum YapperError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("audio error: {0}")]
    Audio(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid state: {0}")]
    State(String),
}

// Tauri commands need serializable errors; the UI only shows the message.
impl Serialize for YapperError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
```

- [ ] **Step 3: Declare modules in `src-tauri/src/lib.rs`** (replace generated contents; commands arrive in later tasks)

```rust
pub mod audio;
pub mod error;
pub mod session;
pub mod store;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Create empty placeholder modules so it compiles: `src-tauri/src/audio/mod.rs`, `src-tauri/src/session/mod.rs`, `src-tauri/src/store/mod.rs`, each containing only a doc comment for now, e.g. `//! Session persistence (filled in by later tasks).`

- [ ] **Step 4: Verify it compiles**

Run: `cd src-tauri && cargo check`
Expected: success, no warnings about missing modules.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: add core Rust deps, error type, module skeleton"
```

---

### Task 3: SessionStore (SQLite)

**Files:**
- Create: `src-tauri/src/store/mod.rs` (replace placeholder)
- Test: inline `#[cfg(test)]` module in the same file

- [ ] **Step 1: Write the failing tests** (bottom of `src-tauri/src/store/mod.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn open_test_store() -> SessionStore {
        SessionStore::open_in_memory().unwrap()
    }

    #[test]
    fn migrations_are_idempotent() {
        let store = open_test_store();
        store.migrate().unwrap(); // second run must not error
    }

    #[test]
    fn create_and_fetch_session() {
        let store = open_test_store();
        let id = store.create_session(1_700_000_000_000, "why i left").unwrap();
        let s = store.get_session(id).unwrap();
        assert_eq!(s.intent, "why i left");
        assert_eq!(s.started_at_ms, 1_700_000_000_000);
        assert!(s.ended_at_ms.is_none());
    }

    #[test]
    fn end_session_records_duration_audio_and_pause() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store
            .end_session(id, 61_000, "/tmp/a.wav", 5_000)
            .unwrap();
        let s = store.get_session(id).unwrap();
        assert_eq!(s.ended_at_ms, Some(61_000));
        assert_eq!(s.duration_ms, Some(60_000));
        assert_eq!(s.paused_ms, 5_000);
        assert_eq!(s.audio_path.as_deref(), Some("/tmp/a.wav"));
    }

    #[test]
    fn list_sessions_newest_first() {
        let store = open_test_store();
        store.create_session(1000, "a").unwrap();
        store.create_session(2000, "b").unwrap();
        let all = store.list_sessions().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].intent, "b");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test store::`
Expected: compile FAILURE — `SessionStore` not defined.

- [ ] **Step 3: Implement `SessionStore`** (top of the same file)

```rust
//! Session persistence. One SQLite DB in the app data dir; all local.

use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

use crate::error::YapperError;

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: i64,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub intent: String,
    pub audio_path: Option<String>,
    pub duration_ms: Option<i64>,
    pub paused_ms: i64,
}

pub struct SessionStore {
    conn: Mutex<Connection>,
}

impl SessionStore {
    pub fn open(path: &Path) -> Result<Self, YapperError> {
        let store = Self { conn: Mutex::new(Connection::open(path)?) };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, YapperError> {
        let store = Self { conn: Mutex::new(Connection::open_in_memory()?) };
        store.migrate()?;
        Ok(store)
    }

    /// Versioned migrations via PRAGMA user_version. Append-only: never edit
    /// an existing migration, add a new numbered one.
    pub fn migrate(&self) -> Result<(), YapperError> {
        let conn = self.conn.lock().unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS sessions (
                    id INTEGER PRIMARY KEY,
                    started_at_ms INTEGER NOT NULL,
                    ended_at_ms INTEGER,
                    intent TEXT NOT NULL DEFAULT '',
                    audio_path TEXT,
                    duration_ms INTEGER,
                    paused_ms INTEGER NOT NULL DEFAULT 0
                );
                PRAGMA user_version = 1;",
            )?;
        }
        Ok(())
    }

    pub fn create_session(&self, started_at_ms: i64, intent: &str) -> Result<i64, YapperError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (started_at_ms, intent) VALUES (?1, ?2)",
            params![started_at_ms, intent],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn end_session(
        &self,
        id: i64,
        ended_at_ms: i64,
        audio_path: &str,
        paused_ms: i64,
    ) -> Result<(), YapperError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions
             SET ended_at_ms = ?2,
                 duration_ms = ?2 - started_at_ms,
                 audio_path = ?3,
                 paused_ms = ?4
             WHERE id = ?1",
            params![id, ended_at_ms, audio_path, paused_ms],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: i64) -> Result<Session, YapperError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT id, started_at_ms, ended_at_ms, intent, audio_path, duration_ms, paused_ms
             FROM sessions WHERE id = ?1",
            params![id],
            Self::row_to_session,
        )?)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, YapperError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, started_at_ms, ended_at_ms, intent, audio_path, duration_ms, paused_ms
             FROM sessions ORDER BY started_at_ms DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_session)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        Ok(Session {
            id: row.get(0)?,
            started_at_ms: row.get(1)?,
            ended_at_ms: row.get(2)?,
            intent: row.get(3)?,
            audio_path: row.get(4)?,
            duration_ms: row.get(5)?,
            paused_ms: row.get(6)?,
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test store::`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/store/mod.rs
git commit -m "feat: SQLite session store with versioned migrations"
```

---

### Task 4: SessionClock state machine (pure, no I/O)

Tracks Idle → Recording ⇄ Paused → Ended and accounts paused time — the pause hotkey and the elapsed display both depend on it. Pure logic; caller supplies timestamps (deterministic tests, no clocks).

**Files:**
- Create: `src-tauri/src/session/mod.rs` (replace placeholder)
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_recording() {
        let c = SessionClock::start(1000);
        assert_eq!(c.state(), ClockState::Recording);
        assert_eq!(c.elapsed_ms(3000), 2000);
        assert_eq!(c.paused_ms(3000), 0);
    }

    #[test]
    fn pause_freezes_elapsed_and_accumulates_paused() {
        let mut c = SessionClock::start(1000);
        c.pause(2000).unwrap();
        assert_eq!(c.state(), ClockState::Paused);
        assert_eq!(c.elapsed_ms(5000), 1000); // frozen at pause point
        assert_eq!(c.paused_ms(5000), 3000);
        c.resume(6000).unwrap();
        assert_eq!(c.elapsed_ms(8000), 3000); // 1000 before + 2000 after
        assert_eq!(c.paused_ms(8000), 4000);
    }

    #[test]
    fn double_pause_and_resume_while_recording_are_errors() {
        let mut c = SessionClock::start(0);
        assert!(c.resume(10).is_err());
        c.pause(10).unwrap();
        assert!(c.pause(20).is_err());
    }

    #[test]
    fn end_returns_totals_and_ends_from_paused_too() {
        let mut c = SessionClock::start(1000);
        c.pause(2000).unwrap();
        let totals = c.end(4000);
        assert_eq!(totals.ended_at_ms, 4000);
        assert_eq!(totals.paused_ms, 2000);
        assert_eq!(c.state(), ClockState::Ended);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test session::`
Expected: compile FAILURE — `SessionClock` not defined.

- [ ] **Step 3: Implement**

```rust
//! Pure session lifecycle clock. No I/O, no system time — callers pass
//! timestamps in, which keeps every path deterministic under test.

use crate::error::YapperError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockState {
    Recording,
    Paused,
    Ended,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionTotals {
    pub ended_at_ms: i64,
    pub paused_ms: i64,
}

pub struct SessionClock {
    started_at_ms: i64,
    state: ClockState,
    paused_accum_ms: i64,
    paused_since_ms: Option<i64>,
}

impl SessionClock {
    pub fn start(now_ms: i64) -> Self {
        Self {
            started_at_ms: now_ms,
            state: ClockState::Recording,
            paused_accum_ms: 0,
            paused_since_ms: None,
        }
    }

    pub fn state(&self) -> ClockState {
        self.state
    }

    pub fn pause(&mut self, now_ms: i64) -> Result<(), YapperError> {
        if self.state != ClockState::Recording {
            return Err(YapperError::State("can only pause while recording".into()));
        }
        self.state = ClockState::Paused;
        self.paused_since_ms = Some(now_ms);
        Ok(())
    }

    pub fn resume(&mut self, now_ms: i64) -> Result<(), YapperError> {
        if self.state != ClockState::Paused {
            return Err(YapperError::State("can only resume while paused".into()));
        }
        self.paused_accum_ms += now_ms - self.paused_since_ms.take().unwrap();
        self.state = ClockState::Recording;
        Ok(())
    }

    /// Speaking time: wall time minus everything spent paused.
    pub fn elapsed_ms(&self, now_ms: i64) -> i64 {
        now_ms - self.started_at_ms - self.paused_ms(now_ms)
    }

    pub fn paused_ms(&self, now_ms: i64) -> i64 {
        match (self.state, self.paused_since_ms) {
            (ClockState::Paused, Some(since)) => self.paused_accum_ms + (now_ms - since),
            _ => self.paused_accum_ms,
        }
    }

    pub fn end(&mut self, now_ms: i64) -> SessionTotals {
        let paused_ms = self.paused_ms(now_ms);
        self.state = ClockState::Ended;
        self.paused_since_ms = None;
        SessionTotals { ended_at_ms: now_ms, paused_ms }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test session::`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/session/mod.rs
git commit -m "feat: pure session clock state machine with pause accounting"
```

---

### Task 5: Audio device listing + RMS level

**Files:**
- Create: `src-tauri/src/audio/mod.rs` (replace placeholder)
- Test: inline `#[cfg(test)]` for `rms_level` (device listing needs hardware; keep it thin and untested)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_silence_is_zero_and_full_scale_is_one() {
        assert_eq!(rms_level(&[0.0; 480]), 0.0);
        let full: Vec<f32> = vec![1.0; 480];
        assert!((rms_level(&full) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rms_of_half_scale_sine_is_about_0_35() {
        let sine: Vec<f32> = (0..480)
            .map(|i| 0.5 * (i as f32 / 480.0 * std::f32::consts::TAU * 10.0).sin())
            .collect();
        let r = rms_level(&sine);
        assert!(r > 0.3 && r < 0.4, "got {r}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test audio::`
Expected: compile FAILURE — `rms_level` not defined.

- [ ] **Step 3: Implement**

```rust
//! Input device enumeration and level math.

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

use crate::error::YapperError;

#[derive(Debug, Clone, Serialize)]
pub struct InputDevice {
    pub name: String,
    pub is_default: bool,
}

pub fn list_input_devices() -> Result<Vec<InputDevice>, YapperError> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok());
    let devices = host
        .input_devices()
        .map_err(|e| YapperError::Audio(e.to_string()))?;
    Ok(devices
        .filter_map(|d| d.name().ok())
        .map(|name| InputDevice {
            is_default: Some(&name) == default_name.as_ref(),
            name,
        })
        .collect())
}

/// Root-mean-square of a mono f32 buffer, 0.0..=1.0 for full-scale input.
pub fn rms_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

pub mod capture;
```

Also create an empty `src-tauri/src/audio/capture.rs` containing `//! Filled in by Task 6.` so the module declaration compiles.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test audio::`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/audio/
git commit -m "feat: input device listing and RMS level math"
```

---

### Task 6: Capture — cpal stream → incremental WAV + level channel

Design: the cpal callback downmixes to mono and sends buffers over a crossbeam channel; a writer thread appends to the WAV (crash-safe: hound finalizes headers on `finalize()`, and we flush periodically) and publishes an RMS level every ~100 ms. Pausing flips an `AtomicBool` — the stream keeps running (device stays warm) but paused buffers are dropped before the channel.

**Files:**
- Create: `src-tauri/src/audio/capture.rs` (replace placeholder)
- Test: inline `#[cfg(test)]` for the writer path using a synthetic sample feed (no hardware in tests)

- [ ] **Step 1: Write the failing test** (tests the writer thread contract, not cpal)

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test audio::capture`
Expected: compile FAILURE — `spawn_writer` / `gate_and_downmix` not defined.

- [ ] **Step 3: Implement `src-tauri/src/audio/capture.rs`**

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test audio::capture`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/audio/capture.rs
git commit -m "feat: cpal capture to incremental WAV with pause gate and level channel"
```

---

### Task 7: Tauri commands + AppState wiring

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Implement AppState and commands** (replace `lib.rs`)

```rust
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

#[tauri::command]
fn start_session(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    intent: String,
    device_name: Option<String>,
) -> Result<i64, YapperError> {
    let mut active = state.active.lock().unwrap();
    if active.is_some() {
        return Err(YapperError::State("a session is already running".into()));
    }
    let started = now_ms();
    let id = state.store.create_session(started, &intent)?;

    let audio_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| YapperError::Audio(e.to_string()))?
        .join("audio");
    std::fs::create_dir_all(&audio_dir)?;
    let wav_path = audio_dir.join(format!("session-{id}.wav"));

    let capture = Capture::start(device_name.as_deref(), wav_path)?;

    // Forward levels to the UI (~unbounded channel drained on a timer thread).
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
    let mut active = state.active.lock().unwrap();
    let s = active.as_mut().ok_or_else(|| YapperError::State("no session".into()))?;
    s.clock.pause(now_ms())?;
    s.capture.paused.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn resume_listening(state: State<'_, AppState>) -> Result<(), YapperError> {
    let mut active = state.active.lock().unwrap();
    let s = active.as_mut().ok_or_else(|| YapperError::State("no session".into()))?;
    s.clock.resume(now_ms())?;
    s.capture.paused.store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn session_status(state: State<'_, AppState>) -> Result<Option<(i64, String, i64)>, YapperError> {
    let active = state.active.lock().unwrap();
    Ok(active.as_ref().map(|s| {
        let st = match s.clock.state() {
            ClockState::Recording => "recording",
            ClockState::Paused => "paused",
            ClockState::Ended => "ended",
        };
        (s.id, st.to_string(), s.clock.elapsed_ms(now_ms()))
    }))
}

#[tauri::command]
fn end_session(state: State<'_, AppState>) -> Result<Session, YapperError> {
    let mut active = state.active.lock().unwrap();
    let mut s = active.take().ok_or_else(|| YapperError::State("no session".into()))?;
    let totals = s.clock.end(now_ms());
    let wav_path = s.capture.stop()?;
    state.store.end_session(
        s.id,
        totals.ended_at_ms,
        wav_path.to_string_lossy().as_ref(),
        totals.paused_ms,
    )?;
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
```

- [ ] **Step 2: Verify compile + existing tests still pass**

Run: `cd src-tauri && cargo test`
Expected: all prior tests pass; `cargo check` clean.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: session lifecycle Tauri commands wired to store and capture"
```

---

### Task 8: UI — typed IPC + Candlelit tokens

**Files:**
- Create: `src/ipc.ts`
- Create: `src/styles.css` (replace scaffold css)
- Delete: scaffold demo files (`src/main.ts` gets replaced in Task 9; remove template assets)

- [ ] **Step 1: Create `src/ipc.ts`**

```typescript
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface InputDevice { name: string; is_default: boolean; }
export interface Session {
  id: number;
  started_at_ms: number;
  ended_at_ms: number | null;
  intent: string;
  audio_path: string | null;
  duration_ms: number | null;
  paused_ms: number;
}

export const ipc = {
  listInputDevices: () => invoke<InputDevice[]>("list_input_devices"),
  startSession: (intent: string, deviceName?: string) =>
    invoke<number>("start_session", { intent, deviceName: deviceName ?? null }),
  pauseListening: () => invoke<void>("pause_listening"),
  resumeListening: () => invoke<void>("resume_listening"),
  sessionStatus: () =>
    invoke<[number, string, number] | null>("session_status"),
  endSession: () => invoke<Session>("end_session"),
  listSessions: () => invoke<Session[]>("list_sessions"),
  onLevel: (cb: (level: number) => void): Promise<UnlistenFn> =>
    listen<number>("audio:level", (e) => cb(e.payload)),
};
```

- [ ] **Step 2: Create `src/styles.css`** (Candlelit Study tokens; screens style against tokens only)

```css
:root {
  /* Candlelit Study — spec: UI section */
  --desk: #2b2114;
  --desk-glow: #4a3a24;
  --paper: #f2e6c8;
  --paper-deep: #e6d5ac;
  --ink: #4a3c26;
  --ink-soft: #8a7248;
  --gold: #d9a92e;
  --gold-bright: #ffe52c;
  --ember: #e8912c;
  --serif: Georgia, "Iowan Old Style", serif;
  --mono: ui-monospace, Menlo, monospace;
}

* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: var(--serif);
  color: var(--paper);
  background: radial-gradient(ellipse at 68% 55%, var(--desk-glow), var(--desk) 55%, #1a140c);
  min-height: 100vh;
}

.screen { max-width: 900px; margin: 0 auto; padding: 32px 20px; }

.paper-panel {
  background: linear-gradient(160deg, var(--paper), var(--paper-deep));
  color: var(--ink);
  border-radius: 3px;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.55);
  padding: 18px 22px;
}

.label {
  font-family: var(--mono);
  font-size: 0.65rem;
  letter-spacing: 0.2em;
  text-transform: uppercase;
  color: var(--ink-soft);
}

button {
  font-family: var(--serif);
  font-size: 1rem;
  background: var(--gold);
  color: #2a2216;
  border: none;
  border-radius: 3px;
  padding: 10px 22px;
  cursor: pointer;
}
button:hover, button:focus-visible { background: var(--gold-bright); }
button.quiet { background: transparent; color: var(--paper); border: 1px solid var(--ink-soft); }

select, textarea {
  font-family: var(--serif);
  width: 100%;
  background: var(--paper);
  color: var(--ink);
  border: 1px solid var(--ink-soft);
  border-radius: 3px;
  padding: 8px 10px;
}

.level-meter {
  height: 10px;
  background: rgba(0, 0, 0, 0.4);
  border-radius: 5px;
  overflow: hidden;
}
.level-meter > div {
  height: 100%;
  width: 0%;
  background: linear-gradient(90deg, var(--gold), var(--ember));
  transition: width 80ms linear;
}

.elapsed { font-family: var(--mono); font-size: 2.4rem; color: var(--gold-bright); }
.paused-note { color: var(--ember); font-style: italic; }
```

- [ ] **Step 3: Verify Vite builds**

Run: `npm run build`
Expected: success (main.ts still the scaffold one at this point — that's fine).

- [ ] **Step 4: Commit**

```bash
git add src/ipc.ts src/styles.css
git commit -m "feat: typed IPC layer and Candlelit Study theme tokens"
```

---

### Task 9: UI — Setup and Live screens

**Files:**
- Create: `src/screens/setup.ts`, `src/screens/live.ts`
- Modify: `src/main.ts` (replace scaffold), `index.html`

- [ ] **Step 1: `index.html`** (body only; keep scaffold head, point at styles.css + main.ts)

```html
<body>
  <div id="app" class="screen"></div>
  <script type="module" src="/src/main.ts"></script>
</body>
```

- [ ] **Step 2: `src/screens/setup.ts`**

```typescript
import { ipc, type InputDevice } from "../ipc";

export function renderSetup(
  root: HTMLElement,
  onStarted: () => void,
): void {
  root.innerHTML = `
    <h1>Yapper</h1>
    <div class="paper-panel">
      <div class="label">Microphone</div>
      <select id="mic"></select>
      <div class="level-meter" style="margin-top:10px"><div id="meter"></div></div>
      <div class="label" style="margin-top:18px">Intent — a title, or paste your whole notes</div>
      <textarea id="intent" rows="4" placeholder="what do you want to talk about?"></textarea>
      <div style="margin-top:16px; display:flex; gap:10px;">
        <button id="start">Begin the talk</button>
      </div>
      <p id="error" class="paused-note" role="alert"></p>
    </div>
  `;

  const mic = root.querySelector<HTMLSelectElement>("#mic")!;
  const errorEl = root.querySelector<HTMLParagraphElement>("#error")!;

  ipc.listInputDevices().then((devices: InputDevice[]) => {
    mic.innerHTML = devices
      .map((d) => `<option ${d.is_default ? "selected" : ""}>${d.name}</option>`)
      .join("");
  }).catch((e) => { errorEl.textContent = String(e); });

  root.querySelector<HTMLButtonElement>("#start")!.onclick = async () => {
    const intent = root.querySelector<HTMLTextAreaElement>("#intent")!.value;
    try {
      await ipc.startSession(intent, mic.value || undefined);
      onStarted();
    } catch (e) {
      errorEl.textContent = String(e);
    }
  };
}
```

- [ ] **Step 3: `src/screens/live.ts`**

```typescript
import { ipc } from "../ipc";

export function renderLive(root: HTMLElement, onEnded: () => void): void {
  root.innerHTML = `
    <div class="paper-panel" style="display:flex; align-items:center; gap:24px;">
      <div class="elapsed" id="elapsed">0:00</div>
      <div class="level-meter" style="flex:1"><div id="meter"></div></div>
      <button id="pause" class="quiet">Pause listening</button>
      <button id="end">End the talk</button>
    </div>
    <p id="state" class="paused-note"></p>
  `;

  const elapsedEl = root.querySelector<HTMLElement>("#elapsed")!;
  const meterEl = root.querySelector<HTMLElement>("#meter")!;
  const stateEl = root.querySelector<HTMLElement>("#state")!;
  const pauseBtn = root.querySelector<HTMLButtonElement>("#pause")!;

  let paused = false;
  let unlisten: (() => void) | null = null;
  ipc.onLevel((level) => {
    meterEl.style.width = `${Math.min(100, level * 300)}%`;
  }).then((fn) => (unlisten = fn));

  const timer = setInterval(async () => {
    const status = await ipc.sessionStatus();
    if (!status) return;
    const [, , elapsedMs] = status;
    const total = Math.floor(elapsedMs / 1000);
    elapsedEl.textContent = `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
  }, 500);

  pauseBtn.onclick = async () => {
    if (paused) {
      await ipc.resumeListening();
      pauseBtn.textContent = "Pause listening";
      stateEl.textContent = "";
    } else {
      await ipc.pauseListening();
      pauseBtn.textContent = "Resume";
      stateEl.textContent = "asleep — hearing nothing";
    }
    paused = !paused;
  };

  root.querySelector<HTMLButtonElement>("#end")!.onclick = async () => {
    clearInterval(timer);
    unlisten?.();
    await ipc.endSession();
    onEnded();
  };
}
```

- [ ] **Step 4: `src/main.ts`** (replace scaffold)

```typescript
import "./styles.css";
import { renderSetup } from "./screens/setup";
import { renderLive } from "./screens/live";
import { ipc } from "./ipc";

const root = document.getElementById("app")!;

function showSetup() {
  renderSetup(root, showLive);
}

async function showLive() {
  renderLive(root, async () => {
    // Minimal end-of-take acknowledgment; real recap arrives in Plan 4.
    const sessions = await ipc.listSessions();
    const last = sessions[0];
    const mins = last?.duration_ms ? Math.round(last.duration_ms / 60000) : 0;
    root.innerHTML = `
      <div class="paper-panel">
        <div class="label">Talk saved</div>
        <p>~${mins} min · audio at <code>${last?.audio_path ?? "?"}</code></p>
        <button id="again">Back to the desk</button>
      </div>`;
    root.querySelector<HTMLButtonElement>("#again")!.onclick = showSetup;
  });
}

showSetup();
```

- [ ] **Step 5: Manual verification (hardware path — no automated test)**

Run: `npm run tauri dev`
Checklist: mic list populates with your default selected → start → level meter moves when you speak → elapsed counts and freezes while paused → resume continues → end shows saved panel → the WAV at the shown path opens and plays your voice → relaunching app and calling start again creates session 2.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: setup and live screens wired to session lifecycle"
```

---

### Task 10: End-to-end sanity + tag

- [ ] **Step 1: Full test suite**

Run: `cd src-tauri && cargo test && cd .. && npm run build`
Expected: all Rust tests pass; Vite build clean.

- [ ] **Step 2: Record a real 2-minute test session on the Mac** (the golden fixture for Plan 2's STT work — speak naturally, include a deliberate pause and some filler words)

Keep the WAV: copy it to `docs/superpowers/fixtures/first-session.wav` is **not** committed (gitignore `docs/superpowers/fixtures/`), it stays local.

```bash
mkdir -p docs/superpowers/fixtures
echo "docs/superpowers/fixtures/" >> .gitignore
```

- [ ] **Step 3: Commit + tag the phase**

```bash
git add .gitignore
git commit -m "chore: local fixtures dir for golden recordings"
git tag plan1-recorder-skeleton
```

---

## Self-review notes

- **Spec coverage (this phase's slice):** audio capture unit ✓ (Task 6), session store ✓ (Task 3), pause hotkey groundwork ✓ (UI button now; global hotkey lands with the wisp phase), intent field ✓ (Task 9), incremental/crash-safe audio ✓ (flush per buffer, Task 6), zero-setup ✓ (no config needed to reach first recording). Deliberately absent per roadmap: STT, analysis, LLM, wisp, recap, exports, model download.
- **Type consistency:** `Session` fields match store schema and `ipc.ts` interface; `SessionClock` API used in Task 7 matches Task 4 definitions; `spawn_writer`/`gate_and_downmix` signatures match tests.
- **Known risk flagged for executor:** cpal `default_input_config` may be f32 or i16 on some Linux devices; if `build_input_stream` errors on SteamOS, match on `config.sample_format()` and add an `I16` branch converting to f32 before `gate_and_downmix`. Don't silently resample — record at native rate; Plan 2 owns the 16 kHz resample for STT.
