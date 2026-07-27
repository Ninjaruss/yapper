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
    pub filler_count: Option<i64>,
    pub word_count: Option<i64>,
    /// The experiment this take carried in (previous retro's try_next).
    pub focus: Option<String>,
    /// Not persisted — computed at the command layer from the filesystem so
    /// the UI can tell when a recording was deleted out from under us.
    #[serde(default)]
    pub audio_exists: bool,
    /// Not queried here — filled in by the command layer via
    /// `count_segments` so the UI can tell whether a transcript exists.
    #[serde(default)]
    pub segment_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptSegment {
    pub id: i64,
    pub session_id: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: i64,
    pub session_id: i64,
    pub at_ms: i64,
    pub kind: String,
    pub note: String,
    pub user_feedback: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Baseline {
    pub fillers_per_min: f64,
    pub words_per_min: f64,
    pub sessions_counted: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetroRow {
    pub session_id: i64,
    pub stakes: Option<String>,
    pub opening: Option<String>,
    pub landing: Option<String>,
    pub try_next: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutlineRow {
    pub id: i64,
    pub session_id: i64,
    pub label: String,
    pub status: String,
    pub updated_at_ms: i64,
}

pub struct SessionStore {
    // Mutex needed: Connection is Send but not Sync; one lock per operation.
    conn: Mutex<Connection>,
}

impl SessionStore {
    pub fn open(path: &Path) -> Result<Self, YapperError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, YapperError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Versioned migrations via PRAGMA user_version. Append-only: never edit
    /// an existing migration, add a new numbered one.
    pub fn migrate(&self) -> Result<(), YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
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
        if version < 2 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS transcript_segments (
                    id INTEGER PRIMARY KEY,
                    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    start_ms INTEGER NOT NULL,
                    end_ms INTEGER NOT NULL,
                    text TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_segments_session
                    ON transcript_segments(session_id, start_ms);
                PRAGMA user_version = 2;",
            )?;
        }
        if version < 3 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS events (
                    id INTEGER PRIMARY KEY,
                    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    at_ms INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    note TEXT NOT NULL DEFAULT '',
                    user_feedback TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id, at_ms);
                CREATE TABLE IF NOT EXISTS baselines (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    fillers_per_min REAL NOT NULL,
                    words_per_min REAL NOT NULL,
                    sessions_counted INTEGER NOT NULL
                );
                ALTER TABLE sessions ADD COLUMN filler_count INTEGER;
                ALTER TABLE sessions ADD COLUMN word_count INTEGER;
                PRAGMA user_version = 3;",
            )?;
        }
        if version < 4 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS outline_entries (
                    id INTEGER PRIMARY KEY,
                    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    label TEXT NOT NULL,
                    status TEXT NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_outline_session ON outline_entries(session_id);
                PRAGMA user_version = 4;",
            )?;
        }
        if version < 5 {
            // Story-shape retrospectives: one per session, written only on a
            // successful LLM pass so a missing row means "not generated yet".
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS retros (
                    session_id INTEGER PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                    stakes TEXT,
                    opening TEXT,
                    landing TEXT,
                    try_next TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL
                );
                PRAGMA user_version = 5;",
            )?;
        }
        if version < 6 {
            // The focus carried into a take (the previous retro's try_next
            // at start time) — lets the recap echo THIS take's experiment
            // even after newer retros exist.
            conn.execute_batch(
                "ALTER TABLE sessions ADD COLUMN focus TEXT;
                PRAGMA user_version = 6;",
            )?;
        }
        Ok(())
    }

    pub fn create_session(&self, started_at_ms: i64, intent: &str) -> Result<i64, YapperError> {
        self.create_session_with_focus(started_at_ms, intent, None)
    }

    /// `focus` is the previous retro's `try_next` captured at start time, so
    /// the recap can echo the experiment this take was actually carrying.
    pub fn create_session_with_focus(
        &self,
        started_at_ms: i64,
        intent: &str,
        focus: Option<&str>,
    ) -> Result<i64, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO sessions (started_at_ms, intent, focus) VALUES (?1, ?2, ?3)",
            params![started_at_ms, intent, focus],
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
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
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

    pub fn delete_session(&self, id: i64) -> Result<(), YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn add_segment(
        &self,
        session_id: i64,
        start_ms: i64,
        end_ms: i64,
        text: &str,
    ) -> Result<i64, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO transcript_segments (session_id, start_ms, end_ms, text) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, start_ms, end_ms, text],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn count_segments(&self, session_id: i64) -> Result<i64, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM transcript_segments WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?)
    }

    pub fn list_segments(&self, session_id: i64) -> Result<Vec<TranscriptSegment>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, session_id, start_ms, end_ms, text FROM transcript_segments
             WHERE session_id = ?1 ORDER BY start_ms",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(TranscriptSegment {
                id: row.get(0)?,
                session_id: row.get(1)?,
                start_ms: row.get(2)?,
                end_ms: row.get(3)?,
                text: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_session(&self, id: i64) -> Result<Session, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        Ok(conn.query_row(
            "SELECT id, started_at_ms, ended_at_ms, intent, audio_path, duration_ms, paused_ms, filler_count, word_count, focus
             FROM sessions WHERE id = ?1",
            params![id],
            Self::row_to_session,
        )?)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, started_at_ms, ended_at_ms, intent, audio_path, duration_ms, paused_ms, filler_count, word_count, focus
             FROM sessions ORDER BY started_at_ms DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_session)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Upserts a session's story-shape retrospective (regeneration replaces).
    pub fn save_retro(&self, retro: &RetroRow, created_at_ms: i64) -> Result<(), YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO retros (session_id, stakes, opening, landing, try_next, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id) DO UPDATE SET
                stakes = excluded.stakes, opening = excluded.opening,
                landing = excluded.landing, try_next = excluded.try_next,
                created_at_ms = excluded.created_at_ms",
            params![
                retro.session_id,
                retro.stakes,
                retro.opening,
                retro.landing,
                retro.try_next,
                created_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn get_retro(&self, session_id: i64) -> Result<Option<RetroRow>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT session_id, stakes, opening, landing, try_next FROM retros
             WHERE session_id = ?1",
        )?;
        let mut rows = stmt.query_map(params![session_id], Self::row_to_retro)?;
        Ok(rows.next().transpose()?)
    }

    /// The most recent retro's `try_next` — the focus carried into the next
    /// take's setup screen.
    pub fn latest_try_next(&self) -> Result<Option<String>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT try_next FROM retros ORDER BY created_at_ms DESC, session_id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.next().transpose()?)
    }

    fn row_to_retro(row: &rusqlite::Row) -> rusqlite::Result<RetroRow> {
        Ok(RetroRow {
            session_id: row.get(0)?,
            stakes: row.get(1)?,
            opening: row.get(2)?,
            landing: row.get(3)?,
            try_next: row.get(4)?,
        })
    }

    pub fn add_event(
        &self,
        session_id: i64,
        at_ms: i64,
        kind: &str,
        note: &str,
    ) -> Result<i64, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO events (session_id, at_ms, kind, note) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, at_ms, kind, note],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_events(&self, session_id: i64) -> Result<Vec<Event>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, session_id, at_ms, kind, note, user_feedback FROM events
             WHERE session_id = ?1 ORDER BY at_ms",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(Event {
                id: row.get(0)?,
                session_id: row.get(1)?,
                at_ms: row.get(2)?,
                kind: row.get(3)?,
                note: row.get(4)?,
                user_feedback: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_baseline(&self) -> Result<Option<Baseline>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let result = conn.query_row(
            "SELECT fillers_per_min, words_per_min, sessions_counted FROM baselines WHERE id = 1",
            [],
            |row| {
                Ok(Baseline {
                    fillers_per_min: row.get(0)?,
                    words_per_min: row.get(1)?,
                    sessions_counted: row.get(2)?,
                })
            },
        );
        match result {
            Ok(baseline) => Ok(Some(baseline)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn upsert_baseline(
        &self,
        fillers_per_min: f64,
        words_per_min: f64,
        sessions_counted: i64,
    ) -> Result<(), YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT OR REPLACE INTO baselines (id, fillers_per_min, words_per_min, sessions_counted)
             VALUES (1, ?1, ?2, ?3)",
            params![fillers_per_min, words_per_min, sessions_counted],
        )?;
        Ok(())
    }

    pub fn set_session_stats(
        &self,
        id: i64,
        filler_count: i64,
        word_count: i64,
    ) -> Result<(), YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "UPDATE sessions SET filler_count = ?2, word_count = ?3 WHERE id = ?1",
            params![id, filler_count, word_count],
        )?;
        Ok(())
    }

    /// Point a session's `audio_path` at a new file — used by the
    /// post-session FLAC encode to swap the WAV path for the FLAC path once
    /// compression succeeds. Does not touch any other column (duration,
    /// paused_ms, etc. are unaffected by re-encoding the same timeline).
    pub fn set_audio_path(&self, id: i64, path: &str) -> Result<(), YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "UPDATE sessions SET audio_path = ?2 WHERE id = ?1",
            params![id, path],
        )?;
        Ok(())
    }

    pub fn replace_outline(
        &self,
        session_id: i64,
        entries: &[(&str, &str)],
        at_ms: i64,
    ) -> Result<(), YapperError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let tx = conn.transaction()?;
        // Delete all existing entries for this session
        tx.execute(
            "DELETE FROM outline_entries WHERE session_id = ?1",
            params![session_id],
        )?;
        // Insert all new entries
        for (label, status) in entries {
            tx.execute(
                "INSERT INTO outline_entries (session_id, label, status, updated_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![session_id, label, status, at_ms],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_outline(&self, session_id: i64) -> Result<Vec<OutlineRow>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, session_id, label, status, updated_at_ms FROM outline_entries
             WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(OutlineRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                label: row.get(2)?,
                status: row.get(3)?,
                updated_at_ms: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_event_feedback(&self, event_id: i64, feedback: &str) -> Result<(), YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "UPDATE events SET user_feedback = ?2 WHERE id = ?1",
            params![event_id, feedback],
        )?;
        Ok(())
    }

    /// Count events with a given kind where user_feedback = 'wrong' across all sessions.
    /// Used for feedback-driven threshold tuning: learning from correction signals.
    pub fn count_wrong_feedback(&self, kind: &str) -> Result<i64, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind = ?1 AND user_feedback = 'wrong'",
            params![kind],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn typical_session_ms(&self) -> Result<Option<i64>, YapperError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT duration_ms FROM sessions
             WHERE duration_ms IS NOT NULL
             ORDER BY started_at_ms DESC
             LIMIT 10",
        )?;
        let durations: Vec<i64> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        if durations.len() < 3 {
            return Ok(None);
        }

        // Calculate median: for even count, use integer division of average of two middles
        let mut sorted = durations;
        sorted.sort();
        let mid = sorted.len() / 2;
        let median = if sorted.len() % 2 == 1 {
            sorted[mid]
        } else {
            // Even count: average of two middle elements, rounded down via integer division
            (sorted[mid - 1] + sorted[mid]) / 2
        };
        Ok(Some(median))
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
            filler_count: row.get(7)?,
            word_count: row.get(8)?,
            focus: row.get(9)?,
            audio_exists: false, // filesystem check happens at the command layer
            segment_count: 0,    // filled in by the command layer via count_segments
        })
    }
}

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
        let id = store
            .create_session(1_700_000_000_000, "why i left")
            .unwrap();
        let s = store.get_session(id).unwrap();
        assert_eq!(s.intent, "why i left");
        assert_eq!(s.started_at_ms, 1_700_000_000_000);
        assert!(s.ended_at_ms.is_none());
    }

    #[test]
    fn end_session_records_duration_audio_and_pause() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store.end_session(id, 61_000, "/tmp/a.wav", 5_000).unwrap();
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

    #[test]
    fn delete_session_removes_row() {
        let store = open_test_store();
        let id = store.create_session(1000, "orphan").unwrap();
        store.delete_session(id).unwrap();
        assert!(store.get_session(id).is_err());
    }

    #[test]
    fn v2_migration_adds_transcript_segments() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store.add_segment(id, 0, 1500, "hello world").unwrap();
        store.add_segment(id, 1600, 3000, "second thought").unwrap();
        let segs = store.list_segments(id).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "hello world");
        assert_eq!(segs[1].start_ms, 1600);
    }

    #[test]
    fn segments_are_deleted_with_their_session() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store.add_segment(id, 0, 500, "x").unwrap();
        store.delete_session(id).unwrap();
        assert!(store.list_segments(id).unwrap().is_empty());
    }

    #[test]
    fn count_segments_reflects_additions() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        assert_eq!(store.count_segments(id).unwrap(), 0);
        store.add_segment(id, 0, 500, "x").unwrap();
        store.add_segment(id, 500, 1000, "y").unwrap();
        assert_eq!(store.count_segments(id).unwrap(), 2);
    }

    #[test]
    fn v3_events_roundtrip_and_cascade() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store
            .add_event(id, 5000, "rhythm_filler", "racing a little")
            .unwrap();
        let evs = store.list_events(id).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, "rhythm_filler");
        assert!(evs[0].user_feedback.is_none());
        store.delete_session(id).unwrap();
        assert!(store.list_events(id).unwrap().is_empty());
    }

    #[test]
    fn baseline_upsert_and_fetch() {
        let store = open_test_store();
        assert!(store.get_baseline().unwrap().is_none());
        store.upsert_baseline(4.2, 150.0, 2).unwrap();
        let b = store.get_baseline().unwrap().unwrap();
        assert_eq!(b.sessions_counted, 2);
        assert!((b.fillers_per_min - 4.2).abs() < 1e-6);
        store.upsert_baseline(3.8, 148.0, 3).unwrap();
        assert_eq!(store.get_baseline().unwrap().unwrap().sessions_counted, 3);
    }

    #[test]
    fn session_stats_update() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store.set_session_stats(id, 12, 300).unwrap();
        let s = store.get_session(id).unwrap();
        assert_eq!(s.filler_count, Some(12));
        assert_eq!(s.word_count, Some(300));
    }

    #[test]
    fn v4_outline_roundtrip_replace_and_cascade() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();

        // First outline snapshot
        store
            .replace_outline(id, &[("the move", "covered"), ("burnout", "current")], 5000)
            .unwrap();
        let outline = store.list_outline(id).unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].label, "the move");
        assert_eq!(outline[0].status, "covered");
        assert_eq!(outline[1].label, "burnout");
        assert_eq!(outline[1].status, "current");

        // Replace with different entry
        store
            .replace_outline(id, &[("restarting", "current")], 10000)
            .unwrap();
        let outline = store.list_outline(id).unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].label, "restarting");
        assert_eq!(outline[0].status, "current");

        // Delete session cascades to outline
        store.delete_session(id).unwrap();
        assert!(store.list_outline(id).unwrap().is_empty());
    }

    #[test]
    fn event_feedback_persists() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        let event_id = store
            .add_event(id, 5000, "rhythm_filler", "racing")
            .unwrap();

        // Initially no feedback
        let evs = store.list_events(id).unwrap();
        assert_eq!(evs.len(), 1);
        assert!(evs[0].user_feedback.is_none());

        // Set feedback
        store.set_event_feedback(event_id, "wrong").unwrap();

        // Verify it persists
        let evs = store.list_events(id).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].user_feedback.as_deref(), Some("wrong"));
    }

    #[test]
    fn typical_session_ms_median() {
        let store = open_test_store();

        // <3 sessions returns None
        let id1 = store.create_session(1000, "").unwrap();
        store.end_session(id1, 61_000, "/tmp/a.wav", 0).unwrap();
        let id2 = store.create_session(100_000, "").unwrap();
        store.end_session(id2, 180_000, "/tmp/b.wav", 0).unwrap();
        assert!(store.typical_session_ms().unwrap().is_none());

        // 3 sessions: median is the middle one
        let id3 = store.create_session(200_000, "").unwrap();
        store.end_session(id3, 800_000, "/tmp/c.wav", 0).unwrap();
        // Durations: 60_000, 80_000, 600_000 → sorted → 60_000, 80_000, 600_000 → median is 80_000
        assert_eq!(store.typical_session_ms().unwrap(), Some(80_000));

        // 4 sessions: average of two middles, rounded down via integer division
        let id4 = store.create_session(900_000, "").unwrap();
        store.end_session(id4, 1_320_000, "/tmp/d.wav", 0).unwrap();
        // Durations: 60_000, 80_000, 600_000, 420_000 → sorted → 60_000, 80_000, 420_000, 600_000
        // Two middles: 80_000 and 420_000 → (80_000 + 420_000) / 2 = 250_000
        assert_eq!(store.typical_session_ms().unwrap(), Some(250_000));
    }

    #[test]
    fn replace_outline_failure_leaves_connection_usable() {
        let store = open_test_store();

        // Try to replace_outline with a non-existent session_id (FK constraint will fail on INSERT).
        // This will fail because session_id 9999 doesn't exist, triggering the foreign key constraint
        // during the insert. The rusqlite Transaction auto-rolls back on drop, leaving the connection usable.
        let result = store.replace_outline(9999, &[("test", "current")], 1000);
        assert!(result.is_err(), "Expected FK constraint error");

        // Verify the connection is still usable by creating a session and replacing its outline.
        let valid_id = store.create_session(2000, "").unwrap();
        let outline_result = store.replace_outline(valid_id, &[("recovery", "current")], 3000);
        assert!(
            outline_result.is_ok(),
            "Connection should be usable after transaction failure"
        );

        // Verify the outline was actually inserted.
        let outline = store.list_outline(valid_id).unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].label, "recovery");

        // One more operation to be sure: another create_session + replace_outline.
        let id2 = store.create_session(5000, "").unwrap();
        store
            .replace_outline(id2, &[("second", "covered")], 6000)
            .unwrap();
        let outline2 = store.list_outline(id2).unwrap();
        assert_eq!(outline2.len(), 1);
        assert_eq!(outline2[0].status, "covered");
    }

    #[test]
    fn set_audio_path_updates_get_session() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store.end_session(id, 61_000, "/tmp/a.wav", 0).unwrap();
        assert_eq!(store.get_session(id).unwrap().audio_path.as_deref(), Some("/tmp/a.wav"));

        store.set_audio_path(id, "/tmp/a.flac").unwrap();

        let s = store.get_session(id).unwrap();
        assert_eq!(s.audio_path.as_deref(), Some("/tmp/a.flac"));
        // Other end_session-written fields are untouched by the swap.
        assert_eq!(s.ended_at_ms, Some(61_000));
        assert_eq!(s.duration_ms, Some(60_000));
    }

    #[test]
    fn count_wrong_feedback_initially_zero() {
        let store = open_test_store();
        let count = store.count_wrong_feedback("rhythm_filler").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn count_wrong_feedback_per_kind_across_sessions() {
        let store = open_test_store();

        // Session 1: add rhythm_filler and rhythm_pace events
        let id1 = store.create_session(1000, "").unwrap();
        let filler_event_1 = store
            .add_event(id1, 5000, "rhythm_filler", "racing")
            .unwrap();
        let pace_event_1 = store.add_event(id1, 6000, "rhythm_pace", "quick").unwrap();
        let _other_event_1 = store
            .add_event(id1, 7000, "repetition", "repeating")
            .unwrap();

        // Session 2: add more events
        let id2 = store.create_session(10000, "").unwrap();
        let filler_event_2 = store
            .add_event(id2, 15000, "rhythm_filler", "racing")
            .unwrap();
        let pace_event_2 = store.add_event(id2, 16000, "rhythm_pace", "quick").unwrap();

        // Initially: no wrong feedback on any
        assert_eq!(store.count_wrong_feedback("rhythm_filler").unwrap(), 0);
        assert_eq!(store.count_wrong_feedback("rhythm_pace").unwrap(), 0);
        assert_eq!(store.count_wrong_feedback("repetition").unwrap(), 0);

        // Mark some as wrong: both filler events + one pace event
        store
            .set_event_feedback(filler_event_1, "wrong")
            .unwrap();
        store
            .set_event_feedback(filler_event_2, "wrong")
            .unwrap();
        store.set_event_feedback(pace_event_1, "wrong").unwrap();

        // Verify counts per kind, including non-existent kinds
        assert_eq!(
            store.count_wrong_feedback("rhythm_filler").unwrap(),
            2,
            "should count both filler wrongs across sessions"
        );
        assert_eq!(
            store.count_wrong_feedback("rhythm_pace").unwrap(),
            1,
            "should count one pace wrong"
        );
        assert_eq!(
            store.count_wrong_feedback("repetition").unwrap(),
            0,
            "repetition event was never marked wrong"
        );

        // Mark the second pace event as wrong
        store
            .set_event_feedback(pace_event_2, "wrong")
            .unwrap();
        assert_eq!(
            store.count_wrong_feedback("rhythm_pace").unwrap(),
            2,
            "should now count both pace wrongs"
        );
    }

    #[test]
    fn retro_round_trip_upsert_and_latest() {
        let store = SessionStore::open_in_memory().unwrap();
        let s1 = store.create_session(0, "first").unwrap();
        let s2 = store.create_session(1, "second").unwrap();

        assert!(store.get_retro(s1).unwrap().is_none());
        assert!(store.latest_try_next().unwrap().is_none());

        store
            .save_retro(
                &RetroRow {
                    session_id: s1,
                    stakes: Some("what the move cost".into()),
                    opening: None,
                    landing: Some("landed on the quiet".into()),
                    try_next: "open inside the moment".into(),
                },
                1_000,
            )
            .unwrap();
        store
            .save_retro(
                &RetroRow {
                    session_id: s2,
                    stakes: None,
                    opening: Some("preamble first".into()),
                    landing: None,
                    try_next: "name the stakes early".into(),
                },
                2_000,
            )
            .unwrap();

        let r1 = store.get_retro(s1).unwrap().expect("retro saved");
        assert_eq!(r1.stakes.as_deref(), Some("what the move cost"));
        assert!(r1.opening.is_none());
        assert_eq!(r1.try_next, "open inside the moment");

        // Latest = most recently created retro.
        assert_eq!(
            store.latest_try_next().unwrap().as_deref(),
            Some("name the stakes early")
        );

        // Upsert replaces in place.
        store
            .save_retro(
                &RetroRow {
                    session_id: s1,
                    stakes: None,
                    opening: None,
                    landing: None,
                    try_next: "revised".into(),
                },
                3_000,
            )
            .unwrap();
        assert_eq!(store.get_retro(s1).unwrap().unwrap().try_next, "revised");
        assert_eq!(store.latest_try_next().unwrap().as_deref(), Some("revised"));
    }
}
