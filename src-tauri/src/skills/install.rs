//! The extra packs that are not bundled: listing them, installing one at its
//! pinned commit after checking its recorded checksum, and removing one.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use super::archive::{Limits, download_github_archive};
use super::pack::{PackSpec, copy_pack, tree_checksum};
use super::staging::{Stage, swap_into};
use crate::engine::UiError;

/// Installed extra packs live in `<app data>/skills/packs/<id>/`.
pub const PACKS_DIR: &str = "packs";
/// The one extra pack that needs a notice accepted before installing.
pub const NEEDS_WARNING: &str = "claude-red";

pub const DOWNLOAD_FAILED: &str =
    "Could not download from GitHub. Check your connection and the link.";
pub const CHECKSUM_MISMATCH: &str =
    "The downloaded pack did not match its recorded checksum, so it was not installed.";
const COULD_NOT_INSTALL: &str = "Could not install that pack on this computer.";
const COULD_NOT_REMOVE: &str = "Could not remove that pack on this computer.";
pub const NOT_INSTALLED: &str = "Only packs you installed can be removed.";

fn other(message: &str) -> UiError {
    UiError::other(message)
}

/// A pack id usable as one folder name: `[A-Za-z0-9._-]`, not `.` or `..`.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtraPack {
    pub id: String,
    pub title: String,
    pub about: String,
    pub license: String,
    pub repo: String,
    pub installed: bool,
    pub needs_warning: bool,
}

/// The lock's non-bundled packs, in lock order.
pub fn extra_packs(lock: &[PackSpec], skills_dir: &Path) -> Vec<ExtraPack> {
    lock.iter()
        .filter(|p| !p.bundled)
        .map(|p| ExtraPack {
            id: p.id.clone(),
            title: p.title.clone(),
            about: p.about.clone(),
            license: p.license.clone(),
            repo: p.repo.clone(),
            installed: safe_id(&p.id) && skills_dir.join(PACKS_DIR).join(&p.id).is_dir(),
            needs_warning: p.id == NEEDS_WARNING,
        })
        .collect()
}

/// Downloads `spec` at its commit, copies its text files, checks the copy
/// against `spec.checksum` and moves it to `packs/<id>/`. `write` is held
/// only while the copy is moved into place.
pub async fn install_pack(
    http: &reqwest::Client,
    codeload: &str,
    spec: &PackSpec,
    skills_dir: &Path,
    write: &Mutex<()>,
) -> Result<(), UiError> {
    if !safe_id(&spec.id) {
        return Err(other(COULD_NOT_INSTALL));
    }
    // Everything below lives in the stage until the last step, and the stage
    // is removed on every return.
    let stage = Stage::new(skills_dir).map_err(|_| other(COULD_NOT_INSTALL))?;
    let top = download_github_archive(
        http,
        codeload,
        &spec.repo,
        &spec.commit,
        &stage.join("download"),
        &Limits::default(),
    )
    .await
    .map_err(|_| other(DOWNLOAD_FAILED))?;
    let pack = stage.join("pack");
    let (owned_spec, owned_pack) = (spec.clone(), pack.clone());
    let checksum = tokio::task::spawn_blocking(move || {
        copy_pack(&top, &owned_spec, &owned_pack, &HashMap::new())
            .map(|_| tree_checksum(&owned_pack))
    })
    .await
    .map_err(|_| other(COULD_NOT_INSTALL))?
    .map_err(|_| other(COULD_NOT_INSTALL))?;
    if spec.checksum.is_empty() || checksum != spec.checksum {
        return Err(other(CHECKSUM_MISMATCH));
    }
    let _held = write.lock().unwrap_or_else(|e| e.into_inner());
    let packs = skills_dir.join(PACKS_DIR);
    fs::create_dir_all(&packs).map_err(|_| other(COULD_NOT_INSTALL))?;
    swap_into(&pack, &packs.join(&spec.id), &stage.join("old"))
        .map_err(|_| other(COULD_NOT_INSTALL))?;
    Ok(())
}

/// Deletes `packs/<id>/`: moved into a stage first, so a partly deleted pack
/// is never left where the library reads it.
pub fn remove_pack(skills_dir: &Path, id: &str) -> Result<(), UiError> {
    let dir = skills_dir.join(PACKS_DIR).join(id);
    if !safe_id(id) || !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err(other(NOT_INSTALLED));
    }
    let stage = Stage::new(skills_dir).map_err(|_| other(COULD_NOT_REMOVE))?;
    fs::rename(&dir, stage.join("old")).map_err(|_| other(COULD_NOT_REMOVE))?;
    Ok(())
}
