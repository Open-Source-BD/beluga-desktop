//! Sending a message and stopping a reply.

use super::auth::revoke;
use super::chat_state::{SKILL_NOTE, skills_to_send, wire_chars};
use super::chats::local;
use crate::agent::{guards, task};
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
    /// A project task stopped and waits for the user; `reason` says why
    /// (`keep_going_declined`, `no_free_requests`, `daily_limit`,
    /// `chat_full`, `going_in_circles` or `rate_limited`).
    Paused,
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
    pub(crate) fn done() -> Self {
        Self {
            status: TurnStatus::Done,
            reason: None,
            error: None,
        }
    }

    pub(crate) fn interrupted(reason: String) -> Self {
        Self {
            status: TurnStatus::Interrupted,
            reason: Some(reason),
            error: None,
        }
    }

    pub(crate) fn paused(reason: &str) -> Self {
        Self {
            status: TurnStatus::Paused,
            reason: Some(reason.to_string()),
            error: None,
        }
    }

    pub(crate) fn failed(error: UiError) -> Self {
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
pub(crate) fn model_is_blind(s: &AppState, model_id: &str) -> bool {
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

pub(crate) fn wire(m: &StoredMessage, with_images: bool) -> WireMessage {
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
    WireMessage {
        role,
        parts,
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

pub(crate) fn system(text: String) -> WireMessage {
    WireMessage {
        role: Role::System,
        parts: vec![Part::Text(text)],
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

/// The system messages sent ahead of the conversation, in this order:
/// 1. when any skill is given, `SKILL_NOTE`;
/// 2. for each skill `(id, SKILL.md text)`, in the order given (activation
///    order), `Skill: {id}\n\n{text}`;
/// 3. when there is a summary, `Summary of the earlier conversation:\n\n{summary}`.
pub(super) fn preamble(skills: &[(&str, &str)], summary: Option<&str>) -> Vec<WireMessage> {
    preamble_with(SKILL_NOTE, skills, summary)
}

/// `preamble` with `note` in place of `SKILL_NOTE` (a project chat sends
/// its own note).
pub(crate) fn preamble_with(
    note: &str,
    skills: &[(&str, &str)],
    summary: Option<&str>,
) -> Vec<WireMessage> {
    let mut out = Vec::new();
    if !skills.is_empty() {
        out.push(system(note.to_string()));
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

/// Stored rows as plain chat for a request without tools. A chat may hold
/// rows of earlier project tasks, and a model sent `tool` messages or
/// `tool_calls` without tools can reject them, so an assistant row's calls
/// are dropped and each result becomes `[{summary}]` on its own line at the
/// end of the assistant message that made the call. A result with no such
/// message before it is left out, and so is an assistant message left with
/// nothing to say. Rows of a chat that never had tools come out exactly as
/// before.
pub(super) fn plain_history(rows: &[StoredMessage], with_images: bool) -> Vec<WireMessage> {
    // Each message, and whether it is an assistant row that made calls.
    let mut out: Vec<(WireMessage, bool)> = Vec::new();
    let mut after_assistant = false;
    for m in rows {
        if m.kind == "tool" {
            let Some((last, _)) = out.last_mut().filter(|_| after_assistant) else {
                continue;
            };
            let summary = m.tool_meta.as_ref().map_or("", |t| t.summary.as_str());
            match last.parts.iter_mut().find_map(|p| match p {
                Part::Text(t) => Some(t),
                Part::ImageDataUrl(_) => None,
            }) {
                Some(text) => text.push_str(&format!("\n[{summary}]")),
                None => last.parts.insert(0, Part::Text(format!("[{summary}]"))),
            }
        } else {
            after_assistant = m.role == "assistant";
            out.push((wire(m, with_images), !m.tool_calls.is_empty()));
        }
    }
    out.into_iter()
        .filter(|(w, made_calls)| !(*made_calls && w.parts.is_empty()))
        .map(|(w, _)| w)
        .collect()
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
    let rows = s.store.sendable_messages(chat_id).map_err(local)?;
    messages.extend(plain_history(&rows, with_images));
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
    let first = Before::Send {
        text,
        attachments: &attachments,
    };
    if task::project_for(s, chat_id, model_id).is_some() {
        return run_project_turn(s, chat_id, model_id, first, on_event).await;
    }
    let turn = take_over(s, chat_id).await;
    refuse_over_limit(s, model_id)?;
    s.store
        .append_message(chat_id, "user", text, &attachments, false)
        .map_err(local)?;
    run_stored_turn(s, chat_id, model_id, &turn, on_event).await
}

/// Today's spending limit refuses a request on a paid model. Checked once
/// the chat is taken over and before anything is stored or sent, so a
/// refused send stores nothing.
fn refuse_over_limit(s: &AppState, model_id: &str) -> Result<(), UiError> {
    match guards::over_limit_for(s, model_id) {
        Some(refused) => Err(refused),
        None => Ok(()),
    }
}

/// The chat's turn, taken over from whatever ran in it before: registered
/// (a running turn is cancelled), and holding the chat's task lock, so a
/// cancelled project task has stored all of its rows before this turn
/// stores any. In a chat where nothing runs it changes nothing.
struct TakenOver<'a> {
    registered: Registered<'a>,
    cancel: CancellationToken,
    _held: tokio::sync::OwnedMutexGuard<()>,
}

async fn take_over<'a>(s: &'a AppState, chat_id: &'a str) -> TakenOver<'a> {
    let (id, cancel) =
        register_turn(s, chat_id, Register::Replace).expect("replacing always registers");
    let registered = Registered { s, chat_id, id };
    let held = s.task_locks.hold(chat_id).await;
    TakenOver {
        registered,
        cancel,
        _held: held,
    }
}

/// Run one turn over the stored history of the chat and store the reply and
/// its usage. The caller has taken the chat over and already stored the
/// user message to answer.
async fn run_stored_turn(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    turn: &TakenOver<'_>,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Result<TurnResult, UiError> {
    s.store.set_chat_model(chat_id, model_id).map_err(local)?;
    let request = TurnRequest {
        model_id: model_id.to_string(),
        messages: wire_history(s, chat_id, model_id)?,
    };
    let registration = (turn.registered.id, turn.cancel.clone());
    let request_chars = request.messages.iter().map(wire_chars).sum();
    let ran = run_registered(s, chat_id, registration, request, on_event).await;
    // The turn ran and the user message is stored, so a failure to store the
    // reply is reported as a failed turn, not as a send that never happened.
    if let Err(e) = persist(s, chat_id, model_id, request_chars, &ran.outcome) {
        return Ok(TurnResult::failed(e));
    }
    Ok(ran.result)
}

/// What a project turn stores before the task starts.
enum Before<'a> {
    /// The user's new message.
    Send {
        text: &'a str,
        attachments: &'a [StoredAttachment],
    },
    /// A retry, `regenerate` as given to `retry_turn`.
    Retry { regenerate: bool },
}

/// Removes the chat's running turn when it is still this one, also when the
/// turn's future is dropped part way.
struct Registered<'a> {
    s: &'a AppState,
    chat_id: &'a str,
    id: u64,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let mut turns = self.s.turns();
        if turns.get(self.chat_id).is_some_and(|t| t.id == self.id) {
            turns.remove(self.chat_id);
        }
    }
}

const NOTHING_TO_RETRY: &str = "There is nothing to retry in this chat.";

/// In a project chat, the reply a retry removes first, if any. A reply that
/// made calls is never removed; a task that stopped after a call (its last
/// row a call or a result) is continued. `Err` when there is nothing to
/// retry.
fn project_retry(s: &AppState, chat_id: &str, regenerate: bool) -> Result<Option<String>, UiError> {
    let messages = s.store.sendable_messages(chat_id).map_err(local)?;
    let replaced = messages.last().filter(|m| {
        m.role == "assistant" && m.tool_calls.is_empty() && (m.interrupted || regenerate)
    });
    let answered = &messages[..messages.len() - usize::from(replaced.is_some())];
    let continues =
        |m: &StoredMessage| m.role == "user" || m.kind == "tool" || !m.tool_calls.is_empty();
    if !answered.last().is_some_and(continues) {
        return Err(UiError::other(NOTHING_TO_RETRY));
    }
    Ok(replaced.map(|m| m.id.clone()))
}

/// Runs a project task as the chat's turn. The chat is taken over first
/// (`take_over`), so every row of a task that ran before is stored. Then
/// calls left without a result are recorded, the user's message is stored
/// (or, for a retry, the conversation is read and the replaced reply
/// removed), and the loop runs. `Err` means the turn never ran.
async fn run_project_turn(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    before: Before<'_>,
    on_event: &(dyn Fn(EngineEvent) + Send + Sync),
) -> Result<TurnResult, UiError> {
    if let Before::Retry { regenerate } = &before {
        // Nothing to retry leaves a running turn alone.
        project_retry(s, chat_id, *regenerate)?;
    }
    let turn = take_over(s, chat_id).await;
    refuse_over_limit(s, model_id)?;
    s.store.set_chat_model(chat_id, model_id).map_err(local)?;
    let repaired = task::repair(&s.store, chat_id).map_err(local)?;
    (0..repaired).for_each(|_| on_event(EngineEvent::MessageStored));
    match before {
        Before::Send { text, attachments } => {
            s.store
                .append_message(chat_id, "user", text, attachments, false)
                .map_err(local)?;
            on_event(EngineEvent::MessageStored);
        }
        Before::Retry { regenerate } => {
            if let Some(replaced) = project_retry(s, chat_id, regenerate)? {
                s.store.delete_message(chat_id, &replaced).map_err(local)?;
            }
        }
    }
    let unauthorized = AtomicBool::new(false);
    let emit = |e: EngineEvent| {
        if is_unauthorized(&e) {
            unauthorized.store(true, Ordering::SeqCst);
        }
        on_event(e);
    };
    let result = task::run_task(s, chat_id, model_id, &emit, turn.cancel.clone()).await;
    if unauthorized.load(Ordering::SeqCst) {
        revoke(s);
    }
    Ok(result)
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

fn persist(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    request_chars: usize,
    out: &TurnOutcome,
) -> Result<(), UiError> {
    if !out.text.is_empty() {
        s.store
            .append_message(chat_id, "assistant", &out.text, &[], out.interrupted)
            .map_err(local)?;
    }
    if out.requests > 0 {
        // The provider reported no cost: record an estimate for a paid model
        // instead of $0, so the daily limit still counts the request. Used
        // only when no cost is reported.
        let cost = if out.cost_reported || out.text.is_empty() {
            out.cost_usd
        } else {
            guards::estimate_cost(
                guards::model_info(s, model_id).as_ref(),
                out.prompt_tokens,
                request_chars,
                out.text.chars().count(),
            )
        };
        s.store
            .record_usage(chat_id, out.requests, cost)
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
    if task::project_for(s, chat_id, model_id).is_some() {
        let before = Before::Retry { regenerate };
        return run_project_turn(s, chat_id, model_id, before, on_event).await;
    }
    // Nothing to retry leaves a running turn alone; after taking over, the
    // conversation is read again (the turn that ran may have added to it).
    plain_retry(s, chat_id, regenerate)?;
    let turn = take_over(s, chat_id).await;
    let replaced = plain_retry(s, chat_id, regenerate)?;
    refuse_over_limit(s, model_id)?;
    if let Some(replaced) = replaced {
        s.store.delete_message(chat_id, &replaced).map_err(local)?;
    }
    run_stored_turn(s, chat_id, model_id, &turn, on_event).await
}

/// On the one-request path, the reply a retry removes first, if any. `Err`
/// when there is nothing to retry.
fn plain_retry(s: &AppState, chat_id: &str, regenerate: bool) -> Result<Option<String>, UiError> {
    let messages = s.store.sendable_messages(chat_id).map_err(local)?;
    let replaced = messages
        .last()
        .filter(|m| m.role == "assistant" && (m.interrupted || regenerate));
    let answered = &messages[..messages.len() - usize::from(replaced.is_some())];
    if answered.last().is_none_or(|m| m.role != "user") {
        return Err(UiError::other(NOTHING_TO_RETRY));
    }
    Ok(replaced.map(|m| m.id.clone()))
}

/// Stops the reply running in this chat, if any.
pub fn stop(s: &AppState, chat_id: &str) {
    if let Some(turn) = s.turns().get(chat_id) {
        turn.cancel.cancel();
    }
}
