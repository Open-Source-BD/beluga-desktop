//! Storage for project work: tool-call bookkeeping, the chat's plan and
//! "all edits" switch, saved command rules per folder, and app settings.

use super::{Result, Store, StoredToolCall};
use rusqlite::{OptionalExtension, params};

/// Settings key of a started call: the chat id has no `:`, so prefixes of
/// different chats never overlap.
fn started_key(chat_id: &str, call_id: &str) -> String {
    format!("call_started:{chat_id}:{call_id}")
}

impl Store {
    /// Calls in the sendable rows that have no result row yet, oldest first.
    /// A result answers a call only inside the call's own turn: after its
    /// assistant row and before the next one, so a reused id cannot match.
    pub fn unanswered_calls(&self, chat_id: &str) -> Result<Vec<StoredToolCall>> {
        let rows = self.sendable_messages(chat_id)?;
        let mut open: Vec<StoredToolCall> = Vec::new();
        let mut out = Vec::new();
        for m in &rows {
            if m.kind == "tool" {
                open.retain(|c| Some(&c.id) != m.tool_call_id.as_ref());
            } else {
                out.append(&mut open);
                open = m.tool_calls.clone();
            }
        }
        out.append(&mut open);
        Ok(out)
    }

    pub fn edits_allowed(&self, chat_id: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT edits_allowed FROM chats WHERE id = ?1",
                [chat_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn set_edits_allowed(&self, chat_id: &str, allowed: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE chats SET edits_allowed = ?2 WHERE id = ?1",
            params![chat_id, allowed],
        )?;
        Ok(())
    }

    /// The chat's checklist as JSON text (`[]` when there is none).
    pub fn plan(&self, chat_id: &str) -> Result<String> {
        Ok(self
            .conn()
            .query_row("SELECT plan FROM chats WHERE id = ?1", [chat_id], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or_else(|| "[]".to_string()))
    }

    pub fn set_plan(&self, chat_id: &str, plan_json: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE chats SET plan = ?2 WHERE id = ?1",
            params![chat_id, plan_json],
        )?;
        Ok(())
    }

    /// Saved command rules of a folder, oldest first.
    pub fn rules(&self, folder: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT rule FROM command_rules WHERE folder = ?1 ORDER BY added_at, rowid")?;
        stmt.query_map([folder], |r| r.get(0))?.collect()
    }

    /// Saves a rule for a folder; saving it again changes nothing.
    pub fn add_rule(&self, folder: &str, rule: &str) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO command_rules (folder, rule, added_at) VALUES (?1, ?2, ?3)",
            params![folder, rule, super::now()],
        )?;
        Ok(())
    }

    /// Forgets a rule; one that was never saved is not an error.
    pub fn remove_rule(&self, folder: &str, rule: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM command_rules WHERE folder = ?1 AND rule = ?2",
            params![folder, rule],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Records that call `call_id` of the chat is about to act (change a
    /// file, run or stop a command, save the plan), so a result that never
    /// arrives is reported as lost, not as not run.
    pub fn mark_call_started(&self, chat_id: &str, call_id: &str) -> Result<()> {
        self.set_setting(&started_key(chat_id, call_id), "1")
    }

    pub fn call_started(&self, chat_id: &str, call_id: &str) -> Result<bool> {
        Ok(self.setting(&started_key(chat_id, call_id))?.is_some())
    }

    /// Forgets the mark once the call's result is stored.
    pub fn clear_call_started(&self, chat_id: &str, call_id: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM settings WHERE key = ?1",
            [started_key(chat_id, call_id)],
        )?;
        Ok(())
    }

    /// Forgets every mark of the chat (it is being deleted).
    pub fn clear_calls_started(&self, chat_id: &str) -> Result<()> {
        let prefix = started_key(chat_id, "");
        self.conn().execute(
            "DELETE FROM settings WHERE substr(key, 1, length(?1)) = ?1",
            [prefix],
        )?;
        Ok(())
    }

    /// Runs raw SQL, so a test can make a later query fail.
    #[cfg(test)]
    pub(crate) fn exec_for_test(&self, sql: &str) -> Result<()> {
        self.conn().execute_batch(sql)
    }
}
