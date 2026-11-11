//! Sending a message and stopping a reply.

use super::auth::revoke;
use super::chat_state::{SKILL_NOTE, skills_to_send};
use super::chats::local;
use crate::engine::{EngineEvent, TurnOutcome, TurnRequest, UiError};
use crate::openrouter::models::ModelCache;
use crate::openrouter::stream::{Part, Role, WireMessage};
use crate::state::{AppState, RunningTurn};
use crate::store::{StoredAttachment, StoredMessage};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
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

impl From<StoredAttachment> for AttachmentInput {
    fn from(a: StoredAttachment) -> Self {
        Self {
            name: a.name,
            mime: a.mime,
            data_base64: a.data_base64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Done,
    Interrupted,
    Failed,
}

/// How a turn that ran ended, returned only after its reply and usage are
/// stored. `reason` is set for `Interrupted` ("stopped" or a `UiError` kind);
/// `error` is set for `Failed`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnResult {
    pub status: TurnStatus,
    pub reason: Option<String>,
    pub error: Option<UiError>,
}

impl TurnResult {
    pub(super) fn done() -> Self {
        Self {
            status: TurnStatus::Done,
            reason: None,
            error: None,
        }
    }

    pub(super) fn interrupted(reason: String) -> Self {
        Self {
            status: TurnStatus::Interrupted,
            reason: Some(reason),
            error: None,
        }
    }

    pub(super) fn failed(error: UiError) -> Self {
        Self {
            status: TurnStatus::Failed,
            reason: None,
            error: Some(error),
        }
    }

    /// The result a terminal event announces; `None` for other events.
    fn from_event(e: &EngineEvent) -> Option<Self> {
        match e {
            EngineEvent::Done => Some(Self::done()),
            EngineEvent::Interrupted { reason } => Some(Self::interrupted(reason.clone())),
            EngineEvent::Failed { error } => Some(Self::failed(error.clone())),
            _ => None,
        }
    }
}

const MAX_ATTACHMENTS: usize = 5;
pub(super) const MAX_TEXT_BYTES: usize = 200_000;
pub(super) const MAX_IMAGE_BYTES: usize = 5_000_000;
const BLIND_MODEL: &str =
    "This model cannot read images. Pick a model that can, or remove the image.";

pub(super) fn is_image(a: &StoredAttachment) -> bool {
    a.mime.starts_with("image/")
}

pub(super) fn decode(a: &StoredAttachment) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(&a.data_base64)
        .ok()
}

pub(super) fn not_attachable(name: &str) -> UiError {
    UiError::other(&format!("{name} is not a text file or image."))
}

pub(super) fn too_large(name: &str) -> UiError {
    UiError::other(&format!("{name} is too large to attach."))
}

/// The web view is not a trust boundary: check count, size and kind here.
fn validate(attachments: &[StoredAttachment]) -> Result<(), UiError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(UiError::other("You can attach up to 5 files."));
    }
    for a in attachments {
        let unsupported = || not_attachable(&a.name);
        let too_large = || too_large(&a.name);
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

pub(super) fn wire(m: &StoredMessage, with_images: bool) -> WireMessage {
    let role = match m.role.as_str() {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        _ => Role::User,
    };
    // An attachment-only message has no text part rather than an empty one.
    let mut parts = Vec::new();
    if !m.text.is_empty() {
        parts.push(Part::Text(m.text.clone()));
    }
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

pub(super) fn system(text: String) -> WireMessage {
    WireMessage {
        role: Role::System,
        parts: vec![Part::Text(text)],
    }
}

/// The system messages sent ahead of the conversation, in this order:
/// 1. when any skill is given, `SKILL_NOTE`;
/// 2. for each skill `(id, SKILL.md text)`, in the order given (activation
///    order), `Skill: {id}\n\n{text}`;
/// 3. when there is a summary, `Summary of the earlier conversation:\n\n{summary}`.
pub(super) fn preamble(skills: &[(&str, &str)], summary: Option<&str>) -> Vec<WireMessage> {
    let mut out = Vec::new();
    if !skills.is_empty() {
        out.push(system(SKILL_NOTE.to_string()));
    }
    for (id, body) in skills {
        out.push(system(format!("Skill: {id}\n\n{body}")));
    }
    if let Some(summary) = summary {
        out.push(system(format!(
            "Summary of the earlier conversation:\n\n{summary}"
        )));
    }
    out
}

/// Everything a turn on `model_id` sends: the `preamble`, then every
/// sendable message (no notes, nothing before the compaction boundary).
fn wire_history(s: &AppState, chat_id: &str, model_id: &str) -> Result<Vec<WireMessage>, UiError> {
    let skills = skills_to_send(s, chat_id, model_id)?;
    let skills: Vec<(&str, &str)> = skills
        .iter()
        .map(|(id, body)| (id.as_str(), body.as_str()))
        .collect();
    let summary = s
        .store
        .summary(chat_id)
        .map_err(local)?
        .map(|(text, _)| text);
    let with_images = !model_is_blind(s, model_id);
    let mut messages = preamble(&skills, summary.as_deref());
    messages.extend(
        s.store
            .sendable_messages(chat_id)
            .map_err(local)?
            .iter()
            .map(|m| wire(m, with_images)),
    );
    Ok(messages)
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
/// Returns how the turn ended once everything is stored; progress arrives
/// through `on_event`. `Err` means the turn never ran.
pub async fn send_message(
    s: &AppState,
    chat_id: &str,
    text: &str,
    attachments: Vec<AttachmentInput>,
    model_id: &str,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Result<TurnResult, UiError> {
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
) -> Result<TurnResult, UiError> {
    s.store.set_chat_model(chat_id, model_id).map_err(local)?;
    let request = TurnRequest {
        model_id: model_id.to_string(),
        messages: wire_history(s, chat_id, model_id)?,
    };
    let registration =
        register_turn(s, chat_id, Register::Replace).expect("replacing always registers");
    let ran = run_registered(s, chat_id, registration, request, on_event).await;
    // The turn ran and the user message is stored, so a failure to store the
    // reply is reported as a failed turn, not as a send that never happened.
    if let Err(e) = persist(s, chat_id, &ran.outcome) {
        return Ok(TurnResult::failed(e));
    }
    Ok(ran.result)
}

/// What `run_registered` hands back once the engine is done.
pub(super) struct Ran {
    pub outcome: TurnOutcome,
    pub result: TurnResult,
    /// Another turn in the same chat started meanwhile and took its place.
    pub superseded: bool,
}

/// How `register_turn` treats a turn already running in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Register {
    /// Cancel it and take its place (sending and retrying).
    Replace,
    /// Leave it running and register nothing (compaction).
    IfFree,
}

/// Registers a new running turn for the chat, checking for and replacing an
/// existing one in a single critical section. Returns its id and
/// cancellation token, or `None` when `IfFree` finds the chat busy, in which
/// case nothing changes.
pub(super) fn register_turn(
    s: &AppState,
    chat_id: &str,
    mode: Register,
) -> Option<(u64, CancellationToken)> {
    let mut turns = s.turns();
    if mode == Register::IfFree && turns.contains_key(chat_id) {
        return None;
    }
    let id = s.next_id();
    let cancel = CancellationToken::new();
    let previous = turns.insert(
        chat_id.to_string(),
        RunningTurn {
            id,
            cancel: cancel.clone(),
        },
    );
    if let Some(previous) = previous {
        previous.cancel.cancel();
    }
    Some((id, cancel))
}

/// Runs one engine request as the chat's running turn, registered by
/// `register_turn`: `stop` cancels it, and a turn started in the same chat
/// meanwhile cancels and supersedes it. Signs out when OpenRouter rejects
/// the key. Stores nothing.
pub(super) async fn run_registered(
    s: &AppState,
    chat_id: &str,
    (id, cancel): (u64, CancellationToken),
    request: TurnRequest,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Ran {
    let unauthorized = AtomicBool::new(false);
    let ended: Mutex<Option<TurnResult>> = Mutex::new(None);
    let emit = |e: EngineEvent| {
        if is_unauthorized(&e) {
            unauthorized.store(true, Ordering::SeqCst);
        }
        if let Some(result) = TurnResult::from_event(&e) {
            *ended.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
        }
        on_event(e);
    };
    let outcome = s.engine.run_turn(request, &emit, cancel).await;

    let superseded = {
        let mut turns = s.turns();
        let ours = turns.get(chat_id).is_some_and(|t| t.id == id);
        if ours {
            turns.remove(chat_id);
        }
        !ours
    };
    if unauthorized.load(Ordering::SeqCst) {
        revoke(s);
    }
    let result = ended
        .into_inner()
        .unwrap_or_else(|p| p.into_inner())
        .unwrap_or_else(|| {
            // Defensive: every engine turn ends with a terminal event.
            if outcome.interrupted {
                TurnResult::interrupted("network".to_string())
            } else {
                TurnResult::done()
            }
        });
    Ran {
        outcome,
        result,
        superseded,
    }
}

fn persist(s: &AppState, chat_id: &str, out: &TurnOutcome) -> Result<(), UiError> {
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
/// interrupted partial reply is removed first; a completed one only when
/// `regenerate` is set. Notes are ignored: only sendable messages count as
/// "last".
pub async fn retry_turn(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    regenerate: bool,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Result<TurnResult, UiError> {
    if s.store.get_chat(chat_id).map_err(local)?.is_none() {
        return Err(UiError::other("This chat no longer exists."));
    }
    let messages = s.store.sendable_messages(chat_id).map_err(local)?;
    let replaced = messages
        .last()
        .filter(|m| m.role == "assistant" && (m.interrupted || regenerate));
    let answered = &messages[..messages.len() - usize::from(replaced.is_some())];
    if answered.last().is_none_or(|m| m.role != "user") {
        return Err(UiError::other("There is nothing to retry in this chat."));
    }
    if let Some(replaced) = replaced {
        s.store
            .delete_message(chat_id, &replaced.id)
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
