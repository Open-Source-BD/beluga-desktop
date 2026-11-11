//! System dialogs, opened only by the Rust core. A path the web view names
//! is never trusted; a path the user picked in a dialog here is.

use std::path::PathBuf;
use tauri::Runtime;
use tauri_plugin_dialog::DialogExt;

pub trait Dialogs: Send + Sync {
    /// Asks the user for a folder; `None` when they cancel. Blocks until the
    /// dialog closes, so call it off the main thread.
    fn pick_folder(&self) -> Option<PathBuf>;
    /// Asks the user where to save a Markdown file; `None` when they cancel.
    fn save_markdown(&self, file_name: &str) -> Option<PathBuf>;
    /// Asks the user for a `.zip` file; `None` when they cancel.
    fn pick_zip(&self) -> Option<PathBuf>;
}

/// The real dialogs. The plugin's callback form is used and waited on from a
/// worker thread, never the main one, so the event loop is not blocked.
pub struct TauriDialogs<R: Runtime>(pub tauri::AppHandle<R>);

impl<R: Runtime> Dialogs for TauriDialogs<R> {
    fn pick_folder(&self) -> Option<PathBuf> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.0.dialog().file().pick_folder(move |p| {
            let _ = tx.send(p);
        });
        rx.recv().ok().flatten()?.into_path().ok()
    }

    fn save_markdown(&self, file_name: &str) -> Option<PathBuf> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.0
            .dialog()
            .file()
            .add_filter("Markdown", &["md"])
            .set_file_name(file_name)
            .save_file(move |p| {
                let _ = tx.send(p);
            });
        rx.recv().ok().flatten()?.into_path().ok()
    }

    fn pick_zip(&self) -> Option<PathBuf> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.0
            .dialog()
            .file()
            .add_filter("ZIP", &["zip"])
            .pick_file(move |p| {
                let _ = tx.send(p);
            });
        rx.recv().ok().flatten()?.into_path().ok()
    }
}
