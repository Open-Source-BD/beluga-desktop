//! Sending a message and stopping a reply.

use super::auth::revoke;
use super::chats::local;
use crate::engine::{EngineEvent, TurnRequest, UiError};
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

/// Text only for now; attachments become message parts in a later task.
fn wire(m: &StoredMessage) -> WireMessage {
    let role = match m.role.as_str() {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        _ => Role::User,
    };
    WireMessage {
        role,
        parts: vec![Part::Text(m.text.clone())],
    }
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
    s.store
        .append_message(chat_id, "user", text, &attachments, false)
        .map_err(local)?;
    s.store.set_chat_model(chat_id, model_id).map_err(local)?;
    let messages = s
        .store
        .get_messages(chat_id)
        .map_err(local)?
        .iter()
        .map(wire)
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

/// Stops the reply running in this chat, if any.
pub fn stop(s: &AppState, chat_id: &str) {
    if let Some(turn) = s.turns().get(chat_id) {
        turn.cancel.cancel();
    }
}
