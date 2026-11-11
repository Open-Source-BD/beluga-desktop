//! Everything installed, as the running app sees it: the bundled packs, any
//! extra packs the user installed, and the user's own skills, with their
//! on/off switches. Reads only `pack.json` files and, on request, the text of
//! one skill; nothing inside a pack is ever executed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use super::pack::{PackSpec, load_lock, read_manifest};
use super::{PackOrigin, SkillEntry};
use crate::engine::UiError;
use crate::store::Store;

/// Display data for packs the lock file knows: title, about, license, repo, commit.
const LOCK_TEXT: &str = include_str!("../../../skills.lock.json");
const DEFAULTS_FILE: &str = "default-skills.json";
const YOURS_ID: &str = "yours";
const YOURS_TITLE: &str = "Added by you";
const PACK_SWITCH_PREFIX: &str = "pack/";
/// A file longer than this is not attached.
const MAX_FILE_BYTES: u64 = 200_000;
const SKILL_FILE: &str = "SKILL.md";
const BUILTIN_MISSING: &str =
    "The built-in skills could not be found. Reinstall the app to restore them.";
const OUTSIDE: &str = "This skill's files are outside its folder.";
const NOT_AVAILABLE: &str = "This skill is not available. Check it in Skills.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillView {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The skill's own setting, independent of its pack's switch.
    pub on: bool,
    pub acts_on_computer: bool,
    pub broken: Option<String>,
    pub body_chars: u32,
    /// On, its pack is on, and not broken: what the `/` menu offers.
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackView {
    pub id: String,
    pub title: String,
    pub origin: PackOrigin,
    pub license: String,
    pub repo: String,
    pub commit: String,
    pub about: String,
    pub on: bool,
    pub error: Option<String>,
    pub skills: Vec<SkillView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDetail {
    pub skill: SkillView,
    pub pack_id: String,
    pub pack_title: String,
    pub license: String,
    pub source_url: String,
    pub body: String,
    pub files: Vec<String>,
}

struct LoadedPack {
    id: String,
    origin: PackOrigin,
    root: PathBuf,
    title: String,
    about: String,
    license: String,
    repo: String,
    commit: String,
    error: Option<String>,
    skills: Vec<SkillEntry>,
}

pub struct Library {
    builtin_dir: PathBuf,
    data_skills_dir: PathBuf,
    lock: Vec<PackSpec>,
    loaded: RwLock<Vec<LoadedPack>>,
}

impl Library {
    /// Set when the bundled skills are missing or unreadable.
    pub fn builtin_error(&self) -> Option<String> {
        let ok = self.builtin_dir.is_dir()
            && std::fs::read_to_string(self.builtin_dir.join(DEFAULTS_FILE))
                .is_ok_and(|t| serde_json::from_str::<serde_json::Value>(&t).is_ok());
        (!ok).then(|| BUILTIN_MISSING.to_string())
    }

    pub fn load(builtin_dir: &Path, data_skills_dir: &Path) -> Library {
        let lib = Library {
            builtin_dir: builtin_dir.to_path_buf(),
            data_skills_dir: data_skills_dir.to_path_buf(),
            // The lock is committed with the app; a bad one only loses display data.
            lock: load_lock(LOCK_TEXT).unwrap_or_default(),
            loaded: RwLock::new(Vec::new()),
        };
        lib.reload();
        lib
    }

    /// Every pack the lock file lists, bundled or not, in lock order.
    pub fn lock_specs(&self) -> &[PackSpec] {
        &self.lock
    }

    /// True for the id of a bundled pack: one the lock marks as bundled, or
    /// one found in the bundled skills folder.
    pub fn is_builtin(&self, id: &str) -> bool {
        self.lock.iter().any(|p| p.bundled && p.id == id)
            || self
                .loaded
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|p| p.origin == PackOrigin::BuiltIn && p.id == id)
    }

    /// Reads every `pack.json` again, e.g. after an install.
    pub fn reload(&self) {
        let packs = self.read_all();
        *self.loaded.write().unwrap_or_else(|e| e.into_inner()) = packs;
    }

    fn read_all(&self) -> Vec<LoadedPack> {
        let mut out = Vec::new();

        let mut builtin = subfolders(&self.builtin_dir);
        let position = |id: &str| self.lock.iter().position(|p| p.id == id);
        builtin.sort_by(|a, b| {
            let (pa, pb) = (position(&a.0), position(&b.0));
            match (pa, pb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.0.cmp(&b.0),
            }
        });
        for (id, dir) in builtin {
            out.push(self.read_pack(&id, PackOrigin::BuiltIn, dir));
        }

        let mut installed = subfolders(&self.data_skills_dir.join("packs"));
        installed.sort();
        for (id, dir) in installed {
            out.push(self.read_pack(&id, PackOrigin::Installed, dir));
        }

        let yours = self.data_skills_dir.join(YOURS_ID);
        if yours.is_dir() {
            out.push(self.read_pack(YOURS_ID, PackOrigin::Yours, yours));
        }
        out
    }

    fn read_pack(&self, id: &str, origin: PackOrigin, root: PathBuf) -> LoadedPack {
        let spec = self.lock.iter().find(|p| p.id == id);
        let mut pack = LoadedPack {
            id: id.to_string(),
            origin,
            title: spec.map_or_else(|| id.to_string(), |p| p.title.clone()),
            about: spec.map(|p| p.about.clone()).unwrap_or_default(),
            license: spec.map(|p| p.license.clone()).unwrap_or_default(),
            repo: spec.map(|p| p.repo.clone()).unwrap_or_default(),
            commit: spec.map(|p| p.commit.clone()).unwrap_or_default(),
            error: None,
            skills: Vec::new(),
            root,
        };
        match read_manifest(&pack.root) {
            Ok(m) => {
                if spec.is_none() {
                    pack.title = m.title;
                    pack.license = m.license;
                    pack.repo = m.repo;
                    pack.commit = m.commit;
                }
                pack.skills = m.skills;
                for e in &mut pack.skills {
                    if !valid_rel_path(&e.dir, true)
                        || !e.files.iter().all(|f| valid_rel_path(f, false))
                    {
                        e.broken = Some(OUTSIDE.to_string());
                    }
                }
                pack.skills
                    .sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
            }
            Err(e) => pack.error = Some(e),
        }
        if origin == PackOrigin::Yours {
            pack.title = YOURS_TITLE.to_string();
        }
        pack
    }

    fn defaults(&self) -> HashSet<String> {
        #[derive(Deserialize)]
        struct Defaults {
            on: Vec<String>,
        }
        std::fs::read_to_string(self.builtin_dir.join(DEFAULTS_FILE))
            .ok()
            .and_then(|t| serde_json::from_str::<Defaults>(&t).ok())
            .map(|d| d.on.into_iter().collect())
            .unwrap_or_default()
    }

    pub fn packs(&self, store: &Store) -> Vec<PackView> {
        let defaults = self.defaults();
        let loaded = self.loaded.read().unwrap_or_else(|e| e.into_inner());
        loaded.iter().map(|p| view(p, store, &defaults)).collect()
    }

    /// The skill and a view of its pack, or `None` for an unknown id.
    pub fn skill(&self, store: &Store, id: &str) -> Option<(PackView, SkillEntry)> {
        let defaults = self.defaults();
        let loaded = self.loaded.read().unwrap_or_else(|e| e.into_inner());
        let (pack, entry) = find(&loaded, id)?;
        Some((view(pack, store, &defaults), entry.clone()))
    }

    /// The full text of the skill's SKILL.md.
    pub fn skill_body(&self, id: &str) -> Result<String, UiError> {
        let path = self.entry_path(id, SKILL_FILE, |_| true)?;
        std::fs::read_to_string(path)
            .map_err(|_| UiError::other("This skill's file could not be read."))
    }

    /// One of the files the entry lists, as bytes.
    pub fn skill_file(&self, id: &str, file: &str) -> Result<Vec<u8>, UiError> {
        if !valid_rel_path(file, false) {
            return Err(UiError::other("That file is not part of this skill."));
        }
        let path = self.entry_path(id, file, |e| e.files.iter().any(|f| f == file))?;
        let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
        let meta = std::fs::metadata(&path)
            .map_err(|_| UiError::other(&format!("{base} could not be read.")))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(UiError::other(&format!("{base} is too large to attach.")));
        }
        std::fs::read(&path).map_err(|_| UiError::other(&format!("{base} could not be read.")))
    }

    /// `<pack root>/<dir>/<file>` for a known skill, if `allowed` accepts its entry.
    fn entry_path(
        &self,
        id: &str,
        file: &str,
        allowed: impl Fn(&SkillEntry) -> bool,
    ) -> Result<PathBuf, UiError> {
        let loaded = self.loaded.read().unwrap_or_else(|e| e.into_inner());
        let (pack, entry) = find(&loaded, id).ok_or_else(|| UiError::other(NOT_AVAILABLE))?;
        if !allowed(entry) {
            return Err(UiError::other("That file is not part of this skill."));
        }
        if entry.broken.as_deref() == Some(OUTSIDE)
            || !valid_rel_path(&entry.dir, true)
            || !valid_rel_path(file, false)
        {
            return Err(UiError::other(NOT_AVAILABLE));
        }
        let mut path = pack.root.clone();
        if entry.dir != "." {
            path.extend(entry.dir.split(['/', '\\']));
        }
        path.extend(file.split(['/', '\\']));
        Ok(path)
    }

    /// The skill's own setting; see the module rules. Unknown and broken
    /// skills are never on.
    pub fn is_on(&self, store: &Store, id: &str) -> bool {
        let defaults = self.defaults();
        let loaded = self.loaded.read().unwrap_or_else(|e| e.into_inner());
        find(&loaded, id).is_some_and(|(p, e)| skill_on(p, e, store, &defaults))
    }
}

/// Manifest paths use `/`; accept `\\` too. Every segment must be non-empty,
/// not `..`, and free of `:` (drive prefixes). `.` alone is allowed only for
/// a skill's `dir`. Checked on the string, so it holds on every platform.
pub fn valid_rel_path(s: &str, allow_dot: bool) -> bool {
    if allow_dot && s == "." {
        return true;
    }
    !s.is_empty()
        && s.split(['/', '\\'])
            .all(|seg| !seg.is_empty() && seg != ".." && seg != "." && !seg.contains(':'))
}

fn find<'a>(loaded: &'a [LoadedPack], id: &str) -> Option<(&'a LoadedPack, &'a SkillEntry)> {
    loaded
        .iter()
        .find_map(|p| p.skills.iter().find(|e| e.id == id).map(|e| (p, e)))
}

/// Child folders as `(name, path)`; empty when `dir` does not exist.
fn subfolders(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect()
}

fn pack_on(pack: &LoadedPack, store: &Store) -> bool {
    store
        .switch(&format!("{PACK_SWITCH_PREFIX}{}", pack.id))
        .ok()
        .flatten()
        .unwrap_or(true)
}

fn skill_on(pack: &LoadedPack, e: &SkillEntry, store: &Store, defaults: &HashSet<String>) -> bool {
    if e.broken.is_some() {
        return false;
    }
    if let Ok(Some(on)) = store.switch(&e.id) {
        return on;
    }
    match pack.origin {
        PackOrigin::BuiltIn => defaults.contains(&e.id),
        PackOrigin::Yours => true,
        PackOrigin::Installed => false,
    }
}

fn skill_view(
    pack: &LoadedPack,
    e: &SkillEntry,
    store: &Store,
    d: &HashSet<String>,
    pack_is_on: bool,
) -> SkillView {
    let on = skill_on(pack, e, store, d);
    SkillView {
        id: e.id.clone(),
        name: e.name.clone(),
        description: e.description.clone(),
        on,
        acts_on_computer: e.acts_on_computer,
        broken: e.broken.clone(),
        body_chars: e.body_chars,
        available: on && pack_is_on && e.broken.is_none(),
    }
}

fn view(pack: &LoadedPack, store: &Store, defaults: &HashSet<String>) -> PackView {
    let on = pack_on(pack, store);
    PackView {
        id: pack.id.clone(),
        title: pack.title.clone(),
        origin: pack.origin,
        license: pack.license.clone(),
        repo: pack.repo.clone(),
        commit: pack.commit.clone(),
        about: pack.about.clone(),
        on,
        error: pack.error.clone(),
        skills: pack
            .skills
            .iter()
            .map(|e| skill_view(pack, e, store, defaults, on))
            .collect(),
    }
}

impl PackView {
    /// Where a skill's folder lives on GitHub; the repository at the commit
    /// when the skill is the repository root or the pack has no repo.
    pub fn source_url(&self, dir: &str) -> String {
        if self.repo.is_empty() {
            return String::new();
        }
        let base = format!("https://github.com/{}/tree/{}", self.repo, self.commit);
        match dir.trim_matches('/') {
            "" | "." => base,
            d => format!("{base}/{d}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn entry(pack: &str, name: &str, files: &[&str], broken: Option<&str>) -> SkillEntry {
        SkillEntry {
            id: if pack == "yours" {
                name.to_string()
            } else {
                format!("{pack}:{name}")
            },
            pack: pack.to_string(),
            name: name.to_string(),
            description: format!("Does {name}."),
            dir: format!("skills/{name}"),
            acts_on_computer: false,
            body_chars: 10,
            files: files.iter().map(|f| f.to_string()).collect(),
            license: None,
            broken: broken.map(str::to_string),
        }
    }

    /// Writes a pack folder: pack.json plus SKILL.md and extra files per skill.
    fn make_pack(root: &Path, id: &str, skills: Vec<SkillEntry>) {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        for s in &skills {
            let sd = dir.join(&s.dir);
            fs::create_dir_all(&sd).unwrap();
            fs::write(
                sd.join("SKILL.md"),
                format!("---\nname: {}\n---\r\nBody of {}.\n", s.name, s.name),
            )
            .unwrap();
            for f in &s.files {
                fs::write(sd.join(f), format!("file {f}")).unwrap();
            }
        }
        let manifest = serde_json::json!({
            "id": id, "title": format!("Title {id}"), "repo": format!("o/{id}"),
            "commit": "abc123", "license": "MIT", "license_files": [],
            "skills": skills, "skipped": [], "checksum": "x",
        });
        fs::write(dir.join("pack.json"), manifest.to_string()).unwrap();
    }

    struct Fx {
        _t: TempDir,
        builtin: PathBuf,
        data: PathBuf,
        store: Store,
    }

    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let builtin = t.path().join("builtin");
        let data = t.path().join("data");
        fs::create_dir_all(&builtin).unwrap();
        fs::write(
            builtin.join("default-skills.json"),
            r#"{"on":["alpha:one"]}"#,
        )
        .unwrap();
        make_pack(
            &builtin,
            "alpha",
            vec![
                entry("alpha", "two", &[], None),
                entry("alpha", "one", &["notes.md"], None),
                entry("alpha", "bad", &[], Some("Missing name")),
            ],
        );
        make_pack(
            &data.join("packs"),
            "extra",
            vec![entry("extra", "x", &[], None)],
        );
        make_pack(&data, "yours", vec![entry("yours", "mine", &[], None)]);
        Fx {
            _t: t,
            builtin,
            data,
            store: Store::in_memory().unwrap(),
        }
    }

    fn lib(f: &Fx) -> Library {
        Library::load(&f.builtin, &f.data)
    }

    #[test]
    fn loads_builtin_installed_and_yours_with_the_right_origins() {
        let f = fx();
        let packs = lib(&f).packs(&f.store);
        let got: Vec<_> = packs.iter().map(|p| (p.id.as_str(), p.origin)).collect();
        assert_eq!(
            got,
            vec![
                ("alpha", PackOrigin::BuiltIn),
                ("extra", PackOrigin::Installed),
                ("yours", PackOrigin::Yours)
            ]
        );
        assert_eq!(packs[0].title, "Title alpha");
        assert_eq!(packs[2].title, "Added by you");
        assert_eq!(packs[1].repo, "o/extra");
        // Skills are sorted by name within a pack.
        let names: Vec<_> = packs[0].skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["bad", "one", "two"]);
        assert!(packs.iter().all(|p| p.on && p.error.is_none()));
    }

    #[test]
    fn missing_installed_and_yours_folders_are_tolerated() {
        let f = fx();
        let l = Library::load(&f.builtin, &f.data.join("nowhere"));
        let ids: Vec<_> = l.packs(&f.store).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["alpha"]);
    }

    #[test]
    fn a_pack_folder_without_pack_json_is_listed_with_an_error() {
        let f = fx();
        fs::create_dir_all(f.data.join("packs/hollow")).unwrap();
        let packs = lib(&f).packs(&f.store);
        let hollow = packs.iter().find(|p| p.id == "hollow").unwrap();
        assert!(hollow.skills.is_empty());
        assert!(hollow.error.as_deref().unwrap().contains("pack.json"));
    }

    #[test]
    fn default_on_follows_default_skills_json() {
        let f = fx();
        let l = lib(&f);
        assert!(l.is_on(&f.store, "alpha:one"));
        assert!(!l.is_on(&f.store, "alpha:two"));
        assert!(!l.is_on(&f.store, "extra:x"), "installed packs default off");
        assert!(l.is_on(&f.store, "mine"), "your own default on");
        assert!(!l.is_on(&f.store, "alpha:nope"));
    }

    #[test]
    fn a_stored_switch_wins() {
        let f = fx();
        let l = lib(&f);
        f.store.set_switch("alpha:one", false).unwrap();
        f.store.set_switch("extra:x", true).unwrap();
        assert!(!l.is_on(&f.store, "alpha:one"));
        assert!(l.is_on(&f.store, "extra:x"));
    }

    #[test]
    fn pack_off_hides_its_skills_but_keeps_their_switches() {
        let f = fx();
        let l = lib(&f);
        f.store.set_switch("pack/alpha", false).unwrap();
        let packs = l.packs(&f.store);
        let alpha = &packs[0];
        assert!(!alpha.on);
        let one = alpha.skills.iter().find(|s| s.name == "one").unwrap();
        assert!(one.on, "the skill's own setting is untouched");
        assert!(!one.available);
        assert!(alpha.skills.iter().all(|s| !s.available));
        let mine = packs[2].skills.iter().find(|s| s.name == "mine").unwrap();
        assert!(mine.available);
    }

    #[test]
    fn broken_skills_are_listed_but_never_on() {
        let f = fx();
        let l = lib(&f);
        f.store.set_switch("alpha:bad", true).unwrap();
        let packs = l.packs(&f.store);
        let bad = packs[0].skills.iter().find(|s| s.name == "bad").unwrap();
        assert_eq!(bad.broken.as_deref(), Some("Missing name"));
        assert!(!bad.on && !bad.available);
        assert!(!l.is_on(&f.store, "alpha:bad"));
    }

    #[test]
    fn skill_body_returns_the_exact_file_text() {
        let f = fx();
        let text = lib(&f).skill_body("alpha:one").unwrap();
        assert_eq!(text, "---\nname: one\n---\r\nBody of one.\n");
    }

    #[test]
    fn skill_body_of_an_unknown_id_is_a_ui_error() {
        let f = fx();
        let e = lib(&f).skill_body("alpha:ghost").unwrap_err();
        assert_eq!(
            e.message,
            "This skill is not available. Check it in Skills."
        );
    }

    #[test]
    fn skill_file_refuses_a_name_not_in_the_entry_and_any_path_with_dot_dot() {
        let f = fx();
        let l = lib(&f);
        assert_eq!(
            l.skill_file("alpha:one", "notes.md").unwrap(),
            b"file notes.md"
        );
        for bad in [
            "SKILL.md",
            "other.md",
            "../two/SKILL.md",
            "/etc/passwd",
            "\\x",
            "a..b",
        ] {
            assert!(l.skill_file("alpha:one", bad).is_err(), "{bad}");
        }
        assert!(l.skill_file("alpha:ghost", "notes.md").is_err());
    }

    #[test]
    fn skill_file_refuses_a_file_over_200000_bytes() {
        let f = fx();
        let p = f.builtin.join("alpha/skills/one/notes.md");
        fs::write(&p, vec![b'a'; 200_001]).unwrap();
        let e = lib(&f).skill_file("alpha:one", "notes.md").unwrap_err();
        assert_eq!(e.message, "notes.md is too large to attach.");
    }

    #[test]
    fn reload_picks_up_a_pack_added_later() {
        let f = fx();
        let l = lib(&f);
        make_pack(
            &f.data.join("packs"),
            "late",
            vec![entry("late", "z", &[], None)],
        );
        assert!(l.packs(&f.store).iter().all(|p| p.id != "late"));
        l.reload();
        assert!(l.packs(&f.store).iter().any(|p| p.id == "late"));
    }

    #[test]
    fn relative_paths_are_validated_as_strings_on_every_platform() {
        for bad in [
            "C:\\x",
            "C:x",
            "/etc/passwd",
            "..\\..\\x",
            "a/../../b",
            "",
            "a//b",
            "\\x",
            "a/",
        ] {
            assert!(!valid_rel_path(bad, false), "{bad}");
            assert!(!valid_rel_path(bad, true), "{bad}");
        }
        for ok in ["v1..2.md", "references/notes.md", "skills/one", "a\\b.md"] {
            assert!(valid_rel_path(ok, false), "{ok}");
        }
        assert!(valid_rel_path(".", true));
        assert!(!valid_rel_path(".", false));
    }

    #[test]
    fn a_manifest_entry_with_a_bad_dir_or_file_is_listed_as_broken() {
        let f = fx();
        for (dir, files) in [
            ("C:\\x", vec![]),
            ("../out", vec![]),
            ("skills/ok", vec!["C:\\Windows\\win.ini"]),
        ] {
            let mut e = entry("evil", "e", &[], None);
            e.dir = dir.to_string();
            e.files = files.iter().map(|x| x.to_string()).collect();
            let root = f.data.join("packs");
            let d = root.join("evil");
            fs::create_dir_all(&d).unwrap();
            let manifest = serde_json::json!({
                "id": "evil", "title": "t", "repo": "o/evil", "commit": "c", "license": "MIT",
                "license_files": [], "skills": [e], "skipped": [], "checksum": "x",
            });
            fs::write(d.join("pack.json"), manifest.to_string()).unwrap();
            let l = lib(&f);
            let packs = l.packs(&f.store);
            let evil = packs.iter().find(|p| p.id == "evil").unwrap();
            assert_eq!(
                evil.skills[0].broken.as_deref(),
                Some("This skill's files are outside its folder.")
            );
            assert!(l.skill_body("evil:e").is_err());
            assert!(l.skill_file("evil:e", "x.md").is_err());
        }
    }

    #[test]
    fn a_file_name_with_two_dots_inside_is_allowed() {
        let f = fx();
        let p = f.builtin.join("alpha/skills/one/v1..2.md");
        fs::write(&p, "x").unwrap();
        let mut m = read_manifest(&f.builtin.join("alpha")).unwrap();
        m.skills
            .iter_mut()
            .find(|s| s.name == "one")
            .unwrap()
            .files
            .push("v1..2.md".into());
        fs::write(
            f.builtin.join("alpha/pack.json"),
            serde_json::to_string(&m).unwrap(),
        )
        .unwrap();
        assert_eq!(lib(&f).skill_file("alpha:one", "v1..2.md").unwrap(), b"x");
    }

    #[test]
    fn a_missing_builtin_dir_reports_the_exact_text_and_lists_no_builtin_packs() {
        let f = fx();
        let l = Library::load(&f.builtin.join("nowhere"), &f.data);
        assert_eq!(
            l.builtin_error().as_deref(),
            Some("The built-in skills could not be found. Reinstall the app to restore them.")
        );
        assert!(
            l.packs(&f.store)
                .iter()
                .all(|p| p.origin != PackOrigin::BuiltIn)
        );
    }

    #[test]
    fn a_builtin_dir_without_default_skills_json_reports_it_too() {
        let f = fx();
        fs::remove_file(f.builtin.join("default-skills.json")).unwrap();
        assert!(lib(&f).builtin_error().is_some());
    }

    #[test]
    fn a_healthy_builtin_dir_reports_no_error() {
        let f = fx();
        assert_eq!(lib(&f).builtin_error(), None);
    }

    fn real_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")
    }

    #[test]
    fn the_real_vendored_tree_loads_all_15_packs_and_at_most_40_skills_are_on() {
        let store = Store::in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let l = Library::load(&real_dir(), tmp.path());
        let packs = l.packs(&store);
        assert_eq!(packs.len(), 15);
        assert!(
            packs
                .iter()
                .all(|p| p.error.is_none() && p.origin == PackOrigin::BuiltIn)
        );
        assert_eq!(l.builtin_error(), None);
        assert_eq!(packs[0].id, "superpowers", "lock order");
        assert_eq!(packs.iter().map(|p| p.skills.len()).sum::<usize>(), 795);
        let on = packs
            .iter()
            .flat_map(|p| &p.skills)
            .filter(|s| s.on)
            .count();
        assert!((1..=40).contains(&on), "{on} on");
        let (pack, gstack) = l.skill(&store, "gstack:gstack").unwrap();
        assert_eq!(pack.id, "gstack");
        assert!(!l.skill_body("gstack:gstack").unwrap().is_empty());
        assert_eq!(gstack.dir, ".");
    }

    #[test]
    fn loading_the_real_tree_takes_under_500_ms() {
        let tmp = tempfile::tempdir().unwrap();
        let start = std::time::Instant::now();
        let l = Library::load(&real_dir(), tmp.path());
        let took = start.elapsed();
        println!("library load took {took:?}");
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
        drop(l);
    }
}
