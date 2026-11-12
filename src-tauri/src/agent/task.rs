//! The loop of a project task: send a request, store the model's reply at
//! once, run its tool calls one at a time through the dispatcher, store each
//! result at once, and repeat until the model stops calling tools.
//!
//! Every row is stored before the next step begins, and the rows of one reply
//! keep the store's pairing rule: an assistant row, then one result per call,
//! before any user or note row. Whatever stops the loop (Stop, an error, a
//! failed write, a dropped future, a crash) leaves a conversation the next
//! send can repair (`repair`) and send. `history` also never sends a call
//! without a result, should the store hold one anyway.
//!
//! The checks Task 12 adds (spend limit, free allowance, trimming, the pause
//! every 25 requests, going in circles, waiting out a rate limit) are the
//! methods of `Guards`; each lets the task continue until then.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::dispatch::{self, Ctx};
use super::paths::Project;
use super::read::summary_text;
use super::shell::{self, Shell};
use super::{ToolCall, ToolOutcome, ToolStatus, prompt, schema};
use crate::commands::chat_state::skills_to_send;
use crate::commands::chats::local;
use crate::commands::turn::{TurnResult, model_is_blind, preamble_with, system, wire};
use crate::engine::{EngineEvent, StepError, StepRequest, UiError};
use crate::openrouter::models::ModelCache;
use crate::openrouter::stream::{Part, Role, WireMessage};
use crate::state::AppState;
use crate::store::{Store, StoredMessage, StoredToolCall, ToolMeta};

/// The result of a call that never got one (a crash, a quit, a lost task).
pub const INTERRUPTED: &str = "Not run: the task was interrupted.";
/// The result of a call skipped because the user pressed Stop.
pub const STOPPED: &str = "Not run: the task was stopped.";
/// The result of a call skipped because the user answered a card with a message.
pub const USER_SENT: &str = "Not run: the user sent a new message.";
/// The result of a call skipped because a guard paused the task.
pub const PAUSED: &str = "Not run: the task was paused.";
/// A note: the model's reply had no text and no calls.
pub const NOTHING: &str = "The model returned nothing.";
/// A note: the reply's calls were too many or too large to run.
pub const OVERFLOW_NOTE: &str = "The model sent too many tool calls at once; none were run.";
/// Told to the model (not stored) after an overflowed reply.
pub const OVERFLOW_ASK: &str = "Your last reply contained too many or too large tool calls, so none were run. Make at most a few calls at a time.";
/// A note: the project folder went away or was changed during the task.
pub const FOLDER_GONE: &str = "The project folder is no longer available, so the task stopped.";
/// A note: a tool ran but its result could not be stored.
pub const RESULT_NOT_SAVED: &str =
    "A tool's result could not be saved, so the task stopped. The next message records it as lost.";
/// The result of a call skipped because the chat's folder was closed or changed.
pub use super::dispatch::FOLDER_CHANGED;
/// The result of a call that may have started but whose result was never
/// stored (a crash, a quit, a failed write): it may have run.
pub const LOST: &str = "The result of this call was lost; it may have run. Check before repeating.";

/// Overflowed replies in a row that pause the task.
const MAX_OVERFLOWS: u32 = 2;

/// What a guard decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Continue,
    /// Stop and wait for the user; the reason is a `TurnResult` reason.
    Pause(&'static str),
}

/// The checks around each request and call. Task 12 fills these in; until
/// then every one lets the task continue.
#[derive(Debug, Default)]
pub struct Guards {
    /// Unreadable calls in a row; an overflowed reply counts as one.
    pub unreadable_in_a_row: u32,
}

impl Guards {
    /// Before each request, with the task's request count so far: the spend
    /// limit, the free allowance, and trimming `messages` to fit.
    pub fn before_request(&mut self, _requests: u32, _messages: &mut Vec<WireMessage>) -> Next {
        Next::Continue
    }

    /// Every 25 requests (`requests` is a multiple of 25): ask whether to
    /// keep going.
    pub async fn keep_going(&mut self, _requests: u32) -> Next {
        Next::Continue
    }

    /// After each dispatched call: going in circles.
    pub fn after_call(&mut self, _call: &ToolCall, _outcome: &ToolOutcome) -> Next {
        Next::Continue
    }

    /// A request failed: `true` sends it again (a rate limit waited out).
    pub async fn retry_after(&mut self, _error: &StepError) -> bool {
        false
    }
}

/// Requests between two "Keep going?" checks.
const KEEP_GOING_EVERY: u32 = 25;

/// One lock per chat, held by a running project task while it stores rows.
/// A new send in the chat cancels the running task and takes the lock once
/// that task has stored its last row, so the new rows come after all of its.
#[derive(Default)]
pub struct TaskLocks(Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl TaskLocks {
    pub async fn hold(&self, chat_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(chat_id.to_string())
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    /// The chat's lock when no task holds it, without waiting.
    pub fn try_hold(&self, chat_id: &str) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let lock = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(chat_id.to_string())
            .or_default()
            .clone();
        lock.try_lock_owned().ok()
    }

    /// Forgets the chat's lock (the chat was deleted). A task still holding
    /// it keeps its own handle.
    pub fn forget(&self, chat_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(chat_id);
    }

    #[cfg(test)]
    pub fn chats(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// The system's name as the model is told it.
pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    }
}

/// The cached model list says this model accepts tools. A model missing
/// from the cache does not.
fn supports_tools(s: &AppState, model_id: &str) -> bool {
    ModelCache::load(&s.model_cache_file())
        .and_then(|list| list.into_iter().find(|m| m.id == model_id))
        .is_some_and(|m| m.supports_tools)
}

/// The chat's project when a turn on `model_id` is a project task: the chat
/// has a folder that still exists and the model accepts tools.
pub fn project_for(s: &AppState, chat_id: &str, model_id: &str) -> Option<Project> {
    if !supports_tools(s, model_id) {
        return None;
    }
    let folder = s.store.folder(chat_id).ok()??;
    Project::open(&folder).ok()
}

fn status_text(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Ok => "ok",
        ToolStatus::Error => "error",
        ToolStatus::Declined => "declined",
        ToolStatus::NotRun => "not_run",
    }
}

fn lost_meta(name: &str) -> ToolMeta {
    ToolMeta {
        name: name.to_string(),
        status: status_text(ToolStatus::Error).to_string(),
        summary: format!("Lost the result of {}", summary_text(name)),
        detail: None,
    }
}

fn not_run_meta(name: &str) -> ToolMeta {
    ToolMeta {
        name: name.to_string(),
        status: status_text(ToolStatus::NotRun).to_string(),
        summary: format!("Did not run {}", summary_text(name)),
        detail: None,
    }
}

/// Records a result for each call of the chat's last reply that has none (a
/// task that crashed, was quit or failed to store a result): a call marked
/// as started may have acted, so its result is recorded as lost (an error,
/// "it may have run"); one never marked did nothing and is recorded as
/// "Not run: the task was interrupted.". Only the last reply's calls can
/// still be answered under the pairing rule; an older one is covered by
/// `history`. Returns how many were recorded.
pub fn repair(store: &Store, chat_id: &str) -> crate::store::Result<usize> {
    let rows = store.sendable_messages(chat_id)?;
    let Some(last) = rows.iter().rev().find(|m| m.kind != "tool") else {
        return Ok(0);
    };
    let open: Vec<StoredToolCall> = store
        .unanswered_calls(chat_id)?
        .into_iter()
        .filter(|c| last.tool_calls.contains(c))
        .collect();
    for c in &open {
        if store.call_started(chat_id, &c.id)? {
            store.append_tool_result(chat_id, &c.id, &lost_meta(&c.name), LOST)?;
        } else {
            store.append_tool_result(chat_id, &c.id, &not_run_meta(&c.name), INTERRUPTED)?;
        }
        let _ = store.clear_call_started(chat_id, &c.id);
    }
    Ok(open.len())
}

fn tool_message(call_id: &str, text: &str) -> WireMessage {
    WireMessage {
        role: Role::Tool,
        parts: vec![Part::Text(text.to_string())],
        tool_calls: Vec::new(),
        tool_call_id: Some(call_id.to_string()),
    }
}

fn user_message(text: &str) -> WireMessage {
    WireMessage {
        role: Role::User,
        parts: vec![Part::Text(text.to_string())],
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

/// The sendable rows as a project request sends them: assistant rows with
/// their calls, results as `tool` messages. Each call is followed by exactly
/// one result before any other message: a result for a call that is not
/// open is left out, and a call with no stored result gets the lost-result
/// text, since it may have started (never stored by this function).
pub fn history(rows: &[StoredMessage], with_images: bool) -> Vec<WireMessage> {
    fn close(out: &mut Vec<WireMessage>, open: &mut Vec<String>) {
        for id in open.drain(..) {
            out.push(tool_message(&id, LOST));
        }
    }
    let mut out = Vec::new();
    let mut open: Vec<String> = Vec::new();
    for m in rows {
        if m.kind == "tool" {
            let Some(id) = m.tool_call_id.as_deref() else {
                continue;
            };
            if let Some(at) = open.iter().position(|o| o == id) {
                open.remove(at);
                out.push(tool_message(id, &m.text));
            }
            continue;
        }
        close(&mut out, &mut open);
        let mut w = wire(m, with_images);
        if m.role == "assistant" {
            w.tool_calls = m
                .tool_calls
                .iter()
                .map(|c| ToolCall {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect();
            open = m.tool_calls.iter().map(|c| c.id.clone()).collect();
        }
        out.push(w);
    }
    close(&mut out, &mut open);
    out
}

/// The folder's last component, the project's name for the model.
fn project_name(project: &Project) -> String {
    project
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string())
}

/// The system message of a project chat: the same text on every request.
pub fn system_text(project: &Project) -> String {
    prompt::system(os_name(), shell::detect().name, &project_name(project))
}

/// How a task ended.
enum End {
    Done,
    Interrupted(String),
    Failed(UiError),
    Paused(&'static str),
}

fn stopped() -> End {
    End::Interrupted("stopped".to_string())
}

struct Task<'a> {
    s: &'a AppState,
    chat_id: &'a str,
    model_id: &'a str,
    project: Project,
    shell: Shell,
    /// Built once; the same text for every request of the chat.
    system: String,
    emit: &'a (dyn Fn(EngineEvent) + Send + Sync),
    cancel: CancellationToken,
    requests: u32,
    cost_usd: f64,
    /// A row was stored by this task.
    stored: bool,
    overflows_in_a_row: u32,
    /// Tell the model, in the next request only, that its calls were not run.
    tell_overflow: bool,
    guards: Guards,
}

/// Runs a project task in the chat, whose user message (or the state to
/// continue from) is already stored. Emits the task's events and one
/// terminal event (`Done`, `Interrupted`, `Failed` or `Paused`); returns once
/// every row is stored. The caller has registered the turn (`cancel` is its
/// token) and holds the chat's task lock.
pub async fn run_task(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    emit: &(dyn Fn(EngineEvent) + Send + Sync),
    cancel: CancellationToken,
) -> TurnResult {
    let end = match s
        .store
        .folder(chat_id)
        .ok()
        .flatten()
        .map(|f| Project::open(&f))
    {
        Some(Ok(project)) => {
            let system = system_text(&project);
            let mut task = Task {
                s,
                chat_id,
                model_id,
                project,
                shell: shell::detect(),
                system,
                emit,
                cancel,
                requests: 0,
                cost_usd: 0.0,
                stored: false,
                overflows_in_a_row: 0,
                tell_overflow: false,
                guards: Guards::default(),
            };
            task.run().await
        }
        _ => End::Failed(UiError::other(FOLDER_GONE)),
    };
    let (result, event) = match end {
        End::Done => (TurnResult::done(), EngineEvent::Done),
        End::Interrupted(reason) => (
            TurnResult::interrupted(reason.clone()),
            EngineEvent::Interrupted { reason },
        ),
        End::Failed(error) => (
            TurnResult::failed(error.clone()),
            EngineEvent::Failed { error },
        ),
        End::Paused(reason) => (
            TurnResult::paused(reason),
            EngineEvent::Paused {
                reason: reason.to_string(),
            },
        ),
    };
    emit(event);
    result
}

impl Task<'_> {
    async fn run(&mut self) -> End {
        match repair(&self.s.store, self.chat_id) {
            Ok(n) => (0..n).for_each(|_| (self.emit)(EngineEvent::MessageStored)),
            Err(e) => return End::Failed(local(e)),
        }
        loop {
            if self.cancel.is_cancelled() {
                return self.cancelled().await;
            }
            let mut messages = match self.messages() {
                Ok(m) => m,
                Err(e) => return End::Failed(e),
            };
            if self.requests > 0
                && self.requests.is_multiple_of(KEEP_GOING_EVERY)
                && let Next::Pause(reason) = self.guards.keep_going(self.requests).await
            {
                return End::Paused(reason);
            }
            if let Next::Pause(reason) = self.guards.before_request(self.requests, &mut messages) {
                return End::Paused(reason);
            }
            let request = StepRequest {
                model_id: self.model_id.to_string(),
                messages,
                tools: Some(schema::tools()),
                session_id: self.chat_id.to_string(),
            };
            let step = self
                .s
                .engine
                .run_step(request, self.emit, self.cancel.clone())
                .await;
            if step.sent {
                self.count_request(step.cost_usd);
            }

            if self.cancel.is_cancelled() {
                return match self.store_partial(&step.text) {
                    Ok(()) => self.cancelled().await,
                    Err(e) => End::Failed(e),
                };
            }
            if let Some(error) = step.error {
                if self.guards.retry_after(&error).await {
                    continue;
                }
                if let Err(e) = self.store_partial(&step.text) {
                    return End::Failed(e);
                }
                return if self.stored {
                    End::Interrupted(error.ui.kind)
                } else {
                    End::Failed(error.ui)
                };
            }
            // Told once: the request that carried the reminder went out and
            // came back.
            if step.sent {
                self.tell_overflow = false;
            }
            if step.overflowed {
                if let Some(end) = self.overflowed(&step.text) {
                    return end;
                }
                continue;
            }
            self.overflows_in_a_row = 0;
            if step.text.is_empty() && step.tool_calls.is_empty() {
                return match self.note(NOTHING) {
                    Ok(()) => End::Done,
                    Err(e) => End::Failed(e),
                };
            }
            let calls: Vec<StoredToolCall> = step
                .tool_calls
                .iter()
                .map(|c| StoredToolCall {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect();
            // Nothing runs unless the reply that asked for it is stored.
            if let Err(e) = self
                .s
                .store
                .append_assistant(self.chat_id, &step.text, &calls, false)
            {
                return End::Failed(local(e));
            }
            self.stored_row();
            if step.tool_calls.is_empty() {
                return End::Done;
            }
            if let Some(end) = self.run_calls(&step.tool_calls).await {
                return end;
            }
            if !self.folder_unchanged().await {
                return self.folder_gone();
            }
        }
    }

    /// The end of a cancelled task: a folder change (which also cancels)
    /// says so; otherwise the user stopped it.
    async fn cancelled(&mut self) -> End {
        if self.folder_unchanged().await {
            stopped()
        } else {
            self.folder_gone()
        }
    }

    fn folder_gone(&mut self) -> End {
        match self.note(FOLDER_GONE) {
            Ok(()) => End::Interrupted("folder_missing".to_string()),
            Err(e) => End::Failed(e),
        }
    }

    /// Why the remaining calls of a reply must not run, if they must not:
    /// the folder was closed or changed, or the task was stopped. Checked
    /// before each call, so nothing is read in a folder the chat has left.
    async fn halted(&mut self, rest: &[ToolCall]) -> Option<End> {
        if !self.folder_unchanged().await {
            return Some(match self.skip(rest, FOLDER_CHANGED, End::Done) {
                End::Failed(e) => End::Failed(e),
                _ => self.folder_gone(),
            });
        }
        if self.cancel.is_cancelled() {
            return Some(self.skip(rest, STOPPED, stopped()));
        }
        None
    }

    /// Everything one request sends: the system message, the skills note and
    /// skills, the summary, the sendable rows, and the overflow reminder
    /// when one is due.
    fn messages(&self) -> Result<Vec<WireMessage>, UiError> {
        let (s, chat_id) = (self.s, self.chat_id);
        let skills = skills_to_send(s, chat_id, self.model_id)?;
        let skills: Vec<(&str, &str)> = skills
            .iter()
            .map(|(id, body)| (id.as_str(), body.as_str()))
            .collect();
        let summary = s.store.summary(chat_id).map_err(local)?.map(|(t, _)| t);
        let rows = s.store.sendable_messages(chat_id).map_err(local)?;
        // Images are sent unchanged on every request of the task: a stable
        // start of the request (prompt caching) matters more than its size.
        let with_images = !model_is_blind(s, self.model_id);
        let mut out = vec![system(self.system.clone())];
        out.extend(preamble_with(
            prompt::skills_note(true),
            &skills,
            summary.as_deref(),
        ));
        out.extend(history(&rows, with_images));
        if self.tell_overflow {
            out.push(user_message(OVERFLOW_ASK));
        }
        Ok(out)
    }

    /// Records one request's usage and emits the task's running totals. A
    /// usage row that cannot be written does not end the task: the reply
    /// still has to be stored.
    fn count_request(&mut self, cost_usd: Option<f64>) {
        let cost = cost_usd.unwrap_or(0.0);
        self.requests += 1;
        self.cost_usd += cost;
        let _ = self.s.store.record_usage(self.chat_id, 1, cost);
        (self.emit)(EngineEvent::Usage {
            requests: self.requests,
            cost_usd: self.cost_usd,
        });
    }

    fn stored_row(&mut self) {
        self.stored = true;
        (self.emit)(EngineEvent::MessageStored);
    }

    fn note(&mut self, text: &str) -> Result<(), UiError> {
        self.s
            .store
            .append_note(self.chat_id, text)
            .map_err(local)?;
        self.stored_row();
        Ok(())
    }

    /// Text that arrived before a stop or an error, kept as an interrupted
    /// reply (with no calls: calls from a cut reply are never run).
    fn store_partial(&mut self, text: &str) -> Result<(), UiError> {
        if text.is_empty() {
            return Ok(());
        }
        self.s
            .store
            .append_assistant(self.chat_id, text, &[], true)
            .map_err(local)?;
        self.stored_row();
        Ok(())
    }

    /// A reply whose calls were too many or too large: none were returned.
    /// Its text is kept, the user is told in a note, and the next request
    /// tells the model. A second in a row pauses the task.
    fn overflowed(&mut self, text: &str) -> Option<End> {
        self.overflows_in_a_row += 1;
        self.guards.unreadable_in_a_row += 1;
        if !text.is_empty() {
            if let Err(e) = self
                .s
                .store
                .append_assistant(self.chat_id, text, &[], false)
            {
                return Some(End::Failed(local(e)));
            }
            self.stored_row();
        }
        if let Err(e) = self.note(OVERFLOW_NOTE) {
            return Some(End::Failed(e));
        }
        if self.overflows_in_a_row >= MAX_OVERFLOWS {
            return Some(End::Paused("going_in_circles"));
        }
        self.tell_overflow = true;
        None
    }

    /// Runs the reply's calls one at a time, storing each result as it
    /// comes. `None`: carry on with the next request.
    async fn run_calls(&mut self, calls: &[ToolCall]) -> Option<End> {
        let read_log = self.s.read_logs.for_chat(self.chat_id);
        for (i, call) in calls.iter().enumerate() {
            if let Some(end) = self.halted(&calls[i..]).await {
                return Some(end);
            }
            let ctx = Ctx {
                chat_id: self.chat_id,
                project: &self.project,
                shell: &self.shell,
                store: &self.s.store,
                library: &self.s.library,
                approvals: &self.s.approvals,
                background: &self.s.background,
                read_log: &read_log,
                emit: self.emit,
                cancel: &self.cancel,
            };
            let d = dispatch::run_call(&ctx, call).await;
            let meta = ToolMeta {
                name: call.name.clone(),
                status: status_text(d.outcome.status).to_string(),
                summary: d.outcome.summary.clone(),
                detail: d.outcome.detail.clone(),
            };
            if let Err(e) =
                self.s
                    .store
                    .append_tool_result(self.chat_id, &call.id, &meta, &d.outcome.text)
            {
                // The call ran: the next send records its result as lost
                // (and any later call as not run).
                let _ = self.s.store.mark_call_started(self.chat_id, &call.id);
                let _ = self.note(RESULT_NOT_SAVED);
                return Some(End::Failed(local(e)));
            }
            let _ = self.s.store.clear_call_started(self.chat_id, &call.id);
            self.stored_row();
            let rest = &calls[i + 1..];
            if let Some(text) = d.user_message {
                // Every result of the reply comes before the user's row.
                if let End::Failed(e) = self.skip(rest, USER_SENT, End::Done) {
                    return Some(End::Failed(e));
                }
                if let Err(e) = self
                    .s
                    .store
                    .append_message(self.chat_id, "user", &text, &[], false)
                {
                    return Some(End::Failed(local(e)));
                }
                self.stored_row();
                return None;
            }
            if let Some(end) = self.halted(rest).await {
                return Some(end);
            }
            if let Next::Pause(reason) = self.guards.after_call(call, &d.outcome) {
                return Some(self.skip(rest, PAUSED, End::Paused(reason)));
            }
        }
        None
    }

    /// Stores a not-run result with `text` for each call, then ends as
    /// `then` (or as failed when a result cannot be stored).
    fn skip(&mut self, calls: &[ToolCall], text: &str, then: End) -> End {
        for c in calls {
            if let Err(e) =
                self.s
                    .store
                    .append_tool_result(self.chat_id, &c.id, &not_run_meta(&c.name), text)
            {
                return End::Failed(local(e));
            }
            self.stored_row();
        }
        then
    }

    /// The chat's folder is still this task's project and still exists.
    async fn folder_unchanged(&self) -> bool {
        let Ok(Some(folder)) = self.s.store.folder(self.chat_id) else {
            return false;
        };
        let root = self.project.root.clone();
        tokio::task::spawn_blocking(move || std::fs::canonicalize(&folder).is_ok_and(|r| r == root))
            .await
            .unwrap_or(false)
    }
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
