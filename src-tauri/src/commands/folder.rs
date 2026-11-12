//! A chat's folder: choosing it, listing its files for `@`, and attaching one.
//! Read-only. Every path goes through `crate::folder`, which keeps it inside
//! the folder.

use super::chat_state::require_chat;
use super::chats::local;
use super::turn::{AttachmentInput, MAX_IMAGE_BYTES, MAX_TEXT_BYTES, not_attachable, too_large};
use crate::engine::UiError;
use crate::folder::{self, FileList};
use crate::state::AppState;
use base64::Engine as _;
use std::path::{Path, PathBuf};

/// Entries shown for one `@` query.
const SHOWN: usize = 50;
const IMAGE_TYPES: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

fn cannot_open() -> UiError {
    UiError::other("That folder could not be opened.")
}

fn outside(message: String) -> UiError {
    UiError::other(&message)
}

fn real_folder(path: &str) -> Result<String, UiError> {
    let p = Path::new(path);
    if !p.is_absolute() || !p.is_dir() {
        return Err(cannot_open());
    }
    let real = std::fs::canonicalize(p).map_err(|_| cannot_open())?;
    Ok(real.to_str().ok_or_else(cannot_open)?.to_string())
}

/// Sets the chat's folder to a folder the user picked earlier in a system
/// dialog (`choose_chat_folder`), or clears it. Any other path is refused:
/// the web view cannot name a folder of its own.
///
/// Closing the folder or changing it to another stops the chat's background
/// commands first: they were started in the old one.
pub async fn set_chat_folder(
    s: &AppState,
    chat_id: &str,
    path: Option<String>,
) -> Result<(), UiError> {
    require_chat(s, chat_id)?;
    let stored = match path {
        None => None,
        Some(p) => {
            let real = real_folder(&p)?;
            if !s.store.is_folder_approved(&real).map_err(local)? {
                return Err(cannot_open());
            }
            Some(real)
        }
    };
    release_old_folder(s, chat_id, stored.as_deref()).await?;
    s.store
        .set_folder(chat_id, stored.as_deref())
        .map_err(local)
}

/// Stops the chat's background commands when its folder is about to become
/// `new` and that is not the folder it has now.
async fn release_old_folder(s: &AppState, chat_id: &str, new: Option<&str>) -> Result<(), UiError> {
    let current = s.store.folder(chat_id).map_err(local)?;
    if current.as_deref() != new {
        s.background.stop_chat(chat_id).await;
    }
    Ok(())
}

pub async fn choose_chat_folder(s: &AppState, chat_id: &str) -> Result<Option<String>, UiError> {
    require_chat(s, chat_id)?;
    // The dialog blocks until it closes: keep it off the async threads.
    let dialogs = s.dialogs.clone();
    let picked = tokio::task::spawn_blocking(move || dialogs.pick_folder())
        .await
        .map_err(|_| UiError::other("That did not finish. Try again."))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let real = real_folder(picked.to_str().ok_or_else(cannot_open)?)?;
    s.store.approve_folder(&real).map_err(local)?;
    release_old_folder(s, chat_id, Some(&real)).await?;
    s.store.set_folder(chat_id, Some(&real)).map_err(local)?;
    Ok(Some(real))
}

pub fn approved_folders(s: &AppState) -> Result<Vec<String>, UiError> {
    s.store.approved_folders().map_err(local)
}

fn root_of(s: &AppState, chat_id: &str) -> Result<Option<PathBuf>, UiError> {
    require_chat(s, chat_id)?;
    Ok(s.store.folder(chat_id).map_err(local)?.map(PathBuf::from))
}

pub fn folder_files(s: &AppState, chat_id: &str, query: &str) -> Result<FileList, UiError> {
    Ok(match root_of(s, chat_id)? {
        Some(root) => folder::list_files(&root, query, SHOWN),
        None => FileList {
            entries: Vec::new(),
            total: 0,
            cut: false,
        },
    })
}

fn attachment(name: String, mime: &str, bytes: &[u8]) -> AttachmentInput {
    AttachmentInput {
        name,
        mime: mime.to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

/// A file of the chat's folder as an attachment, classified and limited as
/// a picked file is; a directory becomes its listing.
pub fn attach_folder_file(
    s: &AppState,
    chat_id: &str,
    path: &str,
) -> Result<AttachmentInput, UiError> {
    let root = root_of(s, chat_id)?.ok_or_else(|| UiError::other("Open a folder first."))?;
    let real = folder::resolve(&root, path).map_err(outside)?;
    let segments = folder::segments(path).unwrap_or_default();
    let name = segments.join("/");
    if real.is_dir() {
        let listing = folder::folder_listing(&root, path).map_err(outside)?;
        let label = if name.is_empty() {
            root.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        } else {
            name
        };
        return Ok(attachment(
            format!("{label}/ listing.txt"),
            "text/plain",
            listing.as_bytes(),
        ));
    }
    // The same resolved path is checked and read; a pipe or device is never
    // opened for reading, and the read itself is bounded.
    let is_file = std::fs::metadata(&real).is_ok_and(|m| m.is_file());
    if !is_file {
        return Err(not_attachable(&name));
    }
    let ext = real
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if IMAGE_TYPES.contains(&ext.as_str()) {
        let bytes =
            folder::read_regular(&real, MAX_IMAGE_BYTES as u64).ok_or_else(|| too_large(&name))?;
        let kind = if ext == "jpg" { "jpeg" } else { ext.as_str() };
        return Ok(attachment(name, &format!("image/{kind}"), &bytes));
    }
    let bytes =
        folder::read_regular(&real, MAX_TEXT_BYTES as u64).ok_or_else(|| too_large(&name))?;
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return Err(not_attachable(&name));
    }
    Ok(attachment(name, "text/plain", &bytes))
}
