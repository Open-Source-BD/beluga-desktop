use super::{ChatSummary, Result, Store, StoredAttachment, StoredMessage, now};
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

fn summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatSummary> {
    Ok(ChatSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        model_id: row.get(2)?,
        updated_at: row.get(3)?,
    })
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
        let mut stmt = conn.prepare(
            "SELECT id, role, text, attachments, interrupted, created_at FROM messages
             WHERE chat_id = ?1 ORDER BY seq",
        )?;
        stmt.query_map([chat_id], |row| {
            let attachments: String = row.get(3)?;
            Ok(StoredMessage {
                id: row.get(0)?,
                role: row.get(1)?,
                text: row.get(2)?,
                attachments: serde_json::from_str(&attachments).unwrap_or_default(),
                interrupted: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect()
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
        let msg = StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: role.to_string(),
            text: text.to_string(),
            attachments: attachments.to_vec(),
            interrupted,
            created_at: now(),
        };
        let attachments_json =
            serde_json::to_string(&msg.attachments).expect("attachments always serialise");
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let first: bool = tx.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM messages WHERE chat_id = ?1)",
            [chat_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO messages (id, chat_id, role, text, attachments, interrupted, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                msg.id,
                chat_id,
                msg.role,
                msg.text,
                attachments_json,
                msg.interrupted,
                msg.created_at
            ],
        )?;
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
