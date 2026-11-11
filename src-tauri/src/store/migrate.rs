//! Schema versions, kept in SQLite's `user_version`. Each step runs only when
//! the database is at the version before it, all inside one transaction.

use super::{Result, StoreError};
use rusqlite::{Connection, TransactionBehavior};

/// The schema version this build knows.
const LATEST: i64 = 2;

/// Exactly the tables the 0.1.x releases created (version 1).
const V1: &str = "
CREATE TABLE IF NOT EXISTS chats (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    model_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    text TEXT NOT NULL,
    attachments TEXT NOT NULL,
    interrupted INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_chat ON messages(chat_id, seq);
CREATE TABLE IF NOT EXISTS usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    requests INTEGER NOT NULL,
    cost_usd REAL NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS usage_chat ON usage(chat_id);
CREATE INDEX IF NOT EXISTS usage_time ON usage(created_at);
";

/// Version 1 to 2: per-chat state, message kinds and on/off switches.
const V1_TO_V2: &str = "
ALTER TABLE chats ADD COLUMN active_skills TEXT NOT NULL DEFAULT '[]';
ALTER TABLE chats ADD COLUMN folder TEXT;
ALTER TABLE chats ADD COLUMN summary TEXT;
ALTER TABLE chats ADD COLUMN sent_from_seq INTEGER;
ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'message';
CREATE TABLE switches (
    id TEXT PRIMARY KEY NOT NULL,
    is_on INTEGER NOT NULL
);
";

fn newer_database() -> StoreError {
    StoreError::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_SCHEMA),
        Some("This database was made by a newer version of the app.".to_string()),
    )
}

/// Brings the database to the latest version. A fresh database (version 0)
/// goes straight to the latest; one made by a newer build is refused before
/// anything is written.
pub(super) fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > LATEST {
        return Err(newer_database());
    }
    if version == 0 {
        tx.execute_batch(V1)?;
        version = 1;
    }
    if version == 1 {
        tx.execute_batch(V1_TO_V2)?;
    }
    tx.pragma_update(None, "user_version", LATEST)?;
    tx.commit()
}
