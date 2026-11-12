//! Tauri commands. Each command's logic is a plain function over `AppState`
//! so it can be tested without a Tauri runtime; the `#[tauri::command]`
//! functions are thin wrappers.

pub mod agent;
pub mod auth;
pub mod chat_state;
pub mod chats;
pub mod compact;
pub mod export;
pub mod folder;
pub mod handlers;
pub mod install;
pub mod models;
pub mod skills;
pub mod turn;

#[cfg(test)]
mod install_tests;
#[cfg(test)]
mod tests;

/// Runs blocking work (a dialog, file moves) off the async workers.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, crate::engine::UiError> + Send + 'static,
) -> Result<T, crate::engine::UiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| crate::engine::UiError::other("That did not finish. Try again."))?
}
