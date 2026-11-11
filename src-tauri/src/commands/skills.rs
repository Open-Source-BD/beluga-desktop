//! The skill library as the interface sees it.

use super::turn::AttachmentInput;
use crate::engine::UiError;
use crate::skills::library::{PackView, SkillDetail};
use crate::state::AppState;
use base64::Engine as _;

const NOT_AVAILABLE: &str = "This skill is not available. Check it in Skills.";

fn local(_: crate::store::StoreError) -> UiError {
    UiError::other("Could not save your skill settings on this computer.")
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SkillsStatus {
    pub builtin_error: Option<String>,
    pub packs: u32,
    pub skills: u32,
    pub on: u32,
}

/// What loaded, so the Skills screen can say when the bundled skills are missing.
pub fn skills_status(s: &AppState) -> Result<SkillsStatus, UiError> {
    let packs = s.library.packs(&s.store);
    let skills = packs.iter().flat_map(|p| &p.skills);
    Ok(SkillsStatus {
        builtin_error: s.library.builtin_error(),
        packs: packs.len() as u32,
        skills: skills.clone().count() as u32,
        on: skills.filter(|k| k.on).count() as u32,
    })
}

pub fn list_packs(s: &AppState) -> Result<Vec<PackView>, UiError> {
    Ok(s.library.packs(&s.store))
}

pub fn get_skill(s: &AppState, id: &str) -> Result<SkillDetail, UiError> {
    let (pack, entry) = s
        .library
        .skill(&s.store, id)
        .ok_or_else(|| UiError::other(NOT_AVAILABLE))?;
    let skill = pack
        .skills
        .iter()
        .find(|v| v.id == entry.id)
        .cloned()
        .ok_or_else(|| UiError::other(NOT_AVAILABLE))?;
    Ok(SkillDetail {
        skill,
        pack_id: pack.id.clone(),
        pack_title: pack.title.clone(),
        license: entry
            .license
            .clone()
            .unwrap_or_else(|| pack.license.clone()),
        source_url: pack.source_url(&entry.dir),
        body: s.library.skill_body(id)?,
        files: entry.files,
    })
}

pub fn set_skill_on(s: &AppState, id: &str, on: bool) -> Result<(), UiError> {
    if s.library.skill(&s.store, id).is_none() {
        return Err(UiError::other(NOT_AVAILABLE));
    }
    s.store.set_switch(id, on).map_err(local)
}

pub fn set_pack_on(s: &AppState, id: &str, on: bool) -> Result<(), UiError> {
    if !s.library.packs(&s.store).iter().any(|p| p.id == id) {
        return Err(UiError::other(
            "This pack is not available. Check it in Skills.",
        ));
    }
    s.store.set_switch(&format!("pack/{id}"), on).map_err(local)
}

/// A file the skill lists, as a text attachment ready for `send_message`.
pub fn skill_file(s: &AppState, id: &str, file: &str) -> Result<AttachmentInput, UiError> {
    let bytes = s.library.skill_file(id, file)?;
    Ok(AttachmentInput {
        name: file.rsplit(['/', '\\']).next().unwrap_or(file).to_string(),
        mime: "text/plain".to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}
