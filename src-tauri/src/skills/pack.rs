//! Select skill folders from an unpacked repository by rule and copy their
//! text files into a vendored pack with a manifest. Nothing here executes
//! anything from the source tree; files are only listed, read and copied.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ParsedSkill, SkillEntry, acts_on_computer, parse_skill};

/// Largest file copied besides `SKILL.md` itself.
const MAX_FILE_BYTES: u64 = 256_000;
/// Longest allowed path, `<pack id>/<dir>/<file>`, in characters.
const MAX_PATH_CHARS: usize = 180;
const ALLOWED_EXTENSIONS: [&str; 6] = ["md", "txt", "json", "yaml", "yml", "csv"];
const SKILL_FILE: &str = "SKILL.md";
const MANIFEST_FILE: &str = "pack.json";
const LICENSE_PREFIXES: [&str; 3] = ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"];

/// One pack's vendoring rule, as listed in the lock file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackSpec {
    pub id: String,
    pub title: String,
    pub repo: String,
    pub commit: String,
    pub select: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub license: String,
    #[serde(default)]
    pub bundled: bool,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub checksum: String,
    /// File extensions (without the dot, any case) not copied from this
    /// pack's skill folders, to keep the installer small. `SKILL.md` is
    /// always copied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leave_out: Vec<String>,
}

/// What a vendored pack contains; written to `pack.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    pub id: String,
    pub title: String,
    pub repo: String,
    pub commit: String,
    pub license: String,
    pub license_files: Vec<String>,
    pub skills: Vec<SkillEntry>,
    pub skipped: Vec<String>,
    pub checksum: String,
}

/// True when `dir` holds a regular file named exactly `SKILL.md`
/// (compared on the listed name, so case-insensitive disks do not match).
pub fn has_skill_md(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    rd.flatten()
        .any(|e| e.file_name() == SKILL_FILE && e.file_type().is_ok_and(|t| t.is_file()))
}

/// Real (non-link) child folders of `dir`.
fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

/// True when a select or exclude pattern stays inside the source root: no
/// empty, `..` or drive-prefixed segment, no leading `/`, no backslash.
fn safe_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && !pattern.contains('\\')
        && !pattern.contains(':')
        && pattern.split('/').all(|seg| !seg.is_empty() && seg != "..")
}

fn forward(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Folders (relative to `root`, `"."` for the root itself) that match a
/// `select` pattern and hold a `SKILL.md`, minus any whose relative path
/// is an `exclude` path or lies under one (whole segments). Sorted and
/// de-duplicated. Patterns and excludes with an empty or `..` segment, a
/// leading `/`, a drive prefix or a backslash are ignored, as are folders
/// whose names are not valid UTF-8.
pub fn select_skill_dirs(root: &Path, select: &[String], exclude: &[String]) -> Vec<PathBuf> {
    let mut found: Vec<String> = Vec::new();
    for pattern in select.iter().filter(|p| safe_pattern(p)) {
        let mut current: Vec<PathBuf> = vec![PathBuf::new()];
        for seg in pattern.split('/').filter(|p| !p.is_empty()) {
            let mut next = Vec::new();
            for rel in &current {
                match seg {
                    "." => next.push(rel.clone()),
                    "*" => {
                        for child in child_dirs(&root.join(rel)) {
                            if let Some(name) = child.file_name().and_then(|n| n.to_str()) {
                                next.push(rel.join(name));
                            }
                        }
                    }
                    lit => {
                        let cand = rel.join(lit);
                        let abs = root.join(&cand);
                        let real = fs::symlink_metadata(&abs).is_ok_and(|m| m.is_dir());
                        if real {
                            next.push(cand);
                        }
                    }
                }
            }
            current = next;
        }
        for rel in current {
            if has_skill_md(&root.join(&rel)) {
                let text = forward(&rel);
                found.push(if text.is_empty() {
                    ".".to_string()
                } else {
                    text
                });
            }
        }
    }
    found.retain(|p| {
        !exclude
            .iter()
            .filter(|x| safe_pattern(x))
            .any(|x| p == x || p.starts_with(&format!("{x}/")))
    });
    found.sort();
    found.dedup();
    found.into_iter().map(PathBuf::from).collect()
}

fn too_long(pack_id: &str, rel_in_pack: &str) -> bool {
    pack_id.chars().count() + 1 + rel_in_pack.chars().count() > MAX_PATH_CHARS
}

fn allowed_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ALLOWED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// True when `name`'s extension is listed in the pack's `leave_out`.
fn left_out(leave_out: &[String], name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| leave_out.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Walks `dir` collecting the files to copy (paths relative to `dir`, forward
/// slashes). Links, names that are not valid UTF-8, left-out types,
/// oversized files and over-long paths go to `skipped`
/// (paths relative to the pack root, via `prefix`).
fn collect_files(
    pack_id: &str,
    leave_out: &[String],
    prefix: &str,
    dir: &Path,
    rel: &str,
    files: &mut Vec<String>,
    skipped: &mut Vec<String>,
) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let raw = e.file_name();
        let name = raw.to_string_lossy().into_owned();
        let rel_here = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let in_pack = if prefix == "." {
            rel_here.clone()
        } else {
            format!("{prefix}/{rel_here}")
        };
        let Ok(ft) = e.file_type() else { continue };
        if raw.to_str().is_none() || ft.is_symlink() {
            skipped.push(in_pack);
        } else if ft.is_dir() {
            collect_files(
                pack_id,
                leave_out,
                prefix,
                &e.path(),
                &rel_here,
                files,
                skipped,
            );
        } else if ft.is_file() {
            if rel.is_empty() && name == SKILL_FILE {
                continue;
            }
            if !allowed_extension(&name) {
                continue;
            }
            if left_out(leave_out, &name) {
                skipped.push(in_pack);
                continue;
            }
            if too_long(pack_id, &in_pack)
                || e.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES)
            {
                skipped.push(in_pack);
            } else {
                files.push(rel_here);
            }
        }
    }
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("Could not copy {}: {e}", from.display()))
}

/// Licence files at the source root plus a `licenses/` folder, copied to the
/// pack root. Returns their pack-relative names. Links, oversized and
/// non-text licence files go to `skipped`.
fn copy_licenses(
    src_root: &Path,
    pack_id: &str,
    dest: &Path,
    skipped: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    if let Ok(rd) = fs::read_dir(src_root) {
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let upper = name.to_ascii_uppercase();
            if !LICENSE_PREFIXES.iter().any(|p| upper.starts_with(p)) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() || e.file_name().to_str().is_none() {
                skipped.push(name);
            } else if ft.is_file() {
                candidates.push((e.path(), name));
            }
        }
    }
    let lic = src_root.join("licenses");
    if fs::symlink_metadata(&lic).is_ok_and(|m| m.is_dir()) {
        walk_all(&lic, "licenses", &mut candidates, skipped);
    }
    let mut out = Vec::new();
    for (from, rel) in candidates {
        // Size first, so an oversized file is never read into memory.
        let usable = !too_long(pack_id, &rel)
            && fs::symlink_metadata(&from).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE_BYTES)
            && fs::read(&from).is_ok_and(|b| std::str::from_utf8(&b).is_ok());
        if !usable {
            skipped.push(rel);
        } else {
            copy_file(&from, &dest.join(&rel))?;
            out.push(rel);
        }
    }
    Ok(out)
}

/// Every regular file under `dir`; links and names that are not valid UTF-8
/// reported as skipped.
fn walk_all(dir: &Path, rel: &str, files: &mut Vec<(PathBuf, String)>, skipped: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let raw = e.file_name();
        let rel_here = format!("{rel}/{}", raw.to_string_lossy());
        let Ok(ft) = e.file_type() else { continue };
        if raw.to_str().is_none() || ft.is_symlink() {
            skipped.push(rel_here);
        } else if ft.is_dir() {
            walk_all(&e.path(), &rel_here, files, skipped);
        } else if ft.is_file() {
            files.push((e.path(), rel_here));
        }
    }
}

/// One skill folder as copied: the files beside its `SKILL.md` (relative
/// to the folder, `/` separators) and its parsed `SKILL.md`, or why it is
/// broken.
#[derive(Debug)]
pub struct CopiedSkill {
    pub files: Vec<String>,
    pub parsed: Option<ParsedSkill>,
    pub broken: Option<String>,
}

/// Copies one skill folder, `src_dir`, into `dest_dir`: its `SKILL.md` and,
/// unless it is the pack root (`in_pack == "."`), every other file of an
/// allowed text type, at most 256,000 bytes, not a link and not left out by
/// `leave_out`. `in_pack` is the folder's path inside pack `pack_id`; it
/// bounds path lengths and prefixes what is pushed to `skipped`.
/// `folder_name` names the skill when its `SKILL.md` gives no name. Nothing
/// outside `src_dir` is read.
pub fn copy_skill_dir(
    pack_id: &str,
    leave_out: &[String],
    in_pack: &str,
    src_dir: &Path,
    dest_dir: &Path,
    folder_name: &str,
    skipped: &mut Vec<String>,
) -> Result<CopiedSkill, String> {
    let is_root = in_pack == ".";
    let skill_in_pack = if is_root {
        SKILL_FILE.to_string()
    } else {
        format!("{in_pack}/{SKILL_FILE}")
    };
    let mut out = CopiedSkill {
        files: Vec::new(),
        parsed: None,
        broken: None,
    };
    if !is_root {
        collect_files(
            pack_id,
            leave_out,
            in_pack,
            src_dir,
            "",
            &mut out.files,
            skipped,
        );
    }
    if too_long(pack_id, &skill_in_pack) {
        skipped.push(skill_in_pack);
        out.broken = Some("The skill's path is too long.".to_string());
        return Ok(out);
    }
    let skill_src = src_dir.join(SKILL_FILE);
    copy_file(&skill_src, &dest_dir.join(SKILL_FILE))?;
    match fs::read(&skill_src) {
        Err(e) => out.broken = Some(format!("Could not read the skill file: {e}")),
        Ok(bytes) => match String::from_utf8(bytes) {
            Err(_) => out.broken = Some("The skill file is not valid text.".to_string()),
            Ok(text) => match parse_skill(&text, folder_name) {
                Ok(p) => out.parsed = Some(p),
                Err(reason) => out.broken = Some(reason),
            },
        },
    }
    for f in &out.files {
        copy_file(&src_dir.join(f), &dest_dir.join(f))?;
    }
    Ok(out)
}

/// Copies the selected skills of `src_root` into `dest` and writes
/// `dest/pack.json`. `overrides` maps skill id to the acts-on-computer mark
/// and wins over the text scan. `dest` must be missing or an empty folder.
pub fn copy_pack(
    src_root: &Path,
    spec: &PackSpec,
    dest: &Path,
    overrides: &HashMap<String, bool>,
) -> Result<PackManifest, String> {
    if let Ok(mut rd) = fs::read_dir(dest)
        && rd.next().is_some()
    {
        return Err(format!("{} is not empty", dest.display()));
    }
    fs::create_dir_all(dest).map_err(|e| format!("Could not create {}: {e}", dest.display()))?;
    let mut skills: Vec<SkillEntry> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut used_names: HashSet<String> = HashSet::new();

    for rel in select_skill_dirs(src_root, &spec.select, &spec.exclude) {
        let dir = forward(&rel);
        let is_root = dir == ".";
        let src_dir = if is_root {
            src_root.to_path_buf()
        } else {
            src_root.join(&rel)
        };
        let folder_name = if is_root {
            spec.id.clone()
        } else {
            rel.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let dest_dir = if is_root {
            dest.to_path_buf()
        } else {
            dest.join(&rel)
        };
        let CopiedSkill {
            files,
            parsed,
            broken,
        } = copy_skill_dir(
            &spec.id,
            &spec.leave_out,
            &dir,
            &src_dir,
            &dest_dir,
            &folder_name,
            &mut skipped,
        )?;

        let mut name = parsed
            .as_ref()
            .map_or(folder_name.clone(), |p| p.name.clone());
        if used_names.contains(&name) {
            name = folder_name.clone();
        }
        used_names.insert(name.clone());
        let id = if spec.id == "yours" {
            name.clone()
        } else {
            format!("{}:{name}", spec.id)
        };
        let (description, body_chars, license, scan) = match &parsed {
            Some(p) => (
                p.description.clone(),
                p.body.chars().count() as u32,
                p.license.clone(),
                acts_on_computer(&p.body),
            ),
            None => (String::new(), 0, None, false),
        };
        let acts = overrides.get(&id).copied().unwrap_or(scan);
        skills.push(SkillEntry {
            id,
            pack: spec.id.clone(),
            name,
            description,
            dir,
            acts_on_computer: acts,
            body_chars,
            files,
            license,
            broken,
        });
    }

    let license_files = copy_licenses(src_root, &spec.id, dest, &mut skipped)?;
    skipped.sort();
    skipped.dedup();
    let manifest = PackManifest {
        id: spec.id.clone(),
        title: spec.title.clone(),
        repo: spec.repo.clone(),
        commit: spec.commit.clone(),
        license: spec.license.clone(),
        license_files,
        skills,
        skipped,
        checksum: tree_checksum(dest),
    };
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    fs::write(dest.join(MANIFEST_FILE), json)
        .map_err(|e| format!("Could not write {MANIFEST_FILE}: {e}"))?;
    Ok(manifest)
}

/// SHA-256 (hex) over every file under `dir` except the top-level
/// `pack.json`: relative paths with forward slashes, sorted bytewise, each
/// followed by a length and the file's exact bytes. Links are ignored.
pub fn tree_checksum(dir: &Path) -> String {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut skipped = Vec::new();
    let mut found = Vec::new();
    walk_all(dir, "", &mut found, &mut skipped);
    for (path, rel) in found {
        let rel = rel.trim_start_matches('/').to_string();
        if rel != MANIFEST_FILE {
            files.push((rel, path));
        }
    }
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hasher = Sha256::new();
    for (rel, path) in files {
        let bytes = fs::read(&path).unwrap_or_default();
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The lock file's shape: `{ "version": 1, "packs": [PackSpec, ...] }`.
#[derive(Deserialize)]
struct LockFile {
    version: u32,
    packs: Vec<PackSpec>,
}

/// Parses `skills.lock.json`, keeping the packs in file order. Fails on an
/// unknown version or a pack id listed twice.
pub fn load_lock(text: &str) -> Result<Vec<PackSpec>, String> {
    let lock: LockFile =
        serde_json::from_str(text).map_err(|e| format!("The lock file is not valid: {e}"))?;
    if lock.version != 1 {
        return Err(format!("Unknown lock file version {}.", lock.version));
    }
    let mut seen = HashSet::new();
    for p in &lock.packs {
        if !seen.insert(p.id.as_str()) {
            return Err(format!("Pack {} is listed twice.", p.id));
        }
    }
    Ok(lock.packs)
}

/// Reads `pack.json` from a vendored pack folder.
pub fn read_manifest(pack_dir: &Path) -> Result<PackManifest, String> {
    let text = fs::read_to_string(pack_dir.join(MANIFEST_FILE))
        .map_err(|e| format!("Could not read {MANIFEST_FILE}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{MANIFEST_FILE} is not valid: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn put(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn skill(root: &Path, dir: &str, name: &str) {
        put(
            root,
            &format!("{dir}/SKILL.md"),
            format!("---\nname: {name}\ndescription: Does {name}.\n---\nBody of {name}.\n")
                .as_bytes(),
        );
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn spec(id: &str, select: &[&str]) -> PackSpec {
        PackSpec {
            id: id.into(),
            title: "T".into(),
            repo: "o/r".into(),
            commit: "abc".into(),
            select: s(select),
            exclude: vec![],
            license: "MIT".into(),
            bundled: false,
            about: String::new(),
            checksum: String::new(),
            leave_out: vec![],
        }
    }

    fn rels(v: Vec<PathBuf>) -> Vec<String> {
        v.iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    fn run(src: &Path, spec: &PackSpec) -> (TempDir, PackManifest) {
        let out = TempDir::new().unwrap();
        let dest = out.path().join(&spec.id);
        let m = copy_pack(src, spec, &dest, &HashMap::new()).unwrap();
        (out, m)
    }

    #[test]
    fn select_one_level_only_folders_with_skill_md() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        skill(t.path(), "skills/b", "b");
        put(t.path(), "skills/notes/README.md", b"x");
        put(t.path(), "other/c/SKILL.md", b"x");
        let got = rels(select_skill_dirs(t.path(), &s(&["skills/*"]), &[]));
        assert_eq!(got, vec!["skills/a", "skills/b"]);
    }

    #[test]
    fn select_two_levels_and_capitalised() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "plugins/p1/skills/x", "x");
        skill(t.path(), "plugins/p2/skills/y", "y");
        skill(t.path(), "Skills/g/h", "h");
        let got = rels(select_skill_dirs(
            t.path(),
            &s(&["plugins/*/skills/*"]),
            &[],
        ));
        assert_eq!(got, vec!["plugins/p1/skills/x", "plugins/p2/skills/y"]);
        let got = rels(select_skill_dirs(t.path(), &s(&["Skills/*/*"]), &[]));
        assert_eq!(got, vec!["Skills/g/h"]);
    }

    #[test]
    fn select_root_and_depth_one_with_exclude() {
        let t = TempDir::new().unwrap();
        skill(t.path(), ".", "root");
        skill(t.path(), "one", "one");
        skill(t.path(), "test", "t");
        skill(t.path(), "openclaw", "o");
        let got = rels(select_skill_dirs(t.path(), &s(&[".", "*"]), &[]));
        assert_eq!(got, vec![".", "one", "openclaw", "test"]);
        let got = rels(select_skill_dirs(
            t.path(),
            &s(&[".", "*"]),
            &s(&["test", "openclaw"]),
        ));
        assert_eq!(got, vec![".", "one"]);
    }

    #[test]
    fn exclude_matches_whole_path_segments_only() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "test", "t");
        skill(t.path(), "test-audit", "ta");
        skill(t.path(), "skills/in-progress/x", "x");
        skill(t.path(), "skills/in-progress-done/y", "y");
        let got = rels(select_skill_dirs(t.path(), &s(&["*"]), &s(&["test"])));
        assert_eq!(got, vec!["test-audit"]);
        let got = rels(select_skill_dirs(
            t.path(),
            &s(&["skills/*/*"]),
            &s(&["skills/in-progress"]),
        ));
        assert_eq!(got, vec!["skills/in-progress-done/y"]);
    }

    #[test]
    fn skill_md_matched_case_sensitively() {
        let t = TempDir::new().unwrap();
        put(t.path(), "skills/a/skill.md", b"hello");
        assert!(select_skill_dirs(t.path(), &s(&["skills/*"]), &[]).is_empty());
    }

    #[test]
    fn copy_keeps_text_types_and_skips_the_rest() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        for f in [
            "n.md",
            "n.txt",
            "n.json",
            "n.yaml",
            "n.yml",
            "n.csv",
            "sub/deep.md",
        ] {
            put(t.path(), &format!("skills/a/{f}"), b"x");
        }
        for f in ["run.sh", "tool.py", "pic.png", "font.ttf"] {
            put(t.path(), &format!("skills/a/{f}"), b"x");
        }
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        let e = &m.skills[0];
        assert_eq!(
            e.files,
            vec![
                "n.csv",
                "n.json",
                "n.md",
                "n.txt",
                "n.yaml",
                "n.yml",
                "sub/deep.md"
            ]
        );
        assert!(out.path().join("p/skills/a/n.csv").is_file());
        assert!(!out.path().join("p/skills/a/run.sh").exists());
        assert!(!out.path().join("p/skills/a/pic.png").exists());
    }

    #[test]
    fn left_out_extensions_are_skipped_and_listed() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "skills/a/data/big.csv", b"x");
        put(t.path(), "skills/a/data/idx.JSON", b"x");
        put(t.path(), "skills/a/notes.md", b"x");
        let mut sp = spec("p", &["skills/*"]);
        sp.leave_out = s(&["csv", "json"]);
        let (out, m) = run(t.path(), &sp);
        assert_eq!(m.skills[0].files, vec!["notes.md"]);
        assert_eq!(
            m.skipped,
            vec!["skills/a/data/big.csv", "skills/a/data/idx.JSON"]
        );
        assert!(!out.path().join("p/skills/a/data").exists());
        // SKILL.md is always copied.
        sp.leave_out = s(&["md"]);
        let (out, m) = run(t.path(), &sp);
        assert!(out.path().join("p/skills/a/SKILL.md").is_file());
        assert_eq!(m.skipped, vec!["skills/a/notes.md"]);
    }

    #[test]
    fn leave_out_is_optional_and_omitted_when_empty() {
        let j =
            r#"{"id":"p","title":"T","repo":"o/r","commit":"c","select":["a"],"license":"MIT"}"#;
        let sp: PackSpec = serde_json::from_str(j).unwrap();
        assert!(sp.leave_out.is_empty());
        assert!(!serde_json::to_string(&sp).unwrap().contains("leave_out"));
    }

    #[test]
    fn oversized_file_is_skipped_and_listed() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "skills/a/big.json", &vec![b' '; 300_000]);
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(m.skipped, vec!["skills/a/big.json"]);
        assert!(!out.path().join("p/skills/a/big.json").exists());
        assert!(m.skills[0].files.is_empty());
    }

    #[test]
    fn oversized_skill_md_is_still_copied_in_full() {
        let t = TempDir::new().unwrap();
        let mut text = String::from("---\nname: big\n---\n");
        text.push_str(&"x".repeat(300_000));
        put(t.path(), "skills/big/SKILL.md", text.as_bytes());
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert!(m.skipped.is_empty());
        assert_eq!(m.skills[0].body_chars, 300_000);
        assert_eq!(
            fs::read(out.path().join("p/skills/big/SKILL.md")).unwrap(),
            text.as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_skipped_and_listed() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "real.md", b"x");
        std::os::unix::fs::symlink(t.path().join("real.md"), t.path().join("skills/a/link.md"))
            .unwrap();
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(m.skipped, vec!["skills/a/link.md"]);
        assert!(!out.path().join("p/skills/a/link.md").exists());
    }

    #[test]
    fn long_path_is_skipped_and_listed() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        let long = format!("{}.md", "n".repeat(170));
        put(t.path(), &format!("skills/a/{long}"), b"x");
        let ok = format!("{}.md", "m".repeat(100));
        put(t.path(), &format!("skills/a/{ok}"), b"x");
        let (_out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(m.skipped, vec![format!("skills/a/{long}")]);
        assert_eq!(m.skills[0].files, vec![ok]);
    }

    #[test]
    fn copied_bytes_are_identical() {
        let t = TempDir::new().unwrap();
        let crlf = b"---\r\nname: a\r\n---\r\nline one\r\nline two\r\n";
        put(t.path(), "skills/a/SKILL.md", crlf);
        put(t.path(), "skills/a/notes.md", b"a\r\nb\r\n");
        let (out, _m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(
            fs::read(out.path().join("p/skills/a/SKILL.md")).unwrap(),
            crlf
        );
        assert_eq!(
            fs::read(out.path().join("p/skills/a/notes.md")).unwrap(),
            b"a\r\nb\r\n"
        );
    }

    #[test]
    fn license_files_go_to_the_pack_root() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "LICENSE", b"MIT text");
        put(t.path(), "NOTICE.md", b"notice");
        put(t.path(), "THIRD_PARTY_NOTICES.txt", b"third");
        put(t.path(), "README.md", b"readme");
        put(t.path(), "licenses/dep.txt", b"dep");
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        let mut lf = m.license_files.clone();
        lf.sort();
        assert_eq!(
            lf,
            vec![
                "LICENSE",
                "NOTICE.md",
                "THIRD_PARTY_NOTICES.txt",
                "licenses/dep.txt"
            ]
        );
        assert!(out.path().join("p/LICENSE").is_file());
        assert!(out.path().join("p/licenses/dep.txt").is_file());
        assert!(!out.path().join("p/README.md").exists());
    }

    #[test]
    fn root_pattern_copies_only_skill_md() {
        let t = TempDir::new().unwrap();
        skill(t.path(), ".", "root");
        put(t.path(), "docs/extra.md", b"x");
        put(t.path(), "top.md", b"x");
        let (out, m) = run(t.path(), &spec("p", &["."]));
        assert_eq!(m.skills.len(), 1);
        assert!(m.skills[0].files.is_empty());
        assert!(out.path().join("p/SKILL.md").is_file());
        assert!(!out.path().join("p/top.md").exists());
        assert!(!out.path().join("p/docs").exists());
    }

    #[test]
    fn unusable_skill_md_is_broken_and_pack_still_copies() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/good", "good");
        put(t.path(), "skills/bad/SKILL.md", b"---\nname: bad\n---\n\n");
        put(t.path(), "skills/bad/extra.md", b"x");
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(m.skills.len(), 2);
        let bad = m.skills.iter().find(|e| e.dir == "skills/bad").unwrap();
        assert!(bad.broken.is_some());
        assert_eq!(bad.name, "bad");
        assert_eq!(bad.files, vec!["extra.md"]);
        let good = m.skills.iter().find(|e| e.dir == "skills/good").unwrap();
        assert!(good.broken.is_none());
        assert!(out.path().join("p/skills/good/SKILL.md").is_file());
    }

    #[test]
    fn duplicate_names_second_uses_folder_name() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/one", "same");
        skill(t.path(), "skills/two", "same");
        let (_o, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(m.skills[0].name, "same");
        assert_eq!(m.skills[0].id, "p:same");
        assert_eq!(m.skills[1].name, "two");
        assert_eq!(m.skills[1].id, "p:two");
    }

    #[test]
    fn ids_are_pack_colon_name_and_yours_is_bare() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "alpha");
        let (_o, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(m.skills[0].id, "p:alpha");
        assert_eq!(m.skills[0].pack, "p");
        let (_o, m) = run(t.path(), &spec("yours", &["skills/*"]));
        assert_eq!(m.skills[0].id, "alpha");
    }

    #[test]
    fn overrides_win_over_the_scan() {
        let t = TempDir::new().unwrap();
        put(
            t.path(),
            "skills/a/SKILL.md",
            b"---\nname: a\n---\n```bash\nls\n```\n",
        );
        skill(t.path(), "skills/b", "b");
        let out = TempDir::new().unwrap();
        let dest = out.path().join("p");
        let plain = copy_pack(t.path(), &spec("p", &["skills/*"]), &dest, &HashMap::new()).unwrap();
        assert!(plain.skills[0].acts_on_computer);
        assert!(!plain.skills[1].acts_on_computer);
        let ov = HashMap::from([("p:a".to_string(), false), ("p:b".to_string(), true)]);
        let dest2 = out.path().join("q");
        let m = copy_pack(t.path(), &spec("p", &["skills/*"]), &dest2, &ov).unwrap();
        assert!(!m.skills[0].acts_on_computer);
        assert!(m.skills[1].acts_on_computer);
    }

    #[test]
    fn checksum_is_stable_and_sensitive() {
        let a = TempDir::new().unwrap();
        put(a.path(), "x/one.md", b"hello");
        put(a.path(), "two.txt", b"world");
        let c1 = tree_checksum(a.path());
        assert_eq!(c1, tree_checksum(a.path()));
        assert_eq!(c1.len(), 64);
        put(a.path(), "pack.json", b"{}");
        assert_eq!(c1, tree_checksum(a.path()));
        put(a.path(), "two.txt", b"worle");
        assert_ne!(c1, tree_checksum(a.path()));
    }

    #[test]
    fn manifest_checksum_matches_the_tree_and_roundtrips() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "LICENSE", b"MIT");
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        let dir = out.path().join("p");
        assert_eq!(m.checksum, tree_checksum(&dir));
        assert_eq!(read_manifest(&dir).unwrap(), m);
        assert!(read_manifest(&out.path().join("nope")).is_err());
    }

    #[test]
    fn pack_spec_defaults_when_fields_absent() {
        let j =
            r#"{"id":"p","title":"T","repo":"o/r","commit":"c","select":["a"],"license":"MIT"}"#;
        let sp: PackSpec = serde_json::from_str(j).unwrap();
        assert!(sp.exclude.is_empty() && sp.about.is_empty() && sp.checksum.is_empty());
        assert!(!sp.bundled);
    }

    #[test]
    fn select_ignores_unsafe_patterns_and_excludes() {
        let t = TempDir::new().unwrap();
        let root = t.path().join("repo");
        skill(&root, "skills/a", "a");
        skill(t.path(), "outside/b", "b");
        for bad in [
            "../outside/*",
            "skills/../../outside/*",
            "skills//*",
            "skills/*/",
            "",
            "/skills/*",
            "C:/skills/*",
            "C:skills/*",
            "skills\\*",
        ] {
            assert!(
                select_skill_dirs(&root, &s(&[bad]), &[]).is_empty(),
                "pattern {bad:?} should be ignored"
            );
        }
        let abs = format!("{}/outside/*", t.path().display());
        assert!(select_skill_dirs(&root, &s(&[&abs]), &[]).is_empty());
        // An unsafe exclude is ignored, so it removes nothing.
        let got = rels(select_skill_dirs(
            &root,
            &s(&["skills/*"]),
            &s(&["../skills", "skills//a", "/skills", "C:/skills"]),
        ));
        assert_eq!(got, vec!["skills/a"]);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_file_name_is_skipped_and_listed() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "skills/a/ok.md", b"x");
        let odd = t
            .path()
            .join("skills/a")
            .join(OsStr::from_bytes(b"bad\xff.md"));
        if fs::write(&odd, b"x").is_err() {
            // Some file systems (APFS) refuse such names; nothing to test.
            return;
        }
        let (_out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert_eq!(m.skills[0].files, vec!["ok.md"]);
        assert_eq!(m.skipped.len(), 1);
        assert!(m.skipped[0].starts_with("skills/a/bad"));
    }

    #[test]
    fn oversized_license_is_skipped_and_listed() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "LICENSE", &vec![b'x'; 300_000]);
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert!(m.license_files.is_empty());
        assert_eq!(m.skipped, vec!["LICENSE"]);
        assert!(!out.path().join("p/LICENSE").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_license_is_skipped_and_listed() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        put(t.path(), "real-license.txt", b"MIT");
        std::os::unix::fs::symlink(t.path().join("real-license.txt"), t.path().join("LICENSE"))
            .unwrap();
        let (out, m) = run(t.path(), &spec("p", &["skills/*"]));
        assert!(m.license_files.is_empty());
        assert_eq!(m.skipped, vec!["LICENSE"]);
        assert!(!out.path().join("p/LICENSE").exists());
    }

    #[test]
    fn copy_pack_refuses_a_non_empty_destination() {
        let t = TempDir::new().unwrap();
        skill(t.path(), "skills/a", "a");
        let out = TempDir::new().unwrap();
        let dest = out.path().join("p");
        put(&dest, "stale.md", b"old");
        let err = copy_pack(t.path(), &spec("p", &["skills/*"]), &dest, &HashMap::new());
        assert!(err.is_err());
        assert!(!dest.join("skills").exists());
        // An existing empty folder is fine.
        let empty = out.path().join("q");
        fs::create_dir_all(&empty).unwrap();
        assert!(copy_pack(t.path(), &spec("p", &["skills/*"]), &empty, &HashMap::new()).is_ok());
    }

    #[test]
    fn load_lock_reads_version_1_packs_in_order() {
        let j = r#"{"version":1,"packs":[
            {"id":"b","title":"B","repo":"o/b","commit":"c1","select":["skills/*"],"license":"MIT","bundled":true},
            {"id":"a","title":"A","repo":"o/a","commit":"c2","select":["*"],"license":"MIT"}]}"#;
        let packs = load_lock(j).unwrap();
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].id, "b");
        assert!(packs[0].bundled);
        assert_eq!(packs[1].id, "a");
        assert!(!packs[1].bundled);
    }

    #[test]
    fn load_lock_rejects_bad_input() {
        assert!(load_lock("not json").is_err());
        assert!(load_lock(r#"{"version":2,"packs":[]}"#).is_err());
        let dup = r#"{"version":1,"packs":[
            {"id":"a","title":"A","repo":"o/a","commit":"c","select":["*"],"license":"MIT"},
            {"id":"a","title":"A","repo":"o/a","commit":"c","select":["*"],"license":"MIT"}]}"#;
        assert!(load_lock(dup).is_err());
    }
}
