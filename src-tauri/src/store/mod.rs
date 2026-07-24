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
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
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
        Ok(())
    }

    pub fn create_session(&self, started_at_ms: i64, intent: &str) -> Result<i64, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
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
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
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
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn add_segment(&self, session_id: i64, start_ms: i64, end_ms: i64, text: &str) -> Result<i64, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO transcript_segments (session_id, start_ms, end_ms, text) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, start_ms, end_ms, text],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn count_segments(&self, session_id: i64) -> Result<i64, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM transcript_segments WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?)
    }

    pub fn list_segments(&self, session_id: i64) -> Result<Vec<TranscriptSegment>, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
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
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        Ok(conn.query_row(
            "SELECT id, started_at_ms, ended_at_ms, intent, audio_path, duration_ms, paused_ms, filler_count, word_count
             FROM sessions WHERE id = ?1",
            params![id],
            Self::row_to_session,
        )?)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT id, started_at_ms, ended_at_ms, intent, audio_path, duration_ms, paused_ms, filler_count, word_count
             FROM sessions ORDER BY started_at_ms DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_session)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn add_event(&self, session_id: i64, at_ms: i64, kind: &str, note: &str) -> Result<i64, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT INTO events (session_id, at_ms, kind, note) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, at_ms, kind, note],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_events(&self, session_id: i64) -> Result<Vec<Event>, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
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
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
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

    pub fn upsert_baseline(&self, fillers_per_min: f64, words_per_min: f64, sessions_counted: i64) -> Result<(), YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "INSERT OR REPLACE INTO baselines (id, fillers_per_min, words_per_min, sessions_counted)
             VALUES (1, ?1, ?2, ?3)",
            params![fillers_per_min, words_per_min, sessions_counted],
        )?;
        Ok(())
    }

    pub fn set_session_stats(&self, id: i64, filler_count: i64, word_count: i64) -> Result<(), YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        conn.execute(
            "UPDATE sessions SET filler_count = ?2, word_count = ?3 WHERE id = ?1",
            params![id, filler_count, word_count],
        )?;
        Ok(())
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
        store.add_event(id, 5000, "rhythm_filler", "racing a little").unwrap();
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
}
