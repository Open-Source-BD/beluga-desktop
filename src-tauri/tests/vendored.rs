//! Checks on the committed, vendored skill packs. They read the tree under
//! `resources/skills/` and the lock file; no network is used.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use beluga_desktop_lib::skills::pack::{
    PackManifest, PackSpec, load_lock, read_manifest, tree_checksum,
};

const ALLOWED_EXTENSIONS: [&str; 6] = ["md", "txt", "json", "yaml", "yml", "csv"];
const MAX_FILE_BYTES: u64 = 256_000;
const MAX_TREE_BYTES: u64 = 25_000_000;
const MAX_DEFAULT_ON: usize = 40;

fn skills_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")
}

fn lock() -> Vec<PackSpec> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills.lock.json");
    let text = fs::read_to_string(&path).expect("skills.lock.json is readable");
    load_lock(&text).expect("skills.lock.json is valid")
}

fn bundled() -> Vec<PackSpec> {
    lock().into_iter().filter(|p| p.bundled).collect()
}

fn manifests() -> Vec<PackManifest> {
    bundled()
        .iter()
        .map(|p| read_manifest(&skills_root().join(&p.id)).expect("pack.json is readable"))
        .collect()
}

fn read_json<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let text = fs::read_to_string(skills_root().join(name)).expect("defaults file is readable");
    serde_json::from_str(&text).expect("defaults file is valid JSON")
}

#[derive(serde::Deserialize)]
struct Defaults {
    on: Vec<String>,
}

fn default_on() -> Vec<String> {
    read_json::<Defaults>("default-skills.json").on
}

/// Every regular file under `dir` with its path relative to the skills root.
fn walk(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let ft = e.file_type().unwrap();
        assert!(
            !ft.is_symlink(),
            "link in vendored tree: {}",
            e.path().display()
        );
        if ft.is_dir() {
            walk(&e.path(), out);
        } else {
            let rel = e
                .path()
                .strip_prefix(skills_root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, e.path()));
        }
    }
}

#[test]
fn lock_has_18_packs_15_bundled_with_the_expected_ids() {
    let ids: Vec<String> = lock().into_iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        vec![
            "superpowers",
            "agent-skills",
            "anthropic",
            "emil",
            "engineer",
            "gstack",
            "ecc",
            "mattpocock",
            "impeccable",
            "ui-ux-pro-max",
            "expo",
            "react-native",
            "apollo",
            "astral",
            "agents",
            "trailofbits",
            "vercel",
            "claude-red",
        ]
    );
    let not_bundled: Vec<String> = lock()
        .into_iter()
        .filter(|p| !p.bundled)
        .map(|p| p.id)
        .collect();
    assert_eq!(not_bundled, vec!["trailofbits", "vercel", "claude-red"]);
    for p in lock() {
        assert_eq!(p.commit.len(), 40, "{} needs a full commit SHA", p.id);
        assert!(p.commit.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

#[test]
fn every_bundled_pack_has_a_pack_json_whose_checksum_matches_the_lock() {
    for spec in bundled() {
        let dir = skills_root().join(&spec.id);
        let m = read_manifest(&dir).unwrap();
        assert_eq!(m.id, spec.id);
        assert_eq!(m.commit, spec.commit, "{}", spec.id);
        assert_eq!(m.checksum, spec.checksum, "{}: pack.json vs lock", spec.id);
        assert_eq!(
            tree_checksum(&dir),
            spec.checksum,
            "{}: tree vs lock",
            spec.id
        );
    }
}

#[test]
fn every_bundled_pack_ships_at_least_one_license_file() {
    for m in manifests() {
        let per_skill = m
            .skills
            .iter()
            .filter(|s| s.files.iter().any(|f| f == "LICENSE.txt"))
            .count();
        if m.id == "anthropic" {
            assert!(per_skill > 0, "anthropic: per-skill LICENSE.txt files");
        } else {
            assert!(!m.license_files.is_empty(), "{} has no license file", m.id);
        }
        for f in &m.license_files {
            assert!(skills_root().join(&m.id).join(f).is_file());
        }
    }
}

#[test]
fn skill_counts_match_the_spec() {
    let expected: HashMap<&str, usize> = HashMap::from([
        ("superpowers", 15),
        ("agent-skills", 25),
        ("anthropic", 19),
        ("emil", 18),
        ("engineer", 67),
        ("gstack", 63),
        ("ecc", 314),
        ("mattpocock", 31),
        ("impeccable", 1),
        ("ui-ux-pro-max", 7),
        ("expo", 24),
        ("react-native", 9),
        ("apollo", 15),
        ("astral", 3),
        ("agents", 185),
    ]);
    let got: HashMap<String, usize> = manifests()
        .into_iter()
        .map(|m| (m.id, m.skills.len()))
        .collect();
    assert_eq!(got.len(), expected.len());
    for (id, n) in expected {
        assert_eq!(got.get(id), Some(&n), "{id}");
    }
}

#[test]
fn vendored_tree_is_at_most_25_megabytes() {
    let mut files = Vec::new();
    walk(&skills_root(), &mut files);
    let total: u64 = files
        .iter()
        .map(|(_, p)| fs::metadata(p).unwrap().len())
        .sum();
    assert!(total <= MAX_TREE_BYTES, "vendored tree is {total} bytes");
}

#[test]
fn no_vendored_file_has_a_disallowed_extension_or_exceeds_256000_bytes() {
    let mut files = Vec::new();
    walk(&skills_root(), &mut files);
    assert!(!files.is_empty());
    for (rel, path) in files {
        let ext = Path::new(&rel)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        // Licence files at a pack's root or in its `licenses/` folder may
        // have any name.
        let depth = rel.matches('/').count();
        let in_licenses = rel.split('/').nth(1) == Some("licenses");
        let is_license = (depth == 1 || in_licenses)
            && Path::new(&rel)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| {
                    let up = n.to_ascii_uppercase();
                    ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"]
                        .iter()
                        .any(|p| up.starts_with(p))
                });
        assert!(
            ALLOWED_EXTENSIONS.contains(&ext.as_str()) || is_license,
            "disallowed file type: {rel}"
        );
        // pack.json is the generated manifest, not a vendored file.
        if !rel.ends_with("/pack.json") || rel.matches('/').count() != 1 {
            let len = fs::metadata(&path).unwrap().len();
            assert!(len <= MAX_FILE_BYTES, "{rel} is {len} bytes");
        }
    }
}

#[test]
fn default_skills_are_at_most_40_exist_and_are_not_broken() {
    let on = default_on();
    assert!(!on.is_empty());
    assert!(
        on.len() <= MAX_DEFAULT_ON,
        "{} skills on by default",
        on.len()
    );
    let unique: HashSet<&String> = on.iter().collect();
    assert_eq!(unique.len(), on.len(), "duplicate default id");
    let all: HashMap<String, Option<String>> = manifests()
        .into_iter()
        .flat_map(|m| m.skills)
        .map(|s| (s.id, s.broken))
        .collect();
    for id in &on {
        match all.get(id) {
            None => panic!("default skill {id} does not exist"),
            Some(Some(reason)) => panic!("default skill {id} is broken: {reason}"),
            Some(None) => {}
        }
    }
}

#[test]
fn default_skills_include() {
    let on = default_on();
    for id in [
        "superpowers:brainstorming",
        "superpowers:writing-plans",
        "superpowers:systematic-debugging",
        "superpowers:test-driven-development",
        "anthropic:frontend-design",
    ] {
        assert!(on.iter().any(|x| x == id), "{id} should be on by default");
    }
}

#[test]
fn default_skills_exclude() {
    let on = default_on();
    for id in [
        "superpowers:dispatching-parallel-agents",
        "superpowers:subagent-driven-development",
        "superpowers:using-git-worktrees",
    ] {
        assert!(!on.iter().any(|x| x == id), "{id} should be off by default");
    }
}

#[test]
fn every_default_skill_has_an_entry_in_acts_overrides() {
    let overrides: HashMap<String, bool> = read_json("acts-overrides.json");
    let marks: HashMap<String, bool> = manifests()
        .into_iter()
        .flat_map(|m| m.skills)
        .map(|s| (s.id, s.acts_on_computer))
        .collect();
    for id in default_on() {
        let Some(mark) = overrides.get(&id) else {
            panic!("{id} has no acts-overrides entry");
        };
        // The vendoring tool applied the hand-set mark.
        assert_eq!(marks.get(&id), Some(mark), "{id}: pack.json mark differs");
    }
}

#[test]
fn no_skill_id_is_duplicated_across_all_packs() {
    let mut seen = HashSet::new();
    for m in manifests() {
        for s in m.skills {
            assert!(seen.insert(s.id.clone()), "duplicate skill id {}", s.id);
        }
    }
}
