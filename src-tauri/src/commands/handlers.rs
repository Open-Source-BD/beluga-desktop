//! Thin `#[tauri::command]` wrappers over the plain command functions.
//! Argument names arrive from the web view in camelCase (`chatId`), as
//! Tauri converts them by default.

use super::auth::{AuthStatus, AuthUrl, StorageChoice};
use super::chat_state::{ChatState, Fullness};
use super::chats::ChatUsage;
use super::skills::SkillsStatus;
use super::turn::{AttachmentInput, TurnResult};
use super::{auth, chat_state, chats, models, skills, turn};
use crate::auth::callback::PRODUCTION_TIMEOUT;
use crate::engine::{EngineEvent, UiError};
use crate::openrouter::key_info::KeyStatus;
use crate::openrouter::models::ModelInfo;
use crate::skills::library::{PackView, SkillDetail};
use crate::state::AppState;
use crate::store::{ChatSummary, StoredMessage};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;

type S<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub async fn auth_status(state: S<'_>) -> Result<AuthStatus, UiError> {
    Ok(auth::auth_status(&state))
}

/// Opens the browser itself. If that fails the URL is still returned, so the
/// web view can offer it.
#[tauri::command]
pub async fn sign_in_start<R: Runtime>(
    app: AppHandle<R>,
    state: S<'_>,
) -> Result<AuthUrl, UiError> {
    let url = auth::sign_in_start(&state, PRODUCTION_TIMEOUT).await?;
    let _ = app.opener().open_url(url.auth_url.as_str(), None::<&str>);
    Ok(url)
}

#[tauri::command]
pub async fn sign_in_code_url(state: S<'_>) -> Result<AuthUrl, UiError> {
    Ok(auth::sign_in_code_url(&state))
}

#[tauri::command]
pub async fn sign_in_cancel(state: S<'_>) -> Result<(), UiError> {
    auth::sign_in_cancel(&state);
    Ok(())
}

#[tauri::command]
pub async fn sign_in_with_code(state: S<'_>, code: String) -> Result<(), UiError> {
    auth::sign_in_with_code(&state, &code).await
}

#[tauri::command]
pub async fn sign_in_with_key(state: S<'_>, key: String) -> Result<(), UiError> {
    auth::sign_in_with_key(&state, &key).await
}

#[tauri::command]
pub async fn resolve_key_storage(state: S<'_>, choice: StorageChoice) -> Result<(), UiError> {
    auth::resolve_key_storage(&state, choice)
}

#[tauri::command]
pub async fn sign_out(state: S<'_>) -> Result<(), UiError> {
    auth::sign_out(&state)
}

#[tauri::command]
pub async fn key_status(state: S<'_>) -> Result<KeyStatus, UiError> {
    auth::key_status(&state).await
}

#[tauri::command]
pub async fn list_models(state: S<'_>, force: bool) -> Result<Vec<ModelInfo>, UiError> {
    models::list_models(&state, force).await
}

#[tauri::command]
pub async fn list_chats(state: S<'_>) -> Result<Vec<ChatSummary>, UiError> {
    chats::list_chats(&state)
}

#[tauri::command]
pub async fn get_messages(state: S<'_>, chat_id: String) -> Result<Vec<StoredMessage>, UiError> {
    chats::get_messages(&state, &chat_id)
}

#[tauri::command]
pub async fn create_chat(state: S<'_>, model_id: String) -> Result<ChatSummary, UiError> {
    chats::create_chat(&state, &model_id)
}

#[tauri::command]
pub async fn rename_chat(state: S<'_>, id: String, title: String) -> Result<(), UiError> {
    chats::rename_chat(&state, &id, &title)
}

#[tauri::command]
pub async fn delete_chat(state: S<'_>, id: String) -> Result<(), UiError> {
    chats::delete_chat(&state, &id)
}

#[tauri::command]
pub async fn chat_usage(state: S<'_>, chat_id: String) -> Result<ChatUsage, UiError> {
    chats::chat_usage(&state, &chat_id)
}

#[tauri::command]
pub async fn month_spend(state: S<'_>) -> Result<f64, UiError> {
    chats::month_spend(&state)
}

#[tauri::command]
pub async fn send_message(
    state: S<'_>,
    chat_id: String,
    text: String,
    attachments: Vec<AttachmentInput>,
    model_id: String,
    on_event: Channel<EngineEvent>,
) -> Result<TurnResult, UiError> {
    let emit = move |e: EngineEvent| {
        let _ = on_event.send(e);
    };
    turn::send_message(&state, &chat_id, &text, attachments, &model_id, &emit).await
}

#[tauri::command]
pub async fn retry_turn(
    state: S<'_>,
    chat_id: String,
    model_id: String,
    on_event: Channel<EngineEvent>,
) -> Result<TurnResult, UiError> {
    let emit = move |e: EngineEvent| {
        let _ = on_event.send(e);
    };
    turn::retry_turn(&state, &chat_id, &model_id, &emit).await
}

#[tauri::command]
pub async fn stop(state: S<'_>, chat_id: String) -> Result<(), UiError> {
    turn::stop(&state, &chat_id);
    Ok(())
}

#[tauri::command]
pub async fn list_packs(state: S<'_>) -> Result<Vec<PackView>, UiError> {
    skills::list_packs(&state)
}

#[tauri::command]
pub async fn get_skill(state: S<'_>, id: String) -> Result<SkillDetail, UiError> {
    skills::get_skill(&state, &id)
}

#[tauri::command]
pub async fn set_skill_on(state: S<'_>, id: String, on: bool) -> Result<(), UiError> {
    skills::set_skill_on(&state, &id, on)
}

#[tauri::command]
pub async fn set_pack_on(state: S<'_>, id: String, on: bool) -> Result<(), UiError> {
    skills::set_pack_on(&state, &id, on)
}

#[tauri::command]
pub async fn skill_file(
    state: S<'_>,
    id: String,
    file: String,
) -> Result<AttachmentInput, UiError> {
    skills::skill_file(&state, &id, &file)
}

#[tauri::command]
pub async fn skills_status(state: S<'_>) -> Result<SkillsStatus, UiError> {
    skills::skills_status(&state)
}

#[tauri::command]
pub async fn chat_state(
    state: S<'_>,
    chat_id: String,
    model_id: String,
) -> Result<ChatState, UiError> {
    chat_state::chat_state(&state, &chat_id, &model_id)
}

#[tauri::command]
pub async fn activate_skill(
    state: S<'_>,
    chat_id: String,
    skill_id: String,
    model_id: String,
) -> Result<ChatState, UiError> {
    chat_state::activate_skill(&state, &chat_id, &skill_id, &model_id)
}

#[tauri::command]
pub async fn deactivate_skill(
    state: S<'_>,
    chat_id: String,
    skill_id: String,
) -> Result<ChatState, UiError> {
    chat_state::deactivate_skill(&state, &chat_id, &skill_id)
}

#[tauri::command]
pub async fn chat_fullness(
    state: S<'_>,
    chat_id: String,
    model_id: String,
    draft_chars: u64,
) -> Result<Fullness, UiError> {
    chat_state::chat_fullness(&state, &chat_id, &model_id, draft_chars)
}
