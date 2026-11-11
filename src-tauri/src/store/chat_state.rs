//! Per-chat state (active skills, folder, compaction summary), the history a
//! turn may send, and the on/off switches for skills and packs.

use super::chats::{MESSAGE_COLUMNS, message_row};
use super::{Result, Store, StoredMessage};
use rusqlite::{OptionalExtension, params};

impl Store {
    pub fn active_skills(&self, chat_id: &str) -> Result<Vec<String>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT active_skills FROM chats WHERE id = ?1",
                [chat_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default())
    }

    pub fn set_active_skills(&self, chat_id: &str, ids: &[String]) -> Result<()> {
        let json = serde_json::to_string(ids).expect("strings always serialise");
        self.conn().execute(
            "UPDATE chats SET active_skills = ?2 WHERE id = ?1",
            params![chat_id, json],
        )?;
        Ok(())
    }

    pub fn folder(&self, chat_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT folder FROM chats WHERE id = ?1", [chat_id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    pub fn set_folder(&self, chat_id: &str, folder: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE chats SET folder = ?2 WHERE id = ?1",
            params![chat_id, folder],
        )?;
        Ok(())
    }

    /// The compaction summary and the `messages.seq` from which history is
    /// sent again, when a summary has been set.
    pub fn summary(&self, chat_id: &str) -> Result<Option<(String, i64)>> {
        let row: Option<(Option<String>, Option<i64>)> = self
            .conn()
            .query_row(
                "SELECT summary, sent_from_seq FROM chats WHERE id = ?1",
                [chat_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((Some(text), Some(seq))) => Some((text, seq)),
            _ => None,
        })
    }

    pub fn set_summary(&self, chat_id: &str, text: &str, sent_from_seq: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE chats SET summary = ?2, sent_from_seq = ?3 WHERE id = ?1",
            params![chat_id, text, sent_from_seq],
        )?;
        Ok(())
    }

    /// Removes the message and every later row of the same chat (notes
    /// included), in one transaction. An id that is not in this chat is an
    /// error and nothing changes.
    pub fn delete_from(&self, chat_id: &str, message_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT seq FROM messages WHERE chat_id = ?1 AND id = ?2",
            params![chat_id, message_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "DELETE FROM messages WHERE chat_id = ?1 AND seq >= ?2",
            params![chat_id, seq],
        )?;
        tx.commit()
    }

    /// The messages a turn may send: no notes, and nothing before the
    /// summary's boundary.
    pub fn sendable_messages(&self, chat_id: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages
             WHERE chat_id = ?1 AND kind = 'message'
               AND seq >= COALESCE((SELECT sent_from_seq FROM chats WHERE id = ?1), 0)
             ORDER BY seq"
        ))?;
        stmt.query_map([chat_id], message_row)?.collect()
    }

    pub fn switch(&self, id: &str) -> Result<Option<bool>> {
        self.conn()
            .query_row("SELECT is_on FROM switches WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn set_switch(&self, id: &str, on: bool) -> Result<()> {
        self.conn().execute(
            "INSERT INTO switches (id, is_on) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET is_on = excluded.is_on",
            params![id, on],
        )?;
        Ok(())
    }
}
