//! Adding your own skills (folder, ZIP or GitHub link) and installing or
//! removing the extra packs. Local paths only ever come from a system dialog
//! the core opens; an add is previewed, then confirmed by token.

use crate::engine::UiError;
use crate::skills::archive::{Limits, download_github_archive, unpack_zip};
use crate::skills::github::{parse_github_url, resolve_commit};
use crate::skills::install::{self as packs, ExtraPack};
use crate::skills::library::{PackView, SkillView};
use crate::skills::staging::{Stage, StagedAdd};
use crate::skills::yours::{self, NO_SKILLS, STAGED_SKILLS, YOURS};
use crate::state::AppState;
use serde::{Deserialize, Serialize};

/// Where an add comes from. Folder and ZIP carry no path: the core asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddSource {
    Folder,
    Zip,
    Github { url: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AddPreview {
    pub token: String,
    pub skills: Vec<SkillView>,
    pub skipped: Vec<String>,
}

const NOT_AVAILABLE: &str = "This pack is not available. Check it in Skills.";
const NOT_A_LINK: &str = "That is not a GitHub link.";
const EXPIRED: &str = "That add has expired. Start again.";
const ACCEPT_FIRST: &str = "Read and accept the notice before installing this pack.";
const COULD_NOT_STAGE: &str = "Could not add those skills on this computer.";
const BAD_ZIP: &str = "That ZIP file could not be read.";
/// A local ZIP may unpack to at most 100 MB.
const ZIP_TOTAL_BYTES: u64 = 100 * 1024 * 1024;

fn switches(_: crate::store::StoreError) -> UiError {
    UiError::other("Could not save your skill settings on this computer.")
}

/// Runs blocking work (a dialog, file copying) off the async workers.
async fn off_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, UiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| UiError::other("That did not finish. Try again."))
}

fn pack_view(s: &AppState, id: &str) -> Result<PackView, UiError> {
    s.library
        .packs(&s.store)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| UiError::other(NOT_AVAILABLE))
}

pub fn extra_packs(s: &AppState) -> Result<Vec<ExtraPack>, UiError> {
    Ok(packs::extra_packs(s.library.lock_specs(), &s.skills_dir()))
}

/// `acknowledged` must be true for the pack that needs a notice; the core
/// checks it rather than trusting the interface to have shown it.
pub async fn install_extra_pack(
    s: &AppState,
    id: &str,
    acknowledged: bool,
) -> Result<PackView, UiError> {
    let spec = s
        .library
        .lock_specs()
        .iter()
        .find(|p| p.id == id && !p.bundled)
        .cloned()
        .filter(|p| p.id != YOURS && !s.library.is_builtin(&p.id))
        .ok_or_else(|| UiError::other(NOT_AVAILABLE))?;
    if spec.id == packs::NEEDS_WARNING && !acknowledged {
        return Err(UiError::other(ACCEPT_FIRST));
    }
    packs::install_pack(
        &s.github_http,
        &s.github.codeload,
        &spec,
        &s.skills_dir(),
        &s.skills_write,
    )
    .await?;
    s.library.reload();
    pack_view(s, id)
}

pub fn remove_pack(s: &AppState, id: &str) -> Result<(), UiError> {
    if id == YOURS || s.library.is_builtin(id) {
        return Err(UiError::other(packs::NOT_INSTALLED));
    }
    {
        let _held = s.skills_write();
        packs::remove_pack(&s.skills_dir(), id)?;
    }
    s.library.reload();
    s.store
        .delete_switch(&format!("pack/{id}"))
        .and_then(|_| s.store.delete_switches_under(&format!("{id}:")))
        .map_err(switches)
}

/// Stages the skills found at `source`; `None` when the user cancels the
/// dialog. Any earlier preview is discarded first.
pub async fn preview_add(s: &AppState, source: AddSource) -> Result<Option<AddPreview>, UiError> {
    drop(s.staged_add().take());
    let new_stage = || Stage::new(&s.skills_dir()).map_err(|_| UiError::other(COULD_NOT_STAGE));
    let (stage, root) = match source {
        AddSource::Folder => {
            let dialogs = s.dialogs.clone();
            let Some(picked) = off_thread(move || dialogs.pick_folder()).await? else {
                return Ok(None);
            };
            let real = std::fs::canonicalize(&picked)
                .ok()
                .filter(|p| p.is_dir())
                .ok_or_else(|| UiError::other(NO_SKILLS))?;
            (new_stage()?, real)
        }
        AddSource::Zip => {
            let dialogs = s.dialogs.clone();
            let Some(picked) = off_thread(move || dialogs.pick_zip()).await? else {
                return Ok(None);
            };
            let stage = new_stage()?;
            let dest = stage.join("unzipped");
            let to = dest.clone();
            let limits = Limits {
                max_total_bytes: ZIP_TOTAL_BYTES,
                ..Limits::default()
            };
            off_thread(move || unpack_zip(&picked, &to, &limits))
                .await?
                .map_err(|_| UiError::other(BAD_ZIP))?;
            (stage, dest)
        }
        AddSource::Github { url } => {
            let link = parse_github_url(&url).ok_or_else(|| UiError::other(NOT_A_LINK))?;
            let failed = || UiError::other(packs::DOWNLOAD_FAILED);
            let sha = resolve_commit(&s.github_http, &s.github.api, &link)
                .await
                .map_err(|_| failed())?;
            let stage = new_stage()?;
            let top = download_github_archive(
                &s.github_http,
                &s.github.codeload,
                &link.repo_path(),
                &sha,
                &stage.join("download"),
                &Limits::default(),
            )
            .await
            .map_err(|_| failed())?;
            let mut root = top;
            if let Some(sub) = &link.sub_path {
                root.extend(sub.split('/'));
            }
            (stage, root)
        }
    };
    let work = stage.path().to_path_buf();
    let staged = off_thread(move || yours::stage_add(&root, &work)).await??;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let preview = AddPreview {
        token: token.clone(),
        skills: staged
            .skills
            .iter()
            .map(|e| SkillView {
                id: e.id.clone(),
                name: e.name.clone(),
                description: e.description.clone(),
                on: true,
                acts_on_computer: e.acts_on_computer,
                broken: e.broken.clone(),
                body_chars: e.body_chars,
                available: e.broken.is_none(),
            })
            .collect(),
        skipped: staged.skipped,
    };
    // Replaces (and so removes) any preview that finished meanwhile.
    *s.staged_add() = Some(StagedAdd { token, stage });
    Ok(Some(preview))
}

/// Takes the staged add if `token` names it; leaves any other in place.
fn take_staged(s: &AppState, token: &str) -> Result<StagedAdd, UiError> {
    let mut slot = s.staged_add();
    match slot.take() {
        Some(add) if add.token == token => Ok(add),
        other => {
            *slot = other;
            Err(UiError::other(EXPIRED))
        }
    }
}

/// Moves the staged skills into "Added by you". The staged copy is removed
/// whether or not this succeeds.
pub fn confirm_add(s: &AppState, token: &str) -> Result<PackView, UiError> {
    let add = take_staged(s, token)?;
    {
        let _held = s.skills_write();
        yours::commit_add(
            &add.stage.join(STAGED_SKILLS),
            &s.skills_dir().join(YOURS),
            add.stage.path(),
        )?;
    }
    drop(add);
    s.library.reload();
    pack_view(s, YOURS)
}

pub fn cancel_add(s: &AppState, token: &str) -> Result<(), UiError> {
    drop(take_staged(s, token)?);
    Ok(())
}

/// Removes one of the user's own skills; any other id is refused.
pub fn remove_skill(s: &AppState, id: &str) -> Result<(), UiError> {
    {
        let _held = s.skills_write();
        let stage = Stage::new(&s.skills_dir())
            .map_err(|_| UiError::other("Could not remove that skill on this computer."))?;
        yours::remove_skill(&s.skills_dir().join(YOURS), id, stage.path())?;
    }
    s.library.reload();
    s.store.delete_switch(id).map_err(switches)
}
