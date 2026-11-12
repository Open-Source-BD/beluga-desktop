//! Schema versions, kept in SQLite's `user_version`. Each step runs only when
//! the database is at the version before it, all inside one transaction.

use super::{Result, StoreError};
use rusqlite::{Connection, TransactionBehavior};

/// The schema version this build knows.
const LATEST: i64 = 4;

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

/// Version 2 to 3: folders the user picked in a system dialog.
const V2_TO_V3: &str = "
CREATE TABLE approved_folders (
    path TEXT PRIMARY KEY NOT NULL,
    added_at INTEGER NOT NULL
);
";

/// Version 3 to 4: tool calls and results, saved command rules, the plan,
/// settings and per-day usage (which outlives the chats it came from).
const V3_TO_V4: &str = "
ALTER TABLE messages ADD COLUMN tool_calls TEXT;
ALTER TABLE messages ADD COLUMN tool_call_id TEXT;
ALTER TABLE messages ADD COLUMN tool_meta TEXT;
ALTER TABLE chats ADD COLUMN edits_allowed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN plan TEXT NOT NULL DEFAULT '[]';
CREATE TABLE command_rules (
    folder TEXT NOT NULL,
    rule TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (folder, rule)
);
CREATE TABLE daily_usage (
    day TEXT PRIMARY KEY NOT NULL,
    requests INTEGER NOT NULL,
    cost_usd REAL NOT NULL
);
CREATE TABLE settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
INSERT INTO daily_usage (day, requests, cost_usd)
    SELECT strftime('%Y-%m-%d', created_at, 'unixepoch'), SUM(requests), SUM(cost_usd)
    FROM usage GROUP BY 1;
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
        version = 2;
    }
    if version == 2 {
        tx.execute_batch(V2_TO_V3)?;
        version = 3;
    }
    if version == 3 {
        tx.execute_batch(V3_TO_V4)?;
    }
    tx.pragma_update(None, "user_version", LATEST)?;
    tx.commit()
}
