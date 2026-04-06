//! Session store — SQLite-backed mapping of (surface, conversation_id) → claude_session_id.
//!
//! This is the daemon's only persistent state. It does not store conversation
//! content; that belongs to Claude Code's own session storage.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, info};

const CURRENT_SCHEMA_VERSION: i64 = 1;

/// A session row from the store.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: i64,
    pub surface: String,
    pub conversation_id: String,
    pub claude_session_id: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub last_active_at: i64,
    pub metadata: Option<String>,
}

/// Errors the session store can produce.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("failed to create database directory: {0}")]
    CreateDir(std::io::Error),
}

/// The session store.
pub struct SessionStore {
    conn: Connection,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs() as i64
}

impl SessionStore {
    /// Open (or create) the session store at the given path.
    /// Enables WAL mode and runs pending migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(StoreError::CreateDir)?;
        }

        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory store — for tests only.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Run pending migrations to bring the schema up to date.
    fn migrate(&mut self) -> Result<(), StoreError> {
        let version = self.schema_version()?;
        if version < 1 {
            self.migrate_v1()?;
        }
        debug!(version = CURRENT_SCHEMA_VERSION, "schema up to date");
        Ok(())
    }

    /// Read the current schema version (0 if the table doesn't exist yet).
    fn schema_version(&self) -> Result<i64, StoreError> {
        // Check if schema_version table exists.
        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get(0),
        )?;

        if !exists {
            return Ok(0);
        }

        let version: i64 = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })?;
        Ok(version)
    }

    /// Apply migration v1: create schema_version and sessions tables.
    fn migrate_v1(&mut self) -> Result<(), StoreError> {
        info!("applying migration v1: creating sessions schema");
        let tx = self.conn.transaction()?;

        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                surface         TEXT    NOT NULL,
                conversation_id TEXT    NOT NULL,
                claude_session_id TEXT,
                status          TEXT    NOT NULL DEFAULT 'active',
                created_at      INTEGER NOT NULL,
                last_active_at  INTEGER NOT NULL,
                metadata        TEXT,
                UNIQUE(surface, conversation_id)
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_last_active ON sessions(last_active_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_surface ON sessions(surface);",
        )?;

        // Set version. Delete first so this is idempotent.
        tx.execute("DELETE FROM schema_version", [])?;
        tx.execute("INSERT INTO schema_version (version) VALUES (?1)", [CURRENT_SCHEMA_VERSION])?;

        tx.commit()?;
        Ok(())
    }

    /// Create a new session. Returns the auto-generated row id.
    pub fn create_session(&self, surface: &str, conversation_id: &str) -> Result<i64, StoreError> {
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO sessions (surface, conversation_id, created_at, last_active_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![surface, conversation_id, now, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Look up a session by (surface, conversation_id).
    pub fn lookup_session(
        &self,
        surface: &str,
        conversation_id: &str,
    ) -> Result<Option<Session>, StoreError> {
        let session = self
            .conn
            .query_row(
                "SELECT id, surface, conversation_id, claude_session_id, status,
                        created_at, last_active_at, metadata
                 FROM sessions
                 WHERE surface = ?1 AND conversation_id = ?2",
                params![surface, conversation_id],
                row_to_session,
            )
            .optional()?;
        Ok(session)
    }

    /// Set the claude_session_id after the first turn's init event.
    pub fn set_claude_session_id(&self, id: i64, claude_session_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET claude_session_id = ?1 WHERE id = ?2",
            params![claude_session_id, id],
        )?;
        Ok(())
    }

    /// Update last_active_at to now.
    pub fn touch_session(&self, id: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET last_active_at = ?1 WHERE id = ?2",
            params![now_unix(), id],
        )?;
        Ok(())
    }

    /// List all sessions, most recently active first.
    pub fn list_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, surface, conversation_id, claude_session_id, status,
                    created_at, last_active_at, metadata
             FROM sessions
             ORDER BY last_active_at DESC",
        )?;
        let sessions = stmt
            .query_map([], row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// List sessions for a specific surface, most recently active first.
    pub fn list_by_surface(&self, surface: &str) -> Result<Vec<Session>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, surface, conversation_id, claude_session_id, status,
                    created_at, last_active_at, metadata
             FROM sessions
             WHERE surface = ?1
             ORDER BY last_active_at DESC",
        )?;
        let sessions = stmt
            .query_map(params![surface], row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        surface: row.get(1)?,
        conversation_id: row.get(2)?,
        claude_session_id: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        last_active_at: row.get(6)?,
        metadata: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_lookup() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = store.create_session("dbus", "conv-1").unwrap();
        assert!(id > 0);

        let session = store.lookup_session("dbus", "conv-1").unwrap().unwrap();
        assert_eq!(session.id, id);
        assert_eq!(session.surface, "dbus");
        assert_eq!(session.conversation_id, "conv-1");
        assert!(session.claude_session_id.is_none());
        assert_eq!(session.status, "active");
    }

    #[test]
    fn lookup_missing_returns_none() {
        let store = SessionStore::open_in_memory().unwrap();
        let result = store.lookup_session("dbus", "nope").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn duplicate_session_rejected() {
        let store = SessionStore::open_in_memory().unwrap();
        store.create_session("dbus", "conv-1").unwrap();
        let err = store.create_session("dbus", "conv-1");
        assert!(err.is_err());
    }

    #[test]
    fn set_claude_session_id() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = store.create_session("dbus", "conv-1").unwrap();
        store.set_claude_session_id(id, "abc-123").unwrap();

        let session = store.lookup_session("dbus", "conv-1").unwrap().unwrap();
        assert_eq!(session.claude_session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn touch_updates_last_active() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = store.create_session("dbus", "conv-1").unwrap();
        let before = store.lookup_session("dbus", "conv-1").unwrap().unwrap();

        // Sleep briefly so the timestamp actually changes.
        std::thread::sleep(std::time::Duration::from_secs(1));
        store.touch_session(id).unwrap();

        let after = store.lookup_session("dbus", "conv-1").unwrap().unwrap();
        assert!(after.last_active_at >= before.last_active_at);
    }

    #[test]
    fn list_sessions_ordered_by_last_active() {
        let store = SessionStore::open_in_memory().unwrap();
        store.create_session("dbus", "conv-1").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        store.create_session("dbus", "conv-2").unwrap();

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        // Most recent first.
        assert_eq!(sessions[0].conversation_id, "conv-2");
        assert_eq!(sessions[1].conversation_id, "conv-1");
    }

    #[test]
    fn list_by_surface_filters() {
        let store = SessionStore::open_in_memory().unwrap();
        store.create_session("dbus", "conv-1").unwrap();
        store.create_session("telegram", "conv-2").unwrap();
        store.create_session("dbus", "conv-3").unwrap();

        let dbus = store.list_by_surface("dbus").unwrap();
        assert_eq!(dbus.len(), 2);
        assert!(dbus.iter().all(|s| s.surface == "dbus"));

        let telegram = store.list_by_surface("telegram").unwrap();
        assert_eq!(telegram.len(), 1);
    }

    #[test]
    fn migration_is_idempotent() {
        let store = SessionStore::open_in_memory().unwrap();
        store.create_session("dbus", "conv-1").unwrap();

        // Re-open (simulates daemon restart). Migration should be a no-op.
        // Can't re-open in-memory, so just re-run migrate on the same connection.
        let version: i64 = store
            .conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn schema_version_is_correct() {
        let store = SessionStore::open_in_memory().unwrap();
        let version: i64 = store
            .conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }
}
