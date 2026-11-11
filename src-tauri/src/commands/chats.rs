//! Chat list, messages and usage, straight from the local store.

use super::chat_state::require_chat;
use super::turn::AttachmentInput;
use crate::engine::UiError;
use crate::state::AppState;
use crate::store::{self, ChatSummary, StoredMessage};
use serde::Serialize;

const TURN_RUNNING: &str = "Wait for the reply to finish, or stop it, before editing.";
const NOT_EDITABLE: &str = "That message cannot be edited.";

/// A user message taken back for editing, ready for the message box.
/// `removed` counts the rows deleted after it (messages and notes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TakenMessage {
    pub text: String,
    pub attachments: Vec<AttachmentInput>,
    pub removed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatUsage {
    pub requests: u32,
    pub cost_usd: f64,
}

pub(crate) fn local(_: store::StoreError) -> UiError {
    UiError::other("Could not read or save your chats on this computer.")
}

pub fn list_chats(s: &AppState) -> Result<Vec<ChatSummary>, UiError> {
    s.store.list_chats().map_err(local)
}

pub fn get_messages(s: &AppState, chat_id: &str) -> Result<Vec<StoredMessage>, UiError> {
    s.store.get_messages(chat_id).map_err(local)
}

pub fn create_chat(s: &AppState, model_id: &str) -> Result<ChatSummary, UiError> {
    s.store.create_chat(model_id).map_err(local)
}

pub fn rename_chat(s: &AppState, id: &str, title: &str) -> Result<(), UiError> {
    s.store.rename_chat(id, title).map_err(local)
}

/// Stops a reply still running in the chat, then removes it.
pub fn delete_chat(s: &AppState, id: &str) -> Result<(), UiError> {
    if let Some(turn) = s.turns().remove(id) {
        turn.cancel.cancel();
    }
    s.store.delete_chat(id).map_err(local)
}

pub fn chat_usage(s: &AppState, chat_id: &str) -> Result<ChatUsage, UiError> {
    let (requests, cost_usd) = s.store.chat_usage(chat_id).map_err(local)?;
    Ok(ChatUsage { requests, cost_usd })
}

/// Spend in the current UTC calendar month.
pub fn month_spend(s: &AppState) -> Result<f64, UiError> {
    s.store.month_spend(store::now()).map_err(local)
}

/// The user message `message_id` of the chat and how many rows follow it.
fn user_message(
    s: &AppState,
    chat_id: &str,
    message_id: &str,
) -> Result<(StoredMessage, u32), UiError> {
    require_chat(s, chat_id)?;
    let rows = s.store.get_messages(chat_id).map_err(local)?;
    let at = rows
        .iter()
        .position(|m| m.id == message_id && m.kind == "message" && m.role == "user")
        .ok_or_else(|| UiError::other(NOT_EDITABLE))?;
    let after = (rows.len() - at - 1) as u32;
    Ok((
        rows.into_iter().nth(at).expect("position is in range"),
        after,
    ))
}

/// How many rows taking `message_id` back would remove after it.
pub fn rows_after(s: &AppState, chat_id: &str, message_id: &str) -> Result<u32, UiError> {
    user_message(s, chat_id, message_id).map(|(_, after)| after)
}

/// Deletes the user message and everything after it, returning its text and
/// attachments. A summary that described any of the deleted rows no longer
/// describes the chat, so it is dropped and the earlier messages are sent
/// in full again; active skills stay.
pub fn take_user_message(
    s: &AppState,
    chat_id: &str,
    message_id: &str,
) -> Result<TakenMessage, UiError> {
    require_chat(s, chat_id)?;
    if s.turns().contains_key(chat_id) {
        return Err(UiError::other(TURN_RUNNING));
    }
    let (message, removed) = user_message(s, chat_id, message_id)?;
    s.store.delete_from(chat_id, message_id).map_err(local)?;
    if let Some((_, boundary)) = s.store.summary(chat_id).map_err(local)?
        && message.seq < boundary
    {
        s.store.clear_summary(chat_id).map_err(local)?;
    }
    Ok(TakenMessage {
        text: message.text,
        attachments: message.attachments.into_iter().map(Into::into).collect(),
        removed,
    })
}
