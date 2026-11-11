//! Sending a message and stopping a reply.

use super::auth::revoke;
use super::chats::local;
use crate::engine::{EngineEvent, TurnRequest, UiError};
use crate::openrouter::models::ModelCache;
use crate::openrouter::stream::{Part, Role, WireMessage};
use crate::state::{AppState, RunningTurn};
use crate::store::{StoredAttachment, StoredMessage};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInput {
    pub name: String,
    pub mime: String,
    pub data_base64: String,
}

impl From<AttachmentInput> for StoredAttachment {
    fn from(a: AttachmentInput) -> Self {
        Self {
            name: a.name,
            mime: a.mime,
            data_base64: a.data_base64,
        }
    }
}

const MAX_ATTACHMENTS: usize = 5;
const MAX_TEXT_BYTES: usize = 200_000;
const MAX_IMAGE_BYTES: usize = 5_000_000;
const BLIND_MODEL: &str =
    "This model cannot read images. Pick a model that can, or remove the image.";

fn is_image(a: &StoredAttachment) -> bool {
    a.mime.starts_with("image/")
}

fn decode(a: &StoredAttachment) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(&a.data_base64)
        .ok()
}

/// The web view is not a trust boundary: check count, size and kind here.
fn validate(attachments: &[StoredAttachment]) -> Result<(), UiError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(UiError::other("You can attach up to 5 files."));
    }
    for a in attachments {
        let unsupported = || UiError::other(&format!("{} is not a text file or image.", a.name));
        let too_large = || UiError::other(&format!("{} is too large to attach.", a.name));
        let bytes = decode(a).ok_or_else(unsupported)?;
        if is_image(a) {
            if bytes.len() > MAX_IMAGE_BYTES {
                return Err(too_large());
            }
        } else {
            if bytes.len() > MAX_TEXT_BYTES {
                return Err(too_large());
            }
            if std::str::from_utf8(&bytes).is_err() {
                return Err(unsupported());
            }
        }
    }
    Ok(())
}

/// The cached model list says this model cannot read images. A model that is
/// not in the cache is given the benefit of the doubt.
fn model_is_blind(s: &AppState, model_id: &str) -> bool {
    ModelCache::load(&s.model_cache_file())
        .and_then(|list| list.into_iter().find(|m| m.id == model_id))
        .is_some_and(|m| !m.accepts_images)
}

/// A fenced block that the contents cannot close early.
fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{text}\n{fence}")
}

fn wire(m: &StoredMessage, with_images: bool) -> WireMessage {
    let role = match m.role.as_str() {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        _ => Role::User,
    };
    let mut parts = vec![Part::Text(m.text.clone())];
    for a in &m.attachments {
        if is_image(a) {
            if with_images {
                parts.push(Part::ImageDataUrl(format!(
                    "data:{};base64,{}",
                    a.mime, a.data_base64
                )));
            }
        } else if let Some(text) = decode(a).and_then(|b| String::from_utf8(b).ok()) {
            parts.push(Part::Text(format!("File: {}\n{}", a.name, fenced(&text))));
        }
    }
    WireMessage { role, parts }
}

fn is_unauthorized(e: &EngineEvent) -> bool {
    match e {
        EngineEvent::Failed { error } => error.kind == "unauthorized",
        EngineEvent::Interrupted { reason } => reason == "unauthorized",
        _ => false,
    }
}

/// Store the user's message, run one turn over the whole chat history, and
/// store the reply (partial replies are marked interrupted) and its usage.
/// Returns when the turn ends; progress arrives through `on_event`.
pub async fn send_message(
    s: &AppState,
    chat_id: &str,
    text: &str,
    attachments: Vec<AttachmentInput>,
    model_id: &str,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Result<(), UiError> {
    if s.store.get_chat(chat_id).map_err(local)?.is_none() {
        return Err(UiError::other("This chat no longer exists."));
    }
    let attachments: Vec<StoredAttachment> = attachments.into_iter().map(Into::into).collect();
    validate(&attachments)?;
    if attachments.iter().any(is_image) && model_is_blind(s, model_id) {
        return Err(UiError::other(BLIND_MODEL));
    }
    s.store
        .append_message(chat_id, "user", text, &attachments, false)
        .map_err(local)?;
    run_stored_turn(s, chat_id, model_id, on_event).await
}

/// Run one turn over the stored history of the chat and store the reply and
/// its usage. The caller has already stored the user message to answer.
async fn run_stored_turn(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Result<(), UiError> {
    s.store.set_chat_model(chat_id, model_id).map_err(local)?;
    let with_images = !model_is_blind(s, model_id);
    let messages = s
        .store
        .get_messages(chat_id)
        .map_err(local)?
        .iter()
        .map(|m| wire(m, with_images))
        .collect();

    let id = s.next_id();
    let cancel = CancellationToken::new();
    let previous = s.turns().insert(
        chat_id.to_string(),
        RunningTurn {
            id,
            cancel: cancel.clone(),
        },
    );
    if let Some(previous) = previous {
        previous.cancel.cancel();
    }

    let unauthorized = AtomicBool::new(false);
    let emit = |e: EngineEvent| {
        if is_unauthorized(&e) {
            unauthorized.store(true, Ordering::SeqCst);
        }
        on_event(e);
    };
    let request = TurnRequest {
        model_id: model_id.to_string(),
        messages,
    };
    let out = s.engine.run_turn(request, &emit, cancel).await;

    {
        let mut turns = s.turns();
        if turns.get(chat_id).is_some_and(|t| t.id == id) {
            turns.remove(chat_id);
        }
    }
    if unauthorized.load(Ordering::SeqCst) {
        revoke(s);
    }
    if !out.text.is_empty() {
        s.store
            .append_message(chat_id, "assistant", &out.text, &[], out.interrupted)
            .map_err(local)?;
    }
    if out.requests > 0 {
        s.store
            .record_usage(chat_id, out.requests, out.cost_usd)
            .map_err(local)?;
    }
    Ok(())
}

/// Answers the chat's last user message again without storing it twice. An
/// interrupted partial reply is removed first; a completed one is not.
pub async fn retry_turn(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Result<(), UiError> {
    if s.store.get_chat(chat_id).map_err(local)?.is_none() {
        return Err(UiError::other("This chat no longer exists."));
    }
    let messages = s.store.get_messages(chat_id).map_err(local)?;
    let partial = messages
        .last()
        .filter(|m| m.role == "assistant" && m.interrupted);
    let answered = &messages[..messages.len() - usize::from(partial.is_some())];
    if answered.last().is_none_or(|m| m.role != "user") {
        return Err(UiError::other("There is nothing to retry in this chat."));
    }
    if let Some(partial) = partial {
        s.store
            .delete_message(chat_id, &partial.id)
            .map_err(local)?;
    }
    run_stored_turn(s, chat_id, model_id, on_event).await
}

/// Stops the reply running in this chat, if any.
pub fn stop(s: &AppState, chat_id: &str) {
    if let Some(turn) = s.turns().get(chat_id) {
        turn.cancel.cancel();
    }
}
