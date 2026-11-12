//! Thin `#[tauri::command]` wrappers over the plain command functions.
//! Argument names arrive from the web view in camelCase (`chatId`), as
//! Tauri converts them by default.

use super::agent::{ProjectRules, SpendLimit, TaskState};
use super::auth::{AuthStatus, AuthUrl, StorageChoice};
use super::chat_state::{ChatState, Fullness};
use super::chats::{ChatUsage, PeekedMessage, TakenMessage};
use super::install::{AddPreview, AddSource};
use super::skills::{LicenseFile, SkillsStatus};
use super::turn::{AttachmentInput, TurnResult};
use super::{
    agent, auth, blocking, chat_state, chats, compact, export, folder, install, models, skills,
    turn,
};
use crate::agent::{Answer, Approval};
use crate::auth::callback::PRODUCTION_TIMEOUT;
use crate::engine::{EngineEvent, UiError};
use crate::folder::FileList;
use crate::openrouter::key_info::KeyStatus;
use crate::openrouter::models::ModelInfo;
use crate::skills::install::ExtraPack;
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
    chats::delete_chat(&state, &id).await
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
    regenerate: bool,
    on_event: Channel<EngineEvent>,
) -> Result<TurnResult, UiError> {
    let emit = move |e: EngineEvent| {
        let _ = on_event.send(e);
    };
    turn::retry_turn(&state, &chat_id, &model_id, regenerate, &emit).await
}

#[tauri::command]
pub async fn take_user_message(
    state: S<'_>,
    chat_id: String,
    message_id: String,
) -> Result<TakenMessage, UiError> {
    chats::take_user_message(&state, &chat_id, &message_id)
}

#[tauri::command]
pub async fn peek_user_message(
    state: S<'_>,
    chat_id: String,
    message_id: String,
) -> Result<PeekedMessage, UiError> {
    chats::peek_user_message(&state, &chat_id, &message_id)
}

#[tauri::command]
pub async fn rows_after(state: S<'_>, chat_id: String, message_id: String) -> Result<u32, UiError> {
    chats::rows_after(&state, &chat_id, &message_id)
}

#[tauri::command]
pub async fn compact_chat(
    state: S<'_>,
    chat_id: String,
    model_id: String,
    keep: Option<String>,
) -> Result<TurnResult, UiError> {
    compact::compact_chat(&state, &chat_id, &model_id, keep.as_deref()).await
}

#[tauri::command]
pub async fn export_chat_with_dialog(
    state: S<'_>,
    chat_id: String,
    suggested_name: String,
) -> Result<Option<String>, UiError> {
    let s = state.inner().clone();
    blocking(move || export::export_chat_with_dialog(&s, &chat_id, &suggested_name)).await
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
pub async fn pack_licenses(state: S<'_>, id: String) -> Result<Vec<LicenseFile>, UiError> {
    skills::pack_licenses(&state, &id)
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

#[tauri::command]
pub async fn set_chat_folder(
    state: S<'_>,
    chat_id: String,
    path: Option<String>,
) -> Result<(), UiError> {
    folder::set_chat_folder(&state, &chat_id, path).await
}

#[tauri::command]
pub async fn folder_files(
    state: S<'_>,
    chat_id: String,
    query: String,
) -> Result<FileList, UiError> {
    folder::folder_files(&state, &chat_id, &query)
}

#[tauri::command]
pub async fn attach_folder_file(
    state: S<'_>,
    chat_id: String,
    path: String,
) -> Result<AttachmentInput, UiError> {
    folder::attach_folder_file(&state, &chat_id, &path)
}

#[tauri::command]
pub async fn choose_chat_folder(state: S<'_>, chat_id: String) -> Result<Option<String>, UiError> {
    folder::choose_chat_folder(&state, &chat_id).await
}

#[tauri::command]
pub async fn approved_folders(state: S<'_>) -> Result<Vec<String>, UiError> {
    folder::approved_folders(&state)
}

#[tauri::command]
pub async fn extra_packs(state: S<'_>) -> Result<Vec<ExtraPack>, UiError> {
    install::extra_packs(&state)
}

#[tauri::command]
pub async fn install_extra_pack(
    state: S<'_>,
    id: String,
    acknowledged: bool,
) -> Result<PackView, UiError> {
    install::install_extra_pack(&state, &id, acknowledged).await
}

#[tauri::command]
pub async fn remove_pack(state: S<'_>, id: String) -> Result<(), UiError> {
    let s = state.inner().clone();
    blocking(move || install::remove_pack(&s, &id)).await
}

/// Opens the folder or ZIP dialog itself (off the async workers) for those sources.
#[tauri::command]
pub async fn preview_add(state: S<'_>, source: AddSource) -> Result<Option<AddPreview>, UiError> {
    install::preview_add(&state, source).await
}

#[tauri::command]
pub async fn confirm_add(state: S<'_>, token: String) -> Result<PackView, UiError> {
    let s = state.inner().clone();
    blocking(move || install::confirm_add(&s, &token)).await
}

#[tauri::command]
pub async fn cancel_add(state: S<'_>, token: String) -> Result<(), UiError> {
    let s = state.inner().clone();
    blocking(move || install::cancel_add(&s, &token)).await
}

#[tauri::command]
pub async fn remove_skill(state: S<'_>, id: String) -> Result<(), UiError> {
    let s = state.inner().clone();
    blocking(move || install::remove_skill(&s, &id)).await
}

#[tauri::command]
pub async fn answer_approval(
    state: S<'_>,
    chat_id: String,
    approval_id: String,
    answer: Answer,
) -> Result<(), UiError> {
    agent::answer_approval(&state, &chat_id, &approval_id, answer)
}

#[tauri::command]
pub async fn waiting_approval(state: S<'_>, chat_id: String) -> Result<Option<Approval>, UiError> {
    agent::waiting_approval(&state, &chat_id)
}

#[tauri::command]
pub async fn task_state(state: S<'_>, chat_id: String) -> Result<TaskState, UiError> {
    agent::task_state(&state, &chat_id)
}

#[tauri::command]
pub async fn set_edits_allowed(state: S<'_>, chat_id: String, on: bool) -> Result<(), UiError> {
    agent::set_edits_allowed(&state, &chat_id, on)
}

#[tauri::command]
pub async fn background_output(
    state: S<'_>,
    chat_id: String,
    id: String,
) -> Result<String, UiError> {
    agent::background_output(&state, &chat_id, &id)
}

#[tauri::command]
pub async fn stop_background(state: S<'_>, chat_id: String, id: String) -> Result<(), UiError> {
    agent::stop_background(&state, &chat_id, &id).await
}

#[tauri::command]
pub async fn project_rules(state: S<'_>) -> Result<Vec<ProjectRules>, UiError> {
    agent::project_rules(&state)
}

#[tauri::command]
pub async fn remove_project_rule(
    state: S<'_>,
    folder: String,
    rule: String,
) -> Result<(), UiError> {
    agent::remove_project_rule(&state, &folder, &rule)
}

#[tauri::command]
pub async fn spend_limit(state: S<'_>) -> Result<SpendLimit, UiError> {
    agent::spend_limit(&state)
}

#[tauri::command]
pub async fn set_spend_limit(state: S<'_>, limit_usd: Option<f64>) -> Result<(), UiError> {
    agent::set_spend_limit(&state, limit_usd)
}
