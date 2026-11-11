//! Saving a chat as a Markdown file.

use super::chat_state::{NO_CHAT, require_chat};
use super::chats::local;
use crate::engine::UiError;
use crate::state::AppState;
use crate::store::StoredMessage;
use std::path::Path;

const CANNOT_SAVE: &str = "Could not save the file. Choose another location.";
const UNTITLED: &str = "New chat";

fn quoted(text: &str) -> String {
    text.lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn block(m: &StoredMessage, model_id: &str) -> String {
    if m.kind == "note" {
        return quoted(&m.text);
    }
    let heading = if m.role == "user" { "You" } else { model_id };
    let mut out = format!("## {heading}\n\n{}", m.text);
    if !m.attachments.is_empty() {
        let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
        out.push_str(&format!("\n\nAttachments: {}", names.join(", ")));
    }
    out
}

/// The chat as Markdown with `\n` line endings: the title, then every row in
/// order (notes and messages from before a compaction included), with the
/// summary placed where the summarised messages end. Message text, code
/// fences included, is written as it is.
fn markdown(s: &AppState, chat_id: &str) -> Result<String, UiError> {
    let chat = s
        .store
        .get_chat(chat_id)
        .map_err(local)?
        .ok_or_else(|| UiError::other(NO_CHAT))?;
    let title = if chat.title.is_empty() {
        UNTITLED
    } else {
        &chat.title
    };
    let mut summary = s.store.summary(chat_id).map_err(local)?;
    let summary_block = |text: &str| format!("## Summary of earlier messages\n\n{text}");
    let mut blocks = vec![format!("# {title}")];
    for m in s.store.get_messages(chat_id).map_err(local)? {
        if let Some((text, _)) = summary.take_if(|(_, from)| m.seq >= *from) {
            blocks.push(summary_block(&text));
        }
        blocks.push(block(&m, &chat.model_id));
    }
    if let Some((text, _)) = summary {
        blocks.push(summary_block(&text));
    }
    let text = blocks.join("\n\n") + "\n";
    Ok(text.replace("\r\n", "\n"))
}

/// Writes the chat as Markdown to `path`, which must be absolute (it comes
/// from a save dialog).
pub fn export_chat(s: &AppState, chat_id: &str, path: &str) -> Result<(), UiError> {
    require_chat(s, chat_id)?;
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(UiError::other(CANNOT_SAVE));
    }
    let text = markdown(s, chat_id)?;
    std::fs::write(path, text).map_err(|_| UiError::other(CANNOT_SAVE))
}
