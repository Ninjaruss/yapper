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
    /// Not persisted — computed at the command layer from the filesystem so
    /// the UI can tell when a recording was deleted out from under us.
    #[serde(default)]
    pub audio_exists: bool,
}

pub struct SessionStore {
    // Mutex needed: Connection is Send but not Sync; one lock per operation.
    conn: Mutex<Connection>,
}

impl SessionStore {
    pub fn open(path: &Path) -> Result<Self, YapperError> {
        let store = Self {
            conn: Mutex::new(Connection::open(path)?),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, YapperError> {
        let store = Self {
            conn: Mutex::new(Connection::open_in_memory()?),
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

    pub fn get_session(&self, id: i64) -> Result<Session, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
        Ok(conn.query_row(
            "SELECT id, started_at_ms, ended_at_ms, intent, audio_path, duration_ms, paused_ms
             FROM sessions WHERE id = ?1",
            params![id],
            Self::row_to_session,
        )?)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, YapperError> {
        let conn = self.conn.lock().map_err(|_| YapperError::State("database lock poisoned".into()))?;
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
            audio_exists: false, // filesystem check happens at the command layer
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
}
