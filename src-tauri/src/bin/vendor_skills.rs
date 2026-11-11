//! Developer tool: regenerate the vendored skill packs from `skills.lock.json`.
//!
//! ```text
//! cargo run --manifest-path src-tauri/Cargo.toml --features vendor-tool --bin vendor-skills
//! cargo run --manifest-path src-tauri/Cargo.toml --features vendor-tool --bin vendor-skills -- --check
//! ```
//!
//! Without arguments it downloads each bundled pack's GitHub archive at its
//! pinned commit into a temporary folder, copies only the pack's text files
//! into `src-tauri/resources/skills/<id>/` with `copy_pack`, and records each
//! pack's checksum in the lock file. `--check` downloads nothing: it re-reads
//! the vendored tree and fails if any checksum differs from the lock.
//!
//! Pack contents are untrusted data. This tool only downloads, unpacks and
//! copies files; it never runs anything from a pack.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use beluga_desktop_lib::skills::archive::{Limits, download_github_archive};
use beluga_desktop_lib::skills::pack::{
    PackSpec, copy_pack, load_lock, read_manifest, tree_checksum,
};

const CODELOAD: &str = "https://codeload.github.com";
const OVERRIDES_FILE: &str = "acts-overrides.json";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent folder")
        .to_path_buf()
}

fn lock_path() -> PathBuf {
    repo_root().join("skills.lock.json")
}

fn skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")
}

fn read_lock() -> Result<Vec<PackSpec>, String> {
    let path = lock_path();
    let text =
        fs::read_to_string(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    load_lock(&text)
}

fn write_lock(packs: &[PackSpec]) -> Result<(), String> {
    let value = serde_json::json!({ "version": 1, "packs": packs });
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n";
    fs::write(lock_path(), text).map_err(|e| format!("Could not write the lock file: {e}"))
}

/// The hand-set acts-on-your-computer marks; absent file means none.
fn read_overrides() -> Result<HashMap<String, bool>, String> {
    let path = skills_dir().join(OVERRIDES_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| format!("{OVERRIDES_FILE} is not valid: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(format!("Could not read {OVERRIDES_FILE}: {e}")),
    }
}

/// Re-reads the vendored tree and lists every pack whose files or manifest
/// do not match the lock.
fn check() -> Result<(), String> {
    let mut problems = Vec::new();
    for spec in read_lock()?.iter().filter(|p| p.bundled) {
        let dir = skills_dir().join(&spec.id);
        if !dir.is_dir() {
            problems.push(format!("{}: not vendored", spec.id));
            continue;
        }
        let tree = tree_checksum(&dir);
        if tree != spec.checksum {
            problems.push(format!("{}: files differ from the lock", spec.id));
        }
        match read_manifest(&dir) {
            Err(e) => problems.push(format!("{}: {e}", spec.id)),
            Ok(m) if m.checksum != spec.checksum || m.commit != spec.commit => {
                problems.push(format!("{}: pack.json differs from the lock", spec.id));
            }
            Ok(_) => {}
        }
    }
    if problems.is_empty() {
        println!("All vendored packs match skills.lock.json.");
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Downloads and copies every bundled pack, writing the lock after each.
async fn vendor() -> Result<(), String> {
    let mut packs = read_lock()?;
    let overrides = read_overrides()?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .user_agent("vendor-skills")
        .build()
        .map_err(|e| format!("Could not set up the HTTP client: {e}"))?;
    let limits = Limits::default();
    fs::create_dir_all(skills_dir()).map_err(|e| format!("Could not create the folder: {e}"))?;

    for i in 0..packs.len() {
        if !packs[i].bundled {
            continue;
        }
        let spec = packs[i].clone();
        println!("{}: downloading {} at {}", spec.id, spec.repo, spec.commit);
        let tmp = tempfile::TempDir::new().map_err(|e| format!("No temporary folder: {e}"))?;
        let download = tmp.path().join("download");
        let src = download_github_archive(
            &http,
            CODELOAD,
            &spec.repo,
            &spec.commit,
            &download,
            &limits,
        )
        .await
        .map_err(|e| format!("{}: {e}", spec.id))?;

        let dest = skills_dir().join(&spec.id);
        if dest.exists() {
            fs::remove_dir_all(&dest)
                .map_err(|e| format!("Could not clear {}: {e}", dest.display()))?;
        }
        let manifest =
            copy_pack(&src, &spec, &dest, &overrides).map_err(|e| format!("{}: {e}", spec.id))?;
        println!(
            "{}: {} skills, {} files skipped",
            spec.id,
            manifest.skills.len(),
            manifest.skipped.len()
        );
        packs[i].checksum = manifest.checksum;
        write_lock(&packs)?;
    }

    let bundled: Vec<&str> = packs
        .iter()
        .filter(|p| p.bundled)
        .map(|p| p.id.as_str())
        .collect();
    if let Ok(rd) = fs::read_dir(skills_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !bundled.contains(&name.as_str()) {
                eprintln!("warning: {name} is not a bundled pack in the lock file");
            }
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => vendor().await,
        [flag] if flag == "--check" => check(),
        _ => Err("usage: vendor-skills [--check]".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
