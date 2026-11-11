//! Running skills in a chat: which skills are active, whether they fit the
//! model a turn is sent with, and how full the chat is.
//!
//! "Fits" is judged per turn against the model the turn is sent with: the
//! fixed note plus the active skill bodies, at four characters a token, must
//! not exceed half the model's context. Skills are left out from the end of
//! the activation order until the rest fit. A model missing from the cached
//! model list is treated as fitting.

use super::chats::local;
use super::turn::{decode, is_image};
use crate::engine::UiError;
use crate::openrouter::models::{ModelCache, ModelInfo};
use crate::skills::MAX_BODY_CHARS;
use crate::skills::library::SkillView;
use crate::state::AppState;
use serde::Serialize;

/// Sent ahead of the skill bodies whenever any active skill is sent.
pub const SKILL_NOTE: &str = "You are running inside a chat app. You cannot read or change files, run commands, browse, or start helpers. When the instructions below tell you to do any of those, say what you would do and ask the user to do it and paste the result, or work from the files they attach. Do not claim to have done something you cannot do.";

pub const MAX_ACTIVE_SKILLS: usize = 3;

/// What an image attachment counts for in `chat_fullness`, in characters.
const IMAGE_CHARS: usize = 1_000;
const CHARS_PER_TOKEN: usize = 4;

const NO_CHAT: &str = "That chat no longer exists.";
const NOT_AVAILABLE: &str = "This skill is not available. Check it in Skills.";
const TOO_MANY: &str = "A chat can have three skills active. Remove one first.";
const TOO_LONG_TO_RUN: &str = "This skill is too long to run in a chat.";

/// One active skill as the interface shows it for a given model.
/// `unavailable`: switched off, broken or removed; kept in the chat but not sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveSkill {
    pub id: String,
    pub name: String,
    pub too_long: bool,
    pub acts_on_computer: bool,
    pub unavailable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatState {
    pub active_skills: Vec<ActiveSkill>,
    pub folder: Option<String>,
    pub compacted: bool,
}

/// `percent` is not capped at 100. Both are 0 for a model missing from the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fullness {
    pub percent: u32,
    pub context_length: u64,
}

/// A stored active skill, resolved against the library and one model.
struct Resolved {
    id: String,
    view: Option<SkillView>,
    /// The whole SKILL.md text; `None` when the skill is not available.
    body: Option<String>,
    too_long: bool,
}

impl Resolved {
    /// The body when this skill is sent with the turn.
    fn sent_body(&self) -> Option<&str> {
        self.body.as_deref().filter(|_| !self.too_long)
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn too_long_for(total_chars: usize, model: &ModelInfo) -> bool {
    (total_chars / CHARS_PER_TOKEN) as u64 > model.context_length / 2
}

fn require_chat(s: &AppState, chat_id: &str) -> Result<(), UiError> {
    match s.store.get_chat(chat_id).map_err(local)? {
        Some(_) => Ok(()),
        None => Err(UiError::other(NO_CHAT)),
    }
}

fn cached_model(s: &AppState, model_id: &str) -> Option<ModelInfo> {
    ModelCache::load(&s.model_cache_file())?
        .into_iter()
        .find(|m| m.id == model_id)
}

fn skill_view(s: &AppState, id: &str) -> Option<SkillView> {
    let (pack, entry) = s.library.skill(&s.store, id)?;
    pack.skills.into_iter().find(|v| v.id == entry.id)
}

/// The skill's text when it can be run now.
fn available_body(s: &AppState, view: Option<&SkillView>) -> Option<String> {
    let view = view.filter(|v| v.available)?;
    s.library.skill_body(&view.id).ok()
}

/// The given skill ids in order, each with its body when available, marked
/// too long from the end until the note and the remaining bodies fit `model`.
fn resolve(s: &AppState, ids: &[String], model: Option<&ModelInfo>) -> Vec<Resolved> {
    let mut out: Vec<Resolved> = ids
        .iter()
        .map(|id| {
            let view = skill_view(s, id);
            let body = available_body(s, view.as_ref());
            Resolved {
                id: id.clone(),
                view,
                body,
                too_long: false,
            }
        })
        .collect();
    if let Some(model) = model {
        let mut total = chars(SKILL_NOTE)
            + out
                .iter()
                .filter_map(|r| r.body.as_deref())
                .map(chars)
                .sum::<usize>();
        for r in out.iter_mut().rev() {
            if !too_long_for(total, model) {
                break;
            }
            if let Some(body) = &r.body {
                total -= chars(body);
                r.too_long = true;
            }
        }
    }
    out
}

fn resolve_chat(s: &AppState, chat_id: &str, model_id: &str) -> Result<Vec<Resolved>, UiError> {
    let ids = s.store.active_skills(chat_id).map_err(local)?;
    Ok(resolve(s, &ids, cached_model(s, model_id).as_ref()))
}

/// `(id, SKILL.md text)` of each active skill sent with a turn on `model_id`,
/// in activation order.
pub(super) fn skills_to_send(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
) -> Result<Vec<(String, String)>, UiError> {
    Ok(resolve_chat(s, chat_id, model_id)?
        .into_iter()
        .filter_map(|r| Some((r.id.clone(), r.sent_body()?.to_string())))
        .collect())
}

fn state(s: &AppState, chat_id: &str, model_id: &str) -> Result<ChatState, UiError> {
    let active_skills = resolve_chat(s, chat_id, model_id)?
        .into_iter()
        .map(|r| ActiveSkill {
            name: r
                .view
                .as_ref()
                .map_or_else(|| r.id.clone(), |v| v.name.clone()),
            acts_on_computer: r.view.as_ref().is_some_and(|v| v.acts_on_computer),
            unavailable: r.body.is_none(),
            too_long: r.too_long,
            id: r.id,
        })
        .collect();
    Ok(ChatState {
        active_skills,
        folder: s.store.folder(chat_id).map_err(local)?,
        compacted: s.store.summary(chat_id).map_err(local)?.is_some(),
    })
}

pub fn chat_state(s: &AppState, chat_id: &str, model_id: &str) -> Result<ChatState, UiError> {
    require_chat(s, chat_id)?;
    state(s, chat_id, model_id)
}

/// Adds the skill to the end of the chat's active skills and notes "Using
/// {id}" in the thread. Activating an active skill changes nothing.
pub fn activate_skill(
    s: &AppState,
    chat_id: &str,
    skill_id: &str,
    model_id: &str,
) -> Result<ChatState, UiError> {
    require_chat(s, chat_id)?;
    let mut ids = s.store.active_skills(chat_id).map_err(local)?;
    if ids.iter().any(|id| id == skill_id) {
        return state(s, chat_id, model_id);
    }
    let view = skill_view(s, skill_id);
    if available_body(s, view.as_ref()).is_none() {
        return Err(UiError::other(NOT_AVAILABLE));
    }
    if ids.len() >= MAX_ACTIVE_SKILLS {
        return Err(UiError::other(TOO_MANY));
    }
    if view.is_some_and(|v| v.body_chars as usize > MAX_BODY_CHARS) {
        return Err(UiError::other(TOO_LONG_TO_RUN));
    }
    ids.push(skill_id.to_string());
    if let Some(model) = cached_model(s, model_id)
        && resolve(s, &ids, Some(&model))
            .last()
            .is_some_and(|r| r.too_long)
    {
        return Err(UiError::other(&format!(
            "This skill is too long for {}. Pick a model with a larger limit, or remove another active skill.",
            model.name
        )));
    }
    s.store.set_active_skills(chat_id, &ids).map_err(local)?;
    s.store
        .append_note(chat_id, &format!("Using {skill_id}"))
        .map_err(local)?;
    state(s, chat_id, model_id)
}

/// Removes the skill from the chat's active skills from the next turn on.
pub fn deactivate_skill(s: &AppState, chat_id: &str, skill_id: &str) -> Result<ChatState, UiError> {
    require_chat(s, chat_id)?;
    let mut ids = s.store.active_skills(chat_id).map_err(local)?;
    ids.retain(|id| id != skill_id);
    s.store.set_active_skills(chat_id, &ids).map_err(local)?;
    let model_id = s
        .store
        .get_chat(chat_id)
        .map_err(local)?
        .map(|c| c.model_id)
        .unwrap_or_default();
    state(s, chat_id, &model_id)
}

/// How much of `model_id`'s context the next turn would use: the fixed note
/// and fitting skill bodies, the summary, every sendable message's text and
/// decoded text attachments, 1,000 characters per image, and the draft, at
/// four characters a token.
pub fn chat_fullness(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    draft_chars: u64,
) -> Result<Fullness, UiError> {
    require_chat(s, chat_id)?;
    let Some(model) = cached_model(s, model_id).filter(|m| m.context_length > 0) else {
        return Ok(Fullness {
            percent: 0,
            context_length: 0,
        });
    };
    let skills = skills_to_send(s, chat_id, model_id)?;
    let mut total = draft_chars as usize;
    if !skills.is_empty() {
        total += chars(SKILL_NOTE) + skills.iter().map(|(_, b)| chars(b)).sum::<usize>();
    }
    if let Some((summary, _)) = s.store.summary(chat_id).map_err(local)? {
        total += chars(&summary);
    }
    for m in s.store.sendable_messages(chat_id).map_err(local)? {
        total += chars(&m.text);
        for a in &m.attachments {
            total += if is_image(a) {
                IMAGE_CHARS
            } else {
                decode(a)
                    .and_then(|b| String::from_utf8(b).ok())
                    .map_or(0, |t| chars(&t))
            };
        }
    }
    let tokens = total as f64 / CHARS_PER_TOKEN as f64;
    Ok(Fullness {
        percent: (tokens / model.context_length as f64 * 100.0).round() as u32,
        context_length: model.context_length,
    })
}
