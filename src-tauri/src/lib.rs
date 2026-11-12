pub mod agent;
pub mod auth;
pub mod brand;
pub mod commands;
pub mod dialogs;
pub mod engine;
pub mod folder;
pub mod openrouter;
pub mod secrets;
pub mod skills;
pub mod state;
pub mod store;

use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use commands::handlers as h;
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let state = state::AppState::open(
                data_dir,
                builtin_skills_dir(app.handle()),
                Arc::new(app.handle().clone()),
                Arc::new(dialogs::TauriDialogs(app.handle().clone())),
            )?;
            app.manage(Arc::new(state));
            // The window takes its title from the one place the name lives.
            if let Some(window) = app.get_webview_window("main") {
                window.set_title(brand::APP_NAME)?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            h::auth_status,
            h::sign_in_start,
            h::sign_in_code_url,
            h::sign_in_cancel,
            h::sign_in_with_code,
            h::sign_in_with_key,
            h::resolve_key_storage,
            h::sign_out,
            h::key_status,
            h::list_models,
            h::list_chats,
            h::get_messages,
            h::create_chat,
            h::rename_chat,
            h::delete_chat,
            h::chat_usage,
            h::month_spend,
            h::send_message,
            h::retry_turn,
            h::take_user_message,
            h::peek_user_message,
            h::rows_after,
            h::compact_chat,
            h::export_chat_with_dialog,
            h::stop,
            h::skills_status,
            h::list_packs,
            h::get_skill,
            h::set_skill_on,
            h::set_pack_on,
            h::skill_file,
            h::pack_licenses,
            h::chat_state,
            h::activate_skill,
            h::deactivate_skill,
            h::chat_fullness,
            h::set_chat_folder,
            h::choose_chat_folder,
            h::approved_folders,
            h::folder_files,
            h::attach_folder_file,
            h::extra_packs,
            h::install_extra_pack,
            h::remove_pack,
            h::preview_add,
            h::confirm_add,
            h::cancel_add,
            h::remove_skill,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // `Exit` is the last event of every normal quit (the menu or
            // shortcut on macOS, the last window closing on Windows and
            // Linux, an explicit `exit`); `ExitRequested` can be vetoed and a
            // closed window is not a quit on macOS, so neither is used. The
            // runtime is still alive here, so the stop can be awaited.
            if let tauri::RunEvent::Exit = event {
                stop_background_commands(app);
            }
        });
}

/// Ends every chat's background commands before the process goes, waiting
/// for the stops (each is bounded to about two seconds).
fn stop_background_commands(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<Arc<state::AppState>>() else {
        return;
    };
    let state = state.inner().clone();
    tauri::async_runtime::block_on(async move {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            state.background.stop_all(),
        )
        .await;
    });
}

/// The bundled skill tree: `<resource dir>/skills` in a built app. A dev run
/// may not have copied it yet, so fall back to the source tree.
fn builtin_skills_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    let bundled = app.path().resource_dir().map(|d| d.join("skills"));
    match bundled {
        Ok(dir) if dir.is_dir() => dir,
        _ if cfg!(debug_assertions) => {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")
        }
        Ok(dir) => dir,
        // Never a relative path: that would read from the current directory.
        Err(_) => std::path::PathBuf::from("/nonexistent/builtin-skills"),
    }
}
