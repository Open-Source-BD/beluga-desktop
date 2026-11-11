pub mod auth;
pub mod brand;
pub mod commands;
pub mod engine;
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
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let state = state::AppState::open(
                data_dir,
                builtin_skills_dir(app.handle()),
                Arc::new(app.handle().clone()),
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
            h::stop,
            h::skills_status,
            h::list_packs,
            h::get_skill,
            h::set_skill_on,
            h::set_pack_on,
            h::skill_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
