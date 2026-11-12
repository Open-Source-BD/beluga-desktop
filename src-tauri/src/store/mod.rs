//! Local SQLite store for chats, messages and usage. All timestamps are UTC
//! unix seconds.

mod agent;
mod chat_state;
mod chats;
mod migrate;
mod usage;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub use rusqlite::Error as StoreError;
pub type Result<T> = rusqlite::Result<T>;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub model_id: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAttachment {
    pub name: String,
    pub mime: String,
    pub data_base64: String,
}

/// A tool call the model made, as stored on its assistant row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as raw JSON text, exactly as the model wrote them.
    pub arguments: String,
}

/// What the interface needs to show a tool result as a trail line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMeta {
    pub name: String,
    /// `"ok"`, `"error"`, `"declined"` or `"not_run"`.
    pub status: String,
    pub summary: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub attachments: Vec<StoredAttachment>,
    pub interrupted: bool,
    pub created_at: i64,
    /// `"message"`, `"note"` or `"tool"` (a tool result, role `"tool"`).
    pub kind: String,
    /// Position in the store's insertion order (the `messages.seq` column).
    pub seq: i64,
    /// Calls made by an assistant row; empty for every other row.
    pub tool_calls: Vec<StoredToolCall>,
    /// On a tool result row: the id of the call it answers.
    pub tool_call_id: Option<String>,
    pub tool_meta: Option<ToolMeta>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate::migrate(&mut conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Current time as UTC unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
