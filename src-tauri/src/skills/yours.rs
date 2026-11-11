//! The user's own skills ("Added by you"): finding skills in a folder,
//! staging a filtered copy, moving it into `<app data>/skills/yours/`, and
//! keeping `yours/pack.json` equal to the skill folders present.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::pack::{
    PackManifest, PackSpec, copy_pack, copy_skill_dir, has_skill_md, select_skill_dirs,
    tree_checksum,
};
use super::staging::{swap_back, swap_into, tree_bytes};
use super::{SkillEntry, acts_on_computer, parse_skill};
use crate::engine::UiError;

pub const YOURS: &str = "yours";
/// Under a stage: the regrouped skills, one folder per skill name.
pub const STAGED_SKILLS: &str = "skills";

pub const NO_SKILLS: &str = "No skills were found there. A skill is a folder with a SKILL.md file.";
pub const TOO_MANY: &str = "That source has more than 200 skills. Add a smaller folder.";
pub const TOO_LARGE: &str = "Those skills are too large to add.";
const COULD_NOT_ADD: &str = "Could not add those skills on this computer.";
const COULD_NOT_REMOVE: &str = "Could not remove that skill on this computer.";
pub const NOT_YOURS: &str = "Only skills you added can be removed.";

const MAX_SKILLS: usize = 200;
const MAX_TEXT_BYTES: u64 = 2_000_000;
const MAX_NAME_CHARS: usize = 64;
const SKILL_FILE: &str = "SKILL.md";
const MANIFEST_FILE: &str = "pack.json";
/// Skill folders up to four levels below the source.
const DEPTHS: [&str; 4] = ["*", "*/*", "*/*/*", "*/*/*/*"];

fn other(message: &str) -> UiError {
    UiError::other(message)
}

/// What a staged add holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Staged {
    pub skills: Vec<SkillEntry>,
    pub skipped: Vec<String>,
}

/// Lowercase `[a-z0-9-]`, runs of `-` collapsed, trimmed, at most 64
/// characters; `skill` when nothing is left. The result never contains `/`
/// or `:`, so it cannot collide with a `pack:name` id or a `pack/<id>` switch.
pub fn sanitize_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.to_lowercase().chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if !(c == '-' && (out.is_empty() || out.ends_with('-'))) {
            out.push(c);
        }
    }
    out.truncate(MAX_NAME_CHARS);
    let out = out.trim_matches('-');
    if out.is_empty() {
        "skill".to_string()
    } else {
        out.to_string()
    }
}

/// True for a name `sanitize_name` leaves unchanged.
pub fn is_skill_name(name: &str) -> bool {
    !name.is_empty() && sanitize_name(name) == name
}

/// `name`, or `name-2`, `name-3`, ... if taken; stays within 64 characters.
fn unique(name: String, taken: &mut HashSet<String>) -> String {
    let mut candidate = name.clone();
    let mut n = 2;
    while taken.contains(&candidate) {
        let suffix = format!("-{n}");
        let mut base = name.clone();
        base.truncate(MAX_NAME_CHARS - suffix.len());
        candidate = format!("{}{suffix}", base.trim_end_matches('-'));
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

/// A folder name usable as a literal select pattern: not hidden, no `*`.
fn literal_ok(segment: &str) -> bool {
    !segment.starts_with('.') && !segment.contains('*')
}

/// Splits a manifest path (`/` separators) onto `root`.
fn under(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    p.extend(rel.split('/'));
    p
}

/// Finds the skills in `source` and copies their text files into
/// `work/skills/<name>/`, applying the add limits. The source itself is the
/// skill when it has a `SKILL.md`, named `fallback_name` unless its
/// `SKILL.md` names it; otherwise every non-hidden folder with one, up to
/// four levels down. Nothing outside `source` is read, and `skipped` lists
/// only paths inside it.
pub fn stage_add(source: &Path, fallback_name: &str, work: &Path) -> Result<Staged, UiError> {
    let copy = work.join("copy");
    // Each skill found: its name, its copied folder and the files beside SKILL.md.
    let mut found: Vec<(String, PathBuf, Vec<String>)> = Vec::new();
    let mut skipped = Vec::new();
    if has_skill_md(source) {
        let in_pack = match fallback_name {
            "" | "." | ".." => "skill",
            name => name,
        };
        let c = copy_skill_dir(YOURS, &[], in_pack, source, &copy, in_pack, &mut skipped)
            .map_err(|_| other(COULD_NOT_ADD))?;
        let name = c.parsed.map_or_else(|| in_pack.to_string(), |p| p.name);
        found.push((name, copy.clone(), c.files));
    } else {
        let depths: Vec<String> = DEPTHS.iter().map(|d| d.to_string()).collect();
        let select: Vec<String> = select_skill_dirs(source, &depths, &[])
            .into_iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|d| d.split('/').all(literal_ok))
            .collect();
        if select.is_empty() {
            return Err(other(NO_SKILLS));
        }
        if select.len() > MAX_SKILLS {
            return Err(other(TOO_MANY));
        }
        let spec = PackSpec {
            id: YOURS.to_string(),
            title: String::new(),
            repo: String::new(),
            commit: String::new(),
            select,
            exclude: Vec::new(),
            license: String::new(),
            bundled: false,
            about: String::new(),
            checksum: String::new(),
            leave_out: Vec::new(),
        };
        let manifest =
            copy_pack(source, &spec, &copy, &HashMap::new()).map_err(|_| other(COULD_NOT_ADD))?;
        skipped = manifest.skipped;
        found = manifest
            .skills
            .into_iter()
            .map(|e| (e.name, under(&copy, &e.dir), e.files))
            .collect();
    }

    let out = work.join(STAGED_SKILLS);
    fs::create_dir_all(&out).map_err(|_| other(COULD_NOT_ADD))?;
    let mut taken = HashSet::new();
    for (name, from, files) in &found {
        // A SKILL.md that had to be skipped (path too long) is in `skipped`.
        if !from.join(SKILL_FILE).is_file() {
            continue;
        }
        let to = out.join(unique(sanitize_name(name), &mut taken));
        for f in std::iter::once(SKILL_FILE).chain(files.iter().map(String::as_str)) {
            let target = under(&to, f);
            let copied = target
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|_| fs::copy(under(from, f), &target));
            copied.map_err(|_| other(COULD_NOT_ADD))?;
        }
    }
    if taken.is_empty() {
        return Err(other(NO_SKILLS));
    }
    if tree_bytes(&out) > MAX_TEXT_BYTES {
        return Err(other(TOO_LARGE));
    }
    let mut names: Vec<String> = taken.into_iter().collect();
    names.sort();
    Ok(Staged {
        skills: names.iter().map(|n| entry_for(&out, n)).collect(),
        skipped,
    })
}

/// Every regular file under `dir`, relative, with `/` separators.
fn list_files(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let here = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        match e.file_type() {
            Ok(t) if t.is_dir() => list_files(&e.path(), &here, out),
            Ok(t) if t.is_file() => out.push(here),
            _ => {}
        }
    }
}

/// The library entry for the skill folder `root/name`. In `yours` a skill's
/// name and id are its folder name.
fn entry_for(root: &Path, name: &str) -> SkillEntry {
    let dir = root.join(name);
    let mut files = Vec::new();
    list_files(&dir, "", &mut files);
    files.retain(|f| f != SKILL_FILE);
    files.sort();
    let parsed = match fs::read(dir.join(SKILL_FILE)) {
        Err(e) => Err(format!("Could not read the skill file: {e}")),
        Ok(bytes) => String::from_utf8(bytes)
            .map_err(|_| "The skill file is not valid text.".to_string())
            .and_then(|text| parse_skill(&text, name)),
    };
    let (description, body_chars, license, acts, broken) = match parsed {
        Ok(p) => (
            p.description,
            p.body.chars().count() as u32,
            p.license,
            acts_on_computer(&p.body),
            None,
        ),
        Err(reason) => (String::new(), 0, None, false, Some(reason)),
    };
    SkillEntry {
        id: name.to_string(),
        pack: YOURS.to_string(),
        name: name.to_string(),
        description,
        dir: name.to_string(),
        acts_on_computer: acts,
        body_chars,
        files,
        license,
        broken,
    }
}

/// The skill folders in `yours`: real folders with a valid name and a `SKILL.md`.
fn skill_folders(yours: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(yours) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_skill_name(n) && has_skill_md(&yours.join(n)))
        .collect();
    names.sort();
    names
}

/// Writes `yours/pack.json` from the skill folders present (via `work`, then
/// a rename). Returns how many skills it lists.
fn write_manifest(yours: &Path, work: &Path) -> std::io::Result<usize> {
    let skills: Vec<SkillEntry> = skill_folders(yours)
        .iter()
        .map(|n| entry_for(yours, n))
        .collect();
    let count = skills.len();
    let manifest = PackManifest {
        id: YOURS.to_string(),
        title: String::new(),
        repo: String::new(),
        commit: String::new(),
        license: String::new(),
        license_files: Vec::new(),
        skills,
        skipped: Vec::new(),
        checksum: tree_checksum(yours),
    };
    let json = serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?;
    let tmp = work.join(MANIFEST_FILE);
    fs::write(&tmp, json)?;
    fs::rename(&tmp, yours.join(MANIFEST_FILE))?;
    Ok(count)
}

/// Moves every folder of `staged` into `yours`, replacing same-named skills,
/// and rewrites `pack.json`. All or nothing.
pub fn commit_add(staged: &Path, yours: &Path, work: &Path) -> Result<(), UiError> {
    let fail = |_| other(COULD_NOT_ADD);
    let created = fs::symlink_metadata(yours).is_err();
    fs::create_dir_all(yours).map_err(fail)?;
    let names = skill_folders(staged);
    let old = work.join("old");
    let mut done: Vec<(&String, bool)> = Vec::new();
    let mut result = Ok(0);
    for name in &names {
        match swap_into(&staged.join(name), &yours.join(name), &old.join(name)) {
            Ok(had_old) => done.push((name, had_old)),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    if result.is_ok() {
        result = write_manifest(yours, work);
    }
    if let Err(e) = result {
        for (name, had_old) in done.into_iter().rev() {
            swap_back(
                &staged.join(name),
                &yours.join(name),
                &old.join(name),
                had_old,
            );
        }
        if created {
            let _ = fs::remove_dir(yours);
        }
        return Err(fail(e));
    }
    Ok(())
}

/// Removes one skill folder from `yours` and rewrites `pack.json`; removes
/// `yours` itself when no skill is left.
pub fn remove_skill(yours: &Path, name: &str, work: &Path) -> Result<(), UiError> {
    let dir = yours.join(name);
    if !is_skill_name(name) || !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err(other(NOT_YOURS));
    }
    let fail = |_| other(COULD_NOT_REMOVE);
    let old = work.join("old");
    fs::rename(&dir, &old).map_err(fail)?;
    match write_manifest(yours, work) {
        Ok(0) => fs::rename(yours, work.join("yours")).map_err(fail),
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = fs::rename(&old, &dir);
            Err(fail(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized_to_lowercase_letters_digits_and_dashes() {
        for (raw, want) in [
            ("brainstorming", "brainstorming"),
            ("My Skill", "my-skill"),
            ("superpowers:brainstorming", "superpowers-brainstorming"),
            ("pack/x", "pack-x"),
            ("  --A__b..C--  ", "a-b-c"),
            ("Ünïcode ✓ name", "n-code-name"),
            ("", "skill"),
            ("::://", "skill"),
        ] {
            assert_eq!(sanitize_name(raw), want, "{raw:?}");
        }
        let long = sanitize_name(&"a".repeat(100));
        assert_eq!(long.len(), 64);
        // Cutting at 64 never leaves a trailing dash.
        let cut = sanitize_name(&format!("{}-b", "a".repeat(63)));
        assert_eq!(cut, "a".repeat(63));
    }
}
