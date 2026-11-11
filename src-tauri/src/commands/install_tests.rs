//! Adding your own skills and installing extra packs, through the command
//! functions. GitHub is a wiremock server; nothing from a pack is executed.

use super::install::{self, AddPreview, AddSource};
use super::tests::{FakeDialogs, Harness, KEY, harness_full, signed_in};
use crate::engine::UiError;
use crate::secrets::SessionStore;
use crate::skills::library::PackView;
use crate::skills::pack::{PackSpec, copy_pack, read_manifest, tree_checksum};
use crate::skills::{PackOrigin, install as packs};
use crate::state::{AppState, HttpClients, HttpTimeouts};
use crate::store::Store;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn skill_text(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\nBody of {name}.\n")
}

fn skill(root: &Path, dir: &str, name: &str) {
    put(
        root,
        &format!("{dir}/SKILL.md"),
        skill_text(name, &format!("Does {name}.")).as_bytes(),
    );
}

/// A harness whose bundled skills are the real tree, so built-in ids exist.
fn local() -> Harness {
    harness_full(
        "http://unused.invalid",
        Arc::new(SessionStore::default()),
        HttpTimeouts::PRODUCTION,
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")),
    )
}

fn skills_dir(h: &Harness) -> PathBuf {
    h.state.skills_dir()
}

/// True when no staging data is left: `.staging` is gone or empty.
fn staging_is_empty(h: &Harness) -> bool {
    let dir = skills_dir(h).join(".staging");
    std::fs::read_dir(&dir).map_or(true, |mut rd| rd.next().is_none())
}

fn yours(h: &Harness) -> Option<PackView> {
    super::skills::list_packs(&h.state)
        .unwrap()
        .into_iter()
        .find(|p| p.id == "yours")
}

async fn preview_folder(h: &Harness, folder: &Path) -> Result<Option<AddPreview>, UiError> {
    *h.dialogs.pick.lock().unwrap() = Some(folder.to_path_buf());
    install::preview_add(&h.state, AddSource::Folder).await
}

async fn add_folder(h: &Harness, folder: &Path) -> PackView {
    let preview = preview_folder(h, folder).await.unwrap().unwrap();
    install::confirm_add(&h.state, &preview.token).unwrap()
}

fn names(p: &PackView) -> Vec<String> {
    p.skills.iter().map(|s| s.name.clone()).collect()
}

fn err(message: &str) -> UiError {
    UiError::other(message)
}

// ---- archive builders ----

fn targz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (name, data) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, name, *data).unwrap();
    }
    gz(b.into_inner().unwrap())
}

/// A tar.gz whose one entry name is written verbatim (the builder refuses `..`).
fn hostile_targz(name: &str) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
    h.set_size(1);
    h.set_mode(0o644);
    h.set_entry_type(tar::EntryType::Regular);
    h.set_cksum();
    b.append(&h, &b"x"[..]).unwrap();
    gz(b.into_inner().unwrap())
}

fn gz(raw: Vec<u8>) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&raw).unwrap();
    enc.finish().unwrap()
}

fn zip_file(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    let p = dir.join("skills.zip");
    std::fs::write(&p, w.finish().unwrap().into_inner()).unwrap();
    p
}

async fn mock_get(server: &MockServer, p: &str, resp: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(resp)
        .mount(server)
        .await;
}

/// GitHub for `o/r`: default branch `main`, `main` and `v1` at `SHA`, and the
/// archive at `SHA`.
async fn github(server: &MockServer, archive: Vec<u8>) {
    mock_get(
        server,
        "/repos/o/r",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"default_branch": "main"})),
    )
    .await;
    for r in ["main", "v1"] {
        mock_get(
            server,
            &format!("/repos/o/r/commits/{r}"),
            ResponseTemplate::new(200).set_body_string(SHA),
        )
        .await;
    }
    mock_get(
        server,
        &format!("/o/r/tar.gz/{SHA}"),
        ResponseTemplate::new(200).set_body_bytes(archive),
    )
    .await;
}

fn repo_archive() -> Vec<u8> {
    let top = format!("o-r-{SHA}");
    let x = skill_text("x", "Does x.");
    let y = skill_text("y", "Does y.");
    targz(&[
        (&format!("{top}/README.md"), b"readme"),
        (&format!("{top}/skills/x/SKILL.md"), x.as_bytes()),
        (&format!("{top}/skills/x/notes.md"), b"x notes"),
        (&format!("{top}/skills/y/SKILL.md"), y.as_bytes()),
    ])
}

// ---- adding from a folder ----

#[tokio::test]
async fn add_folder_with_one_skill() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    let folder = src.path().join("Note Taker");
    skill(&folder, ".", "Note Taker");
    put(&folder, "reference/notes.md", b"notes");

    let preview = preview_folder(&h, &folder).await.unwrap().unwrap();
    assert_eq!(preview.skills.len(), 1);
    assert_eq!(preview.skills[0].id, "note-taker");
    assert_eq!(preview.skills[0].description, "Does Note Taker.");
    // Nothing is added before the user confirms.
    assert!(yours(&h).is_none());

    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    assert_eq!(pack.id, "yours");
    assert_eq!(pack.origin, PackOrigin::Yours);
    assert_eq!(names(&pack), ["note-taker"]);
    assert!(pack.skills[0].on && pack.skills[0].available);
    let d = super::skills::get_skill(&h.state, "note-taker").unwrap();
    assert_eq!(d.files, ["reference/notes.md"]);
    assert!(d.body.contains("Body of Note Taker."));
    assert!(staging_is_empty(&h));
}

#[tokio::test]
async fn add_folder_containing_three_skills() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    let root = src.path();
    skill(root, "a", "a");
    skill(root, "group/b", "b");
    skill(root, "one/two/three/c", "c");
    // Hidden folders and anything deeper than four levels are not looked at.
    skill(root, ".hidden/h", "h");
    skill(root, "one/.git/g", "g");
    skill(root, "one/two/three/four/d", "d");

    let pack = add_folder(&h, root).await;
    assert_eq!(names(&pack), ["a", "b", "c"]);
}

#[tokio::test]
async fn scripts_are_not_copied() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    let folder = src.path().join("tool");
    skill(&folder, ".", "tool");
    put(&folder, "run.sh", b"#!/bin/sh\nrm -rf /\n");
    put(&folder, "helper.py", b"print(1)");
    put(&folder, "data.json", b"{}");
    add_folder(&h, &folder).await;
    let dir = skills_dir(&h).join("yours/tool");
    assert!(dir.join("data.json").is_file());
    assert!(!dir.join("run.sh").exists());
    assert!(!dir.join("helper.py").exists());
}

#[tokio::test]
async fn skill_names_are_sanitized_and_kept_unique() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    skill(src.path(), "a", "superpowers:brainstorming");
    skill(src.path(), "b", "Pack/X");
    skill(src.path(), "c", "pack-x");
    let pack = add_folder(&h, src.path()).await;
    let mut got = names(&pack);
    got.sort();
    assert_eq!(got, ["pack-x", "pack-x-2", "superpowers-brainstorming"]);
    for s in &pack.skills {
        assert!(!s.id.contains(':') && !s.id.contains('/'), "{}", s.id);
    }
}

#[tokio::test]
async fn a_cancelled_dialog_returns_none() {
    let h = local();
    assert_eq!(
        install::preview_add(&h.state, AddSource::Folder).await,
        Ok(None)
    );
    assert_eq!(
        install::preview_add(&h.state, AddSource::Zip).await,
        Ok(None)
    );
    assert!(staging_is_empty(&h));
}

#[tokio::test]
async fn claude_red_style_skills_without_frontmatter_get_names_and_descriptions() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    put(
        src.path(),
        "Skills/web/sql-injection/SKILL.md",
        b"# SQL Injection\n\n## Metadata\n- **Description:** Finds injectable parameters.\n\n## Steps\nLook.\n",
    );
    let pack = add_folder(&h, src.path()).await;
    assert_eq!(pack.skills.len(), 1);
    let s = &pack.skills[0];
    assert_eq!(s.name, "sql-injection");
    assert_eq!(s.description, "Finds injectable parameters.");
    assert!(s.broken.is_none());
}

// ---- ZIP ----

#[tokio::test]
async fn add_zip() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    let z = skill_text("zipped", "From a ZIP.");
    let file = zip_file(
        src.path(),
        &[
            ("bundle/zipped/SKILL.md", z.as_bytes()),
            ("bundle/zipped/guide.md", b"guide"),
            ("bundle/zipped/install.sh", b"echo hi"),
        ],
    );
    *h.dialogs.zip.lock().unwrap() = Some(file);
    let preview = install::preview_add(&h.state, AddSource::Zip)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preview.skills[0].name, "zipped");
    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    assert_eq!(names(&pack), ["zipped"]);
    let d = super::skills::get_skill(&h.state, "zipped").unwrap();
    assert_eq!(d.files, ["guide.md"]);
}

#[tokio::test]
async fn a_zip_with_a_parent_entry_is_refused_and_nothing_is_written() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    let z = skill_text("z", "Z.");
    let file = zip_file(
        src.path(),
        &[("z/SKILL.md", z.as_bytes()), ("../escaped.md", b"x")],
    );
    *h.dialogs.zip.lock().unwrap() = Some(file);
    let r = install::preview_add(&h.state, AddSource::Zip).await;
    assert!(r.is_err(), "{r:?}");
    assert!(staging_is_empty(&h));
    assert!(!skills_dir(&h).join("escaped.md").exists());
    assert!(!skills_dir(&h).join(".staging/escaped.md").exists());
    assert!(yours(&h).is_none());
}

// ---- GitHub ----

#[tokio::test]
async fn add_github() {
    let server = MockServer::start().await;
    github(&server, repo_archive()).await;
    let h = signed_in(&server.uri());
    let preview = install::preview_add(
        &h.state,
        AddSource::Github {
            url: "https://github.com/o/r".into(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    let mut got: Vec<_> = preview.skills.iter().map(|s| s.name.as_str()).collect();
    got.sort();
    assert_eq!(got, ["x", "y"]);
    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    assert_eq!(pack.skills.len(), 2);

    // The app's name as User-Agent; no credential of any kind.
    let requests = server.received_requests().await.unwrap();
    let paths: Vec<_> = requests.iter().map(|r| r.url.path().to_string()).collect();
    assert_eq!(
        paths,
        [
            "/repos/o/r",
            "/repos/o/r/commits/main",
            &format!("/o/r/tar.gz/{SHA}")
        ]
    );
    for r in &requests {
        assert_eq!(
            r.headers.get("user-agent").unwrap().to_str().unwrap(),
            crate::brand::APP_NAME
        );
        assert!(r.headers.get("authorization").is_none());
        assert!(r.headers.get("cookie").is_none());
        for (_, v) in r.headers.iter() {
            assert!(!String::from_utf8_lossy(v.as_bytes()).contains(KEY));
        }
        assert!(!r.url.as_str().contains(KEY));
    }
}

#[tokio::test]
async fn a_github_sub_path_link_adds_only_that_folder() {
    let server = MockServer::start().await;
    github(&server, repo_archive()).await;
    let h = signed_in(&server.uri());
    let preview = install::preview_add(
        &h.state,
        AddSource::Github {
            url: "https://github.com/o/r/tree/v1/skills/x".into(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(preview.skills.len(), 1);
    assert_eq!(preview.skills[0].name, "x");
    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    assert_eq!(names(&pack), ["x"]);
    // A named ref is pinned to its commit without asking for the default branch.
    let requests = server.received_requests().await.unwrap();
    let paths: Vec<_> = requests.iter().map(|r| r.url.path().to_string()).collect();
    assert_eq!(
        paths,
        ["/repos/o/r/commits/v1", &format!("/o/r/tar.gz/{SHA}")]
    );
}

#[tokio::test]
async fn a_link_that_is_not_github_is_refused_with_the_exact_copy() {
    let server = MockServer::start().await;
    let h = signed_in(&server.uri());
    for url in [
        "https://gitlab.com/o/r",
        "http://github.com/o/r",
        "https://github.com/o/r?x=1",
        "https://github.com/o/r/tree/feature/x/../y",
    ] {
        let r = install::preview_add(&h.state, AddSource::Github { url: url.into() }).await;
        assert_eq!(r, Err(err("That is not a GitHub link.")), "{url}");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_github_failure_has_the_exact_copy_and_leaves_nothing() {
    let server = MockServer::start().await;
    mock_get(&server, "/repos/o/r", ResponseTemplate::new(404)).await;
    let h = signed_in(&server.uri());
    let download = err("Could not download from GitHub. Check your connection and the link.");
    let r = install::preview_add(
        &h.state,
        AddSource::Github {
            url: "https://github.com/o/r".into(),
        },
    )
    .await;
    assert_eq!(r, Err(download.clone()));

    // A hostile archive fails the same way and writes nothing.
    let server = MockServer::start().await;
    github(&server, hostile_targz("top/../../escaped.md")).await;
    let h = signed_in(&server.uri());
    let r = install::preview_add(
        &h.state,
        AddSource::Github {
            url: "https://github.com/o/r".into(),
        },
    )
    .await;
    assert_eq!(r, Err(download));
    assert!(staging_is_empty(&h));
    assert!(!skills_dir(&h).join("escaped.md").exists());
}

// ---- limits ----

#[tokio::test]
async fn each_limit_returns_its_exact_copy() {
    let h = local();

    let empty = tempfile::tempdir().unwrap();
    put(empty.path(), "README.md", b"no skills here");
    assert_eq!(
        preview_folder(&h, empty.path()).await,
        Err(err(
            "No skills were found there. A skill is a folder with a SKILL.md file."
        ))
    );

    let many = tempfile::tempdir().unwrap();
    for i in 0..201 {
        skill(many.path(), &format!("s{i}"), &format!("s{i}"));
    }
    assert_eq!(
        preview_folder(&h, many.path()).await,
        Err(err(
            "That source has more than 200 skills. Add a smaller folder."
        ))
    );

    let big = tempfile::tempdir().unwrap();
    skill(big.path(), "big", "big");
    for i in 0..9 {
        put(big.path(), &format!("big/part{i}.md"), &vec![b'a'; 240_000]);
    }
    assert_eq!(
        preview_folder(&h, big.path()).await,
        Err(err("Those skills are too large to add."))
    );
    assert!(staging_is_empty(&h));
    assert!(yours(&h).is_none());

    // Exactly 200 skills is allowed.
    let ok = tempfile::tempdir().unwrap();
    for i in 0..200 {
        skill(ok.path(), &format!("s{i}"), &format!("s{i}"));
    }
    let preview = preview_folder(&h, ok.path()).await.unwrap().unwrap();
    assert_eq!(preview.skills.len(), 200);
}

// ---- tokens ----

#[tokio::test]
async fn a_stale_token_has_expired_and_a_new_preview_replaces_the_old() {
    let h = local();
    let expired = err("That add has expired. Start again.");
    assert_eq!(install::confirm_add(&h.state, "nope"), Err(expired.clone()));

    let src = tempfile::tempdir().unwrap();
    skill(src.path(), "a", "a");
    let first = preview_folder(&h, src.path()).await.unwrap().unwrap();
    let second = preview_folder(&h, src.path()).await.unwrap().unwrap();
    assert_ne!(first.token, second.token);
    assert_eq!(
        std::fs::read_dir(skills_dir(&h).join(".staging"))
            .unwrap()
            .count(),
        1,
        "only the newest preview is kept"
    );
    assert_eq!(
        install::confirm_add(&h.state, &first.token),
        Err(expired.clone())
    );
    // A wrong token does not discard the current preview.
    assert_eq!(
        install::cancel_add(&h.state, &first.token),
        Err(expired.clone())
    );
    install::cancel_add(&h.state, &second.token).unwrap();
    assert!(staging_is_empty(&h));
    assert_eq!(install::confirm_add(&h.state, &second.token), Err(expired));
    assert!(yours(&h).is_none());
}

#[test]
fn staged_data_is_removed_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let leftover = dir.path().join("skills/.staging/old/x.md");
    std::fs::create_dir_all(leftover.parent().unwrap()).unwrap();
    std::fs::write(&leftover, "x").unwrap();
    let http =
        HttpClients::from_builder(reqwest::Client::builder, HttpTimeouts::PRODUCTION).unwrap();
    let _state = AppState::new(
        http,
        "http://unused.invalid".into(),
        Arc::new(SessionStore::default()),
        Store::in_memory().unwrap(),
        dir.path().to_path_buf(),
        dir.path().join("none"),
        Arc::new(NoEvents),
        Arc::new(FakeDialogs::default()),
    );
    assert!(!dir.path().join("skills/.staging").exists());
}

struct NoEvents;
impl crate::state::EventSink for NoEvents {
    fn emit(&self, _: &str, _: serde_json::Value) {}
}

// ---- replacing and removing ----

#[tokio::test]
async fn adding_a_skill_twice_replaces_it() {
    let h = local();
    let one = tempfile::tempdir().unwrap();
    put(
        one.path(),
        "w/SKILL.md",
        skill_text("w", "First.").as_bytes(),
    );
    put(one.path(), "w/old.md", b"old");
    skill(one.path(), "keep", "keep");
    add_folder(&h, one.path()).await;

    let two = tempfile::tempdir().unwrap();
    put(
        two.path(),
        "w/SKILL.md",
        skill_text("w", "Second.").as_bytes(),
    );
    put(two.path(), "w/new.md", b"new");
    let pack = add_folder(&h, two.path()).await;

    assert_eq!(names(&pack), ["keep", "w"]);
    let d = super::skills::get_skill(&h.state, "w").unwrap();
    assert_eq!(d.skill.description, "Second.");
    assert_eq!(d.files, ["new.md"]);
    assert!(!skills_dir(&h).join("yours/w/old.md").exists());
    // pack.json lists exactly the skill folders present.
    let m = read_manifest(&skills_dir(&h).join("yours")).unwrap();
    let listed: Vec<_> = m.skills.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(listed, ["keep", "w"]);
    assert!(staging_is_empty(&h));
}

#[tokio::test]
async fn remove_skill_deletes_only_that_skill_and_refuses_built_in_ids() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    skill(src.path(), "a", "a");
    skill(src.path(), "b", "b");
    add_folder(&h, src.path()).await;
    super::skills::set_skill_on(&h.state, "a", false).unwrap();

    install::remove_skill(&h.state, "a").unwrap();
    assert_eq!(names(&yours(&h).unwrap()), ["b"]);
    assert!(!skills_dir(&h).join("yours/a").exists());
    assert_eq!(h.state.store.switch("a").unwrap(), None);
    let m = read_manifest(&skills_dir(&h).join("yours")).unwrap();
    assert_eq!(m.skills.len(), 1);

    let refused = err("Only skills you added can be removed.");
    for id in [
        "superpowers:brainstorming",
        "a",
        "../yours",
        "b/../b",
        "",
        "yours:b",
    ] {
        assert_eq!(
            install::remove_skill(&h.state, id),
            Err(refused.clone()),
            "{id}"
        );
    }
    assert!(super::skills::get_skill(&h.state, "superpowers:brainstorming").is_ok());

    // Removing the last skill removes the pack.
    install::remove_skill(&h.state, "b").unwrap();
    assert!(yours(&h).is_none());
    assert!(!skills_dir(&h).join("yours").exists());
}

#[tokio::test]
async fn a_failed_confirm_leaves_nothing_behind() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    skill(src.path(), "a", "a");
    let preview = preview_folder(&h, src.path()).await.unwrap().unwrap();
    // A file where the user's pack folder should be makes the move fail.
    std::fs::write(skills_dir(&h).join("yours"), "not a folder").unwrap();
    let r = install::confirm_add(&h.state, &preview.token);
    assert!(r.is_err(), "{r:?}");
    assert!(staging_is_empty(&h));
    assert_eq!(
        std::fs::read_to_string(skills_dir(&h).join("yours")).unwrap(),
        "not a folder"
    );
    // The staged add is gone with it.
    assert_eq!(
        install::confirm_add(&h.state, &preview.token),
        Err(err("That add has expired. Start again."))
    );
}

// ---- extra packs ----

#[test]
fn extra_packs_lists_three_with_needs_warning_only_for_claude_red() {
    let h = local();
    let packs = install::extra_packs(&h.state).unwrap();
    let got: Vec<_> = packs
        .iter()
        .map(|p| (p.id.as_str(), p.needs_warning, p.installed))
        .collect();
    assert_eq!(
        got,
        [
            ("trailofbits", false, false),
            ("vercel", false, false),
            ("claude-red", true, false),
        ]
    );
    let v = serde_json::to_value(&packs[0]).unwrap();
    let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "about",
            "id",
            "installed",
            "license",
            "needs_warning",
            "repo",
            "title"
        ]
    );
    assert_eq!(packs[1].repo, "vercel-labs/agent-skills");
}

/// An extra pack's archive: two skills, a script, and a license.
fn pack_archive(top: &str) -> Vec<u8> {
    let a = skill_text("alpha", "Does alpha.");
    let b = "# Beta\n\nDoes beta without frontmatter.\n";
    targz(&[
        (&format!("{top}/LICENSE"), b"license text"),
        (&format!("{top}/skills/alpha/SKILL.md"), a.as_bytes()),
        (&format!("{top}/skills/alpha/ref.md"), b"ref"),
        (&format!("{top}/skills/alpha/run.sh"), b"echo"),
        (&format!("{top}/skills/beta/SKILL.md"), b.as_bytes()),
    ])
}

/// The checksum `copy_pack` gives for `pack_archive`.
fn pack_archive_checksum(spec: &PackSpec) -> String {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    put(&src, "LICENSE", b"license text");
    put(
        &src,
        "skills/alpha/SKILL.md",
        skill_text("alpha", "Does alpha.").as_bytes(),
    );
    put(&src, "skills/alpha/ref.md", b"ref");
    put(&src, "skills/alpha/run.sh", b"echo");
    put(
        &src,
        "skills/beta/SKILL.md",
        b"# Beta\n\nDoes beta without frontmatter.\n",
    );
    copy_pack(&src, spec, &t.path().join("out"), &HashMap::new()).unwrap();
    tree_checksum(&t.path().join("out"))
}

fn vercel_spec(h: &Harness) -> PackSpec {
    h.state
        .library
        .lock_specs()
        .iter()
        .find(|p| p.id == "vercel")
        .cloned()
        .unwrap()
}

async fn serve_pack(server: &MockServer, spec: &PackSpec, archive: Vec<u8>) {
    mock_get(
        server,
        &format!("/{}/tar.gz/{}", spec.repo, spec.commit),
        ResponseTemplate::new(200).set_body_bytes(archive),
    )
    .await;
}

#[tokio::test]
async fn install_extra_pack_copies_text_files_writes_pack_json_and_starts_all_skills_off() {
    let server = MockServer::start().await;
    let h = harness_full(
        &server.uri(),
        Arc::new(SessionStore::default()),
        HttpTimeouts::PRODUCTION,
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")),
    );
    // The lock's real checksum cannot match a test archive, so this installs
    // with the same spec carrying the test archive's checksum.
    let mut spec = vercel_spec(&h);
    spec.select = vec!["skills/*".into()];
    spec.checksum = pack_archive_checksum(&spec);
    serve_pack(&server, &spec, pack_archive("top")).await;
    packs::install_pack(
        &h.state.github_http,
        &server.uri(),
        &spec,
        &skills_dir(&h),
        &Mutex::new(()),
    )
    .await
    .unwrap();
    h.state.library.reload();

    let dir = skills_dir(&h).join("packs/vercel");
    assert!(dir.join("skills/alpha/SKILL.md").is_file());
    assert!(dir.join("skills/alpha/ref.md").is_file());
    assert!(!dir.join("skills/alpha/run.sh").exists());
    let m = read_manifest(&dir).unwrap();
    assert_eq!(m.checksum, spec.checksum);
    let pack = super::skills::list_packs(&h.state)
        .unwrap()
        .into_iter()
        .find(|p| p.id == "vercel")
        .unwrap();
    assert_eq!(pack.origin, PackOrigin::Installed);
    assert_eq!(pack.title, "Vercel");
    assert_eq!(pack.skills.len(), 2);
    assert!(pack.skills.iter().all(|s| !s.on && !s.available));
    let beta = pack.skills.iter().find(|s| s.name == "beta").unwrap();
    assert_eq!(beta.description, "Does beta without frontmatter.");
    let listed = install::extra_packs(&h.state).unwrap();
    assert!(listed.iter().any(|p| p.id == "vercel" && p.installed));
    assert!(staging_is_empty(&h));

    // remove_pack deletes it and forgets its switches.
    super::skills::set_pack_on(&h.state, "vercel", false).unwrap();
    super::skills::set_skill_on(&h.state, "vercel:alpha", true).unwrap();
    install::remove_pack(&h.state, "vercel").unwrap();
    assert!(!dir.exists());
    assert!(
        !super::skills::list_packs(&h.state)
            .unwrap()
            .iter()
            .any(|p| p.id == "vercel")
    );
    assert_eq!(h.state.store.switch("pack/vercel").unwrap(), None);
    assert_eq!(h.state.store.switch("vercel:alpha").unwrap(), None);
    assert!(staging_is_empty(&h));
}

#[tokio::test]
async fn removing_a_built_in_pack_yours_or_an_unknown_pack_is_refused() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    skill(src.path(), "a", "a");
    add_folder(&h, src.path()).await;
    let refused = err("Only packs you installed can be removed.");
    for id in [
        "superpowers",
        "yours",
        "vercel",
        "../yours",
        "",
        "packs/../yours",
    ] {
        assert_eq!(
            install::remove_pack(&h.state, id),
            Err(refused.clone()),
            "{id}"
        );
    }
    assert!(yours(&h).is_some());
}

#[tokio::test]
async fn install_extra_pack_with_a_wrong_checksum_is_refused_and_leaves_nothing_behind() {
    let server = MockServer::start().await;
    let h = harness_full(
        &server.uri(),
        Arc::new(SessionStore::default()),
        HttpTimeouts::PRODUCTION,
        None,
    );
    let spec = vercel_spec(&h);
    serve_pack(&server, &spec, pack_archive("top")).await;
    let r = install::install_extra_pack(&h.state, "vercel", false).await;
    assert_eq!(
        r,
        Err(err(
            "The downloaded pack did not match its recorded checksum, so it was not installed."
        ))
    );
    assert!(!skills_dir(&h).join("packs/vercel").exists());
    assert!(staging_is_empty(&h));
    assert!(
        !install::extra_packs(&h.state)
            .unwrap()
            .iter()
            .any(|p| p.installed)
    );
}

#[tokio::test]
async fn a_failed_install_leaves_no_partial_folder() {
    let server = MockServer::start().await;
    let h = harness_full(
        &server.uri(),
        Arc::new(SessionStore::default()),
        HttpTimeouts::PRODUCTION,
        None,
    );
    let mut spec = vercel_spec(&h);
    spec.checksum = pack_archive_checksum(&spec);
    serve_pack(&server, &spec, pack_archive("top")).await;

    // The final move fails: a file sits where the packs folder goes.
    std::fs::create_dir_all(skills_dir(&h)).unwrap();
    std::fs::write(skills_dir(&h).join("packs"), "x").unwrap();
    let r = packs::install_pack(
        &h.state.github_http,
        &server.uri(),
        &spec,
        &skills_dir(&h),
        &Mutex::new(()),
    )
    .await;
    assert!(r.is_err(), "{r:?}");
    assert!(staging_is_empty(&h));
    assert!(skills_dir(&h).join("packs").is_file());
    std::fs::remove_file(skills_dir(&h).join("packs")).unwrap();

    // A hostile archive.
    let server = MockServer::start().await;
    serve_pack(&server, &spec, hostile_targz("top/../../../escaped.md")).await;
    let r = packs::install_pack(
        &h.state.github_http,
        &server.uri(),
        &spec,
        &skills_dir(&h),
        &Mutex::new(()),
    )
    .await;
    assert_eq!(
        r,
        Err(err(
            "Could not download from GitHub. Check your connection and the link."
        ))
    );
    assert!(staging_is_empty(&h));
    assert!(!skills_dir(&h).join("packs/vercel").exists());
    assert!(!h._dir.path().join("escaped.md").exists());

    // A download failure.
    let server = MockServer::start().await;
    let r = packs::install_pack(
        &h.state.github_http,
        &server.uri(),
        &spec,
        &skills_dir(&h),
        &Mutex::new(()),
    )
    .await;
    assert!(r.is_err());
    assert!(staging_is_empty(&h));
    assert!(!skills_dir(&h).join("packs/vercel").exists());
}

#[tokio::test]
async fn install_extra_pack_refuses_claude_red_without_acknowledgement_and_unknown_or_built_in_ids()
{
    let server = MockServer::start().await;
    let h = harness_full(
        &server.uri(),
        Arc::new(SessionStore::default()),
        HttpTimeouts::PRODUCTION,
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")),
    );
    assert_eq!(
        install::install_extra_pack(&h.state, "claude-red", false).await,
        Err(err(
            "Read and accept the notice before installing this pack."
        ))
    );
    let unavailable = err("This pack is not available. Check it in Skills.");
    for id in ["superpowers", "yours", "nope", "../x"] {
        assert_eq!(
            install::install_extra_pack(&h.state, id, true).await,
            Err(unavailable.clone()),
            "{id}"
        );
    }
    // Nothing was downloaded for any refusal.
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ---- fix round 1 ----

/// Every file name under `dir`, relative, with `/` separators.
fn all_files(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let here = format!("{rel}{}", e.file_name().to_string_lossy());
            out.push(here.clone());
            if e.file_type().unwrap().is_dir() {
                walk(&e.path(), &format!("{here}/"), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out
}

#[tokio::test]
async fn a_single_skill_folder_never_reads_or_reports_its_parent() {
    let h = local();
    let parent = tempfile::tempdir().unwrap();
    let p = parent.path();
    put(p, "LICENSE", b"parent license");
    put(p, "NOTICE.md", &[0xff, 0xfe, 0x00]);
    put(p, "THIRD_PARTY_NOTICES.txt", &vec![b'a'; 300_000]);
    put(p, "licenses/x.pdf", b"%PDF");
    put(p, "licenses/deep/LICENSE.txt", b"deep");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/hosts", p.join("LICENSE-link")).unwrap();
        // Unreadable: opening it would fail and be reported.
        put(p, "LICENSE.secret", b"secret");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            p.join("LICENSE.secret"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
    }
    let folder = p.join("tool");
    skill(&folder, ".", "tool");
    put(&folder, "guide.md", b"guide");
    put(&folder, "big.md", &vec![b'a'; 300_000]);

    let preview = preview_folder(&h, &folder).await.unwrap().unwrap();
    // Only paths inside the chosen folder are reported.
    assert_eq!(preview.skipped, ["tool/big.md"]);
    let staged = all_files(&skills_dir(&h).join(".staging"));
    for f in &staged {
        let up = f.to_ascii_uppercase();
        assert!(
            !up.contains("LICENSE") && !up.contains("NOTICE") && !f.contains("x.pdf"),
            "parent file staged: {f}"
        );
    }
    assert!(
        staged.iter().any(|f| f.ends_with("tool/guide.md")),
        "{staged:?}"
    );
    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    assert_eq!(names(&pack), ["tool"]);
}

const NO_NAME: &[u8] = b"# Handy\n\nDoes handy things.\n";

#[tokio::test]
async fn the_fallback_name_of_a_single_skill_folder_is_its_own_name() {
    let h = local();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("My Tool");
    put(&folder, "SKILL.md", NO_NAME);
    let preview = preview_folder(&h, &folder).await.unwrap().unwrap();
    assert_eq!(preview.skills[0].name, "my-tool");
}

#[tokio::test]
async fn the_fallback_name_of_a_zip_whose_root_is_a_skill_is_the_zip_name() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    let file = zip_file(src.path(), &[("SKILL.md", NO_NAME), ("ref.md", b"r")]);
    let named = src.path().join("Handy Helper.zip");
    std::fs::rename(&file, &named).unwrap();
    *h.dialogs.zip.lock().unwrap() = Some(named);
    let preview = install::preview_add(&h.state, AddSource::Zip)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preview.skills.len(), 1);
    assert_eq!(preview.skills[0].name, "handy-helper");
    install::confirm_add(&h.state, &preview.token).unwrap();
    let d = super::skills::get_skill(&h.state, "handy-helper").unwrap();
    assert_eq!(d.files, ["ref.md"]);
}

#[tokio::test]
async fn the_fallback_name_of_a_repository_whose_root_is_a_skill_is_the_repository_name() {
    let server = MockServer::start().await;
    let top = format!("o-r-{SHA}");
    github(
        &server,
        targz(&[
            (&format!("{top}/SKILL.md"), NO_NAME),
            (&format!("{top}/notes.md"), b"n"),
        ]),
    )
    .await;
    let h = signed_in(&server.uri());
    let preview = install::preview_add(
        &h.state,
        AddSource::Github {
            url: "https://github.com/o/r.git".into(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(preview.skills.len(), 1);
    assert_eq!(preview.skills[0].name, "r");
}

#[tokio::test]
async fn an_add_of_more_than_ten_skills_stores_them_all_off() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    for i in 0..11 {
        skill(src.path(), &format!("s{i}"), &format!("s{i}"));
    }
    let preview = preview_folder(&h, src.path()).await.unwrap().unwrap();
    assert_eq!(preview.skills.len(), 11);
    assert!(preview.skills.iter().all(|s| !s.on && !s.available));
    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    assert_eq!(pack.skills.len(), 11);
    assert!(pack.skills.iter().all(|s| !s.on));
    for s in &pack.skills {
        assert_eq!(
            h.state.store.switch(&s.id).unwrap(),
            Some(false),
            "{}",
            s.id
        );
    }
}

#[tokio::test]
async fn an_add_of_ten_or_fewer_skills_starts_on() {
    let h = local();
    let src = tempfile::tempdir().unwrap();
    for i in 0..3 {
        skill(src.path(), &format!("s{i}"), &format!("s{i}"));
    }
    let preview = preview_folder(&h, src.path()).await.unwrap().unwrap();
    assert!(preview.skills.iter().all(|s| s.on && s.available));
    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    assert!(pack.skills.iter().all(|s| s.on));
}

#[tokio::test]
async fn replacing_a_switched_off_skill_keeps_it_off_and_the_preview_says_so() {
    let h = local();
    let one = tempfile::tempdir().unwrap();
    skill(one.path(), "w", "w");
    skill(one.path(), "v", "v");
    add_folder(&h, one.path()).await;
    super::skills::set_skill_on(&h.state, "w", false).unwrap();

    let preview = preview_folder(&h, one.path()).await.unwrap().unwrap();
    let on: Vec<_> = preview
        .skills
        .iter()
        .map(|s| (s.id.as_str(), s.on))
        .collect();
    assert_eq!(on, [("v", true), ("w", false)]);
    let pack = install::confirm_add(&h.state, &preview.token).unwrap();
    let on: Vec<_> = pack.skills.iter().map(|s| (s.id.as_str(), s.on)).collect();
    assert_eq!(on, [("v", true), ("w", false)]);
}

#[tokio::test]
async fn a_github_link_to_a_listed_pack_is_refused_without_a_request() {
    let server = MockServer::start().await;
    let h = signed_in(&server.uri());
    for url in [
        "https://github.com/SnailSploit/Claude-Red",
        "https://github.com/snailsploit/claude-red.git",
        "https://github.com/SNAILSPLOIT/CLAUDE-RED/tree/main/Skills/web",
        "https://github.com/obra/superpowers/tree/main/skills/brainstorming",
        "https://github.com/vercel-labs/agent-skills",
    ] {
        let r = install::preview_add(&h.state, AddSource::Github { url: url.into() }).await;
        assert_eq!(
            r,
            Err(err(
                "That pack is already listed in Skills. Install it from there."
            )),
            "{url}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// A harness whose GitHub client accepts redirects only to `allowed`.
fn redirect_harness(
    server: &MockServer,
    allowed: Vec<String>,
) -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let http = HttpClients::from_builder_for(
        || reqwest::Client::builder().no_proxy(),
        HttpTimeouts::PRODUCTION,
        allowed,
    )
    .unwrap();
    let state = AppState::new(
        http,
        server.uri(),
        Arc::new(SessionStore::default()),
        Store::in_memory().unwrap(),
        dir.path().to_path_buf(),
        dir.path().join("none"),
        Arc::new(NoEvents),
        Arc::new(FakeDialogs::default()),
    )
    .with_github(crate::state::GithubBases {
        api: server.uri(),
        codeload: server.uri(),
    });
    (Arc::new(state), dir)
}

fn redirect_to(location: &str) -> ResponseTemplate {
    ResponseTemplate::new(302).insert_header("location", location)
}

#[tokio::test]
async fn github_redirects_to_another_host_are_not_followed() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    github(&elsewhere, repo_archive()).await;
    mock_get(
        &server,
        "/repos/o/r",
        redirect_to(&format!("{}/repos/o/r", elsewhere.uri())),
    )
    .await;
    let (state, _dir) = redirect_harness(&server, vec![server.uri()]);
    let r = install::preview_add(
        &state,
        AddSource::Github {
            url: "https://github.com/o/r".into(),
        },
    )
    .await;
    assert_eq!(
        r,
        Err(err(
            "Could not download from GitHub. Check your connection and the link."
        ))
    );
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn github_redirects_on_an_allowed_host_are_followed_up_to_five_hops() {
    // Five hops: followed.
    let server = MockServer::start().await;
    github(&server, repo_archive()).await;
    let hops = ["/repos/o/r/commits/v2", "/h1", "/h2", "/h3", "/h4"];
    for (i, from) in hops.iter().enumerate() {
        let to = hops
            .get(i + 1)
            .copied()
            .unwrap_or("/repos/o/r/commits/main");
        mock_get(&server, from, redirect_to(&format!("{}{to}", server.uri()))).await;
    }
    let (state, _dir) = redirect_harness(&server, vec![server.uri()]);
    let r = install::preview_add(
        &state,
        AddSource::Github {
            url: "https://github.com/o/r/tree/v2".into(),
        },
    )
    .await;
    assert!(matches!(r, Ok(Some(_))), "{r:?}");
    let paths: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(paths.contains(&"/h4".to_string()), "{paths:?}");

    // Six hops: stopped.
    let server = MockServer::start().await;
    github(&server, repo_archive()).await;
    let hops = ["/repos/o/r/commits/v2", "/h1", "/h2", "/h3", "/h4", "/h5"];
    for (i, from) in hops.iter().enumerate() {
        let to = hops
            .get(i + 1)
            .copied()
            .unwrap_or("/repos/o/r/commits/main");
        mock_get(&server, from, redirect_to(&format!("{}{to}", server.uri()))).await;
    }
    let (state, _dir) = redirect_harness(&server, vec![server.uri()]);
    let r = install::preview_add(
        &state,
        AddSource::Github {
            url: "https://github.com/o/r/tree/v2".into(),
        },
    )
    .await;
    assert_eq!(
        r,
        Err(err(
            "Could not download from GitHub. Check your connection and the link."
        ))
    );
}

#[test]
fn the_production_github_client_allows_only_the_three_github_hosts_over_https() {
    assert_eq!(
        crate::state::GITHUB_ORIGINS,
        [
            "https://github.com",
            "https://api.github.com",
            "https://codeload.github.com"
        ]
    );
}
