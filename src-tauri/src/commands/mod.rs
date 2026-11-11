//! Tauri commands. Each command's logic is a plain function over `AppState`
//! so it can be tested without a Tauri runtime; the `#[tauri::command]`
//! functions are thin wrappers.

pub mod auth;
pub mod chats;
pub mod handlers;
pub mod models;
pub mod turn;

#[cfg(test)]
mod tests;
