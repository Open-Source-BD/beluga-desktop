use super::{
    ChatSummary, Result, Store, StoredAttachment, StoredMessage, StoredToolCall, ToolMeta, now,
};
use rusqlite::{OptionalExtension, params};

const TITLE_CHARS: usize = 40;

/// The first 40 characters of `text` (whitespace collapsed), cut back to the
/// last word boundary when that cut would split a word.
fn title_from(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = collapsed.chars().collect();
    if chars.len() <= TITLE_CHARS {
        return collapsed;
    }
    let head: String = chars[..TITLE_CHARS].iter().collect();
    if chars[TITLE_CHARS] == ' ' {
        return head;
    }
    match head.rfind(' ') {
        Some(i) => head[..i].to_string(),
        None => head,
    }
}

pub(super) const MESSAGE_COLUMNS: &str = "id, role, text, attachments, interrupted, created_at, kind, seq, tool_calls, tool_call_id, tool_meta";

pub(super) fn message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    let attachments: String = row.get(3)?;
    let tool_calls: Option<String> = row.get(8)?;
    let tool_meta: Option<String> = row.get(10)?;
    Ok(StoredMessage {
        id: row.get(0)?,
        role: row.get(1)?,
        text: row.get(2)?,
        attachments: serde_json::from_str(&attachments).unwrap_or_default(),
        interrupted: row.get(4)?,
        created_at: row.get(5)?,
        kind: row.get(6)?,
        seq: row.get(7)?,
        tool_calls: tool_calls
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
        tool_call_id: row.get(9)?,
        tool_meta: tool_meta.and_then(|j| serde_json::from_str(&j).ok()),
    })
}

fn summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatSummary> {
    Ok(ChatSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        model_id: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

/// The parts of a row to insert; the defaults are an ordinary text row.
struct NewRow<'a> {
    kind: &'a str,
    role: &'a str,
    text: &'a str,
    interrupted: bool,
    attachments: &'a [StoredAttachment],
    tool_calls: &'a [StoredToolCall],
    tool_call_id: Option<&'a str>,
    tool_meta: Option<&'a ToolMeta>,
}

impl<'a> NewRow<'a> {
    fn new(kind: &'a str, role: &'a str, text: &'a str, interrupted: bool) -> Self {
        NewRow {
            kind,
            role,
            text,
            interrupted,
            attachments: &[],
            tool_calls: &[],
            tool_call_id: None,
            tool_meta: None,
        }
    }
}

impl Store {
    pub fn create_chat(&self, model_id: &str) -> Result<ChatSummary> {
        let chat = ChatSummary {
            id: uuid::Uuid::new_v4().to_string(),
            title: String::new(),
            model_id: model_id.to_string(),
            updated_at: now(),
        };
        self.conn().execute(
            "INSERT INTO chats (id, title, model_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![chat.id, chat.title, chat.model_id, chat.updated_at],
        )?;
        Ok(chat)
    }

    /// Newest activity first.
    pub fn list_chats(&self) -> Result<Vec<ChatSummary>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, model_id, updated_at FROM chats ORDER BY updated_at DESC, rowid DESC",
        )?;
        stmt.query_map([], summary)?.collect()
    }

    pub fn get_chat(&self, id: &str) -> Result<Option<ChatSummary>> {
        self.conn()
            .query_row(
                "SELECT id, title, model_id, updated_at FROM chats WHERE id = ?1",
                [id],
                summary,
            )
            .optional()
    }

    /// Every message of the chat, oldest first.
    pub fn get_messages(&self, chat_id: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE chat_id = ?1 ORDER BY seq"
        ))?;
        stmt.query_map([chat_id], message_row)?.collect()
    }

    /// Adds a message and marks the chat as active. The first user message of
    /// an untitled chat gives the chat its title.
    pub fn append_message(
        &self,
        chat_id: &str,
        role: &str,
        text: &str,
        attachments: &[StoredAttachment],
        interrupted: bool,
    ) -> Result<StoredMessage> {
        self.insert_message(chat_id, "message", role, text, attachments, interrupted)
    }

    /// Adds a note (a line shown in the thread, never sent to the model).
    pub fn append_note(&self, chat_id: &str, text: &str) -> Result<StoredMessage> {
        self.insert_message(chat_id, "note", "system", text, &[], false)
    }

    /// Adds an assistant message, with the tool calls it made (none for a
    /// plain answer).
    pub fn append_assistant(
        &self,
        chat_id: &str,
        text: &str,
        tool_calls: &[StoredToolCall],
        interrupted: bool,
    ) -> Result<StoredMessage> {
        self.insert_row(
            chat_id,
            NewRow {
                tool_calls,
                ..NewRow::new("message", "assistant", text, interrupted)
            },
        )
    }

    /// Adds the result of tool call `call_id`.
    pub fn append_tool_result(
        &self,
        chat_id: &str,
        call_id: &str,
        meta: &ToolMeta,
        text: &str,
    ) -> Result<StoredMessage> {
        self.insert_row(
            chat_id,
            NewRow {
                tool_call_id: Some(call_id),
                tool_meta: Some(meta),
                ..NewRow::new("tool", "tool", text, false)
            },
        )
    }

    fn insert_message(
        &self,
        chat_id: &str,
        kind: &str,
        role: &str,
        text: &str,
        attachments: &[StoredAttachment],
        interrupted: bool,
    ) -> Result<StoredMessage> {
        self.insert_row(
            chat_id,
            NewRow {
                attachments,
                ..NewRow::new(kind, role, text, interrupted)
            },
        )
    }

    fn insert_row(&self, chat_id: &str, row: NewRow<'_>) -> Result<StoredMessage> {
        let mut msg = StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: row.role.to_string(),
            text: row.text.to_string(),
            attachments: row.attachments.to_vec(),
            interrupted: row.interrupted,
            created_at: now(),
            kind: row.kind.to_string(),
            seq: 0,
            tool_calls: row.tool_calls.to_vec(),
            tool_call_id: row.tool_call_id.map(str::to_string),
            tool_meta: row.tool_meta.cloned(),
        };
        let attachments_json =
            serde_json::to_string(&msg.attachments).expect("attachments always serialise");
        let tool_calls_json = (!msg.tool_calls.is_empty())
            .then(|| serde_json::to_string(&msg.tool_calls).expect("tool calls always serialise"));
        let tool_meta_json = msg
            .tool_meta
            .as_ref()
            .map(|m| serde_json::to_string(m).expect("tool meta always serialises"));
        let (role, text) = (row.role, row.text);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let first: bool = tx.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM messages WHERE chat_id = ?1)",
            [chat_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO messages (id, chat_id, role, text, attachments, interrupted, created_at, kind,
                                   tool_calls, tool_call_id, tool_meta)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                msg.id,
                chat_id,
                msg.role,
                msg.text,
                attachments_json,
                msg.interrupted,
                msg.created_at,
                msg.kind,
                tool_calls_json,
                msg.tool_call_id,
                tool_meta_json
            ],
        )?;
        msg.seq = tx.last_insert_rowid();
        tx.execute(
            "UPDATE chats SET updated_at = ?2 WHERE id = ?1",
            params![chat_id, msg.created_at],
        )?;
        if first && role == "user" {
            tx.execute(
                "UPDATE chats SET title = ?2 WHERE id = ?1 AND title = ''",
                params![chat_id, title_from(text)],
            )?;
        }
        tx.commit()?;
        Ok(msg)
    }

    pub fn delete_message(&self, chat_id: &str, id: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM messages WHERE chat_id = ?1 AND id = ?2",
            params![chat_id, id],
        )?;
        Ok(())
    }

    pub fn set_chat_model(&self, id: &str, model_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE chats SET model_id = ?2 WHERE id = ?1",
            params![id, model_id],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn touch_chat_at(&self, id: &str, at: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE chats SET updated_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        Ok(())
    }

    pub fn rename_chat(&self, id: &str, title: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE chats SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(())
    }

    /// Removes the chat with its messages and usage.
    pub fn delete_chat(&self, id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM chats WHERE id = ?1", [id])?;
        Ok(())
    }
}
