//! Tauri commands. Each command's logic is a plain function over `AppState`
//! so it can be tested without a Tauri runtime; the `#[tauri::command]`
//! functions are thin wrappers.

pub mod auth;
pub mod chat_state;
pub mod chats;
pub mod compact;
pub mod export;
pub mod handlers;
pub mod models;
pub mod skills;
pub mod turn;

#[cfg(test)]
mod tests;
