//! Chat list, messages and usage, straight from the local store.

use crate::engine::UiError;
use crate::state::AppState;
use crate::store::{self, ChatSummary, StoredMessage};
use serde::Serialize;

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
