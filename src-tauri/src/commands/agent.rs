//! What the interface calls to work with project tasks: answer a card, read
//! a chat's task state, manage background commands, saved command rules and
//! the spending limit.
//!
//! The web view is not a trust boundary. Every function here checks its
//! arguments against what the core holds (the chat exists, the card is the
//! one waiting, the command belongs to the chat, the rule is saved, the limit
//! is acceptable) and refuses anything else with a plain `UiError`, changing
//! nothing. No path from the web view is ever opened: the folder given to
//! `remove_project_rule` is only compared as text with the stored rule keys.

use serde::Serialize;

use super::chat_state::require_chat;
use super::chats::local;
use crate::agent::background::BgInfo;
use crate::agent::paths::Project;
use crate::agent::plan::PlanItem;
use crate::agent::{Answer, Approval, guards};
use crate::engine::UiError;
use crate::state::AppState;

const NO_RULES: &str = "There are no saved command rules for that folder.";
const NO_SPEND: &str = "Could not read today's spending on this computer.";
const NO_RULE: &str = "That command rule is not saved for that folder.";

/// Everything the interface needs to draw a chat's task after a reload.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskState {
    pub plan: Vec<PlanItem>,
    pub edits_allowed: bool,
    pub background: Vec<BgInfo>,
    /// The chat has a folder and it still exists.
    pub project: bool,
    /// The card the core is waiting on in this chat, if any.
    pub waiting: Option<Approval>,
    /// A turn is running for this chat (the interface may have lost its event channel to it).
    pub running: bool,
}

/// The command rules saved for one folder, keyed as the dispatcher keys them
/// (the real path without a Windows `\\?\` prefix).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectRules {
    pub folder: String,
    pub rules: Vec<String>,
}

/// `limit_usd` is `None` when there is no limit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpendLimit {
    pub limit_usd: Option<f64>,
    pub spent_today_usd: f64,
}

/// Delivers the user's answer to the card the core is waiting on in the chat.
/// An id that is not that card's, an answer the card does not offer, and a
/// chat with no waiting card are refused.
pub fn answer_approval(
    s: &AppState,
    chat_id: &str,
    approval_id: &str,
    answer: Answer,
) -> Result<(), UiError> {
    require_chat(s, chat_id)?;
    s.approvals.answer(chat_id, approval_id, answer)
}

/// The card waiting in the chat, to draw it again after a reload.
pub fn waiting_approval(s: &AppState, chat_id: &str) -> Result<Option<Approval>, UiError> {
    require_chat(s, chat_id)?;
    Ok(s.approvals.waiting(chat_id))
}

/// The chat has a folder that still exists.
pub(crate) fn has_project(s: &AppState, chat_id: &str) -> Result<bool, UiError> {
    let folder = s.store.folder(chat_id).map_err(local)?;
    Ok(folder.is_some_and(|f| Project::open(&f).is_ok()))
}

pub fn task_state(s: &AppState, chat_id: &str) -> Result<TaskState, UiError> {
    require_chat(s, chat_id)?;
    // A plan that does not parse (the app never writes one) is an empty one.
    let plan = s
        .store
        .plan(chat_id)
        .map_err(local)
        .map(|json| serde_json::from_str::<Vec<PlanItem>>(&json).unwrap_or_default())?;
    Ok(TaskState {
        plan,
        edits_allowed: s.store.edits_allowed(chat_id).map_err(local)?,
        background: s.background.list(chat_id),
        project: has_project(s, chat_id)?,
        waiting: s.approvals.waiting(chat_id),
        running: s.turns().contains_key(chat_id),
    })
}

/// Switches "all edits in this chat" off, so the next edit asks again. There
/// is no command that switches it on: only the user's "all edits" answer to
/// an edit card does that.
pub fn revoke_edits(s: &AppState, chat_id: &str) -> Result<(), UiError> {
    require_chat(s, chat_id)?;
    s.store.set_edits_allowed(chat_id, false).map_err(local)
}

/// All the output kept for the user of one of the chat's background
/// commands (up to 200,000 characters), whether the model read it or not.
pub fn background_output(s: &AppState, chat_id: &str, id: &str) -> Result<String, UiError> {
    require_chat(s, chat_id)?;
    s.background
        .kept(chat_id, id)
        .map_err(|m| UiError::other(&m))
}

/// Stops one of the chat's background commands and everything it started.
pub async fn stop_background(s: &AppState, chat_id: &str, id: &str) -> Result<(), UiError> {
    require_chat(s, chat_id)?;
    s.background
        .stop(chat_id, id)
        .await
        .map_err(|m| UiError::other(&m))
}

/// Every folder with saved command rules, with its rules.
pub fn project_rules(s: &AppState) -> Result<Vec<ProjectRules>, UiError> {
    Ok(s.store
        .all_rules()
        .map_err(local)?
        .into_iter()
        .map(|(folder, rules)| ProjectRules { folder, rules })
        .collect())
}

/// Forgets one saved rule, so the next matching command asks again. `folder`
/// is the text `project_rules` returned; it is only compared with the stored
/// keys, never opened.
pub fn remove_project_rule(s: &AppState, folder: &str, rule: &str) -> Result<(), UiError> {
    let saved = s.store.rules(folder).map_err(local)?;
    if saved.is_empty() {
        return Err(UiError::other(NO_RULES));
    }
    if !saved.iter().any(|r| r == rule) {
        return Err(UiError::other(NO_RULE));
    }
    s.store.remove_rule(folder, rule).map_err(local)
}

pub fn spend_limit(s: &AppState) -> Result<SpendLimit, UiError> {
    Ok(SpendLimit {
        limit_usd: guards::spend_limit(&s.store),
        spent_today_usd: guards::spent_today(&s.store).ok_or_else(|| UiError::other(NO_SPEND))?,
    })
}

/// `None` removes the limit; anything under $0.10 is refused.
pub fn set_spend_limit(s: &AppState, limit_usd: Option<f64>) -> Result<(), UiError> {
    guards::set_spend_limit(&s.store, limit_usd)
}
