//! Temporary folders for installs and adds. They live in
//! `<app data>/skills/.staging/`, on the same volume as the packs, so moving
//! a finished copy into place is a rename.

use std::fs;
use std::path::{Path, PathBuf};

pub const STAGING_DIR: &str = ".staging";

/// Removes every leftover staging folder, e.g. at startup.
pub fn clean(skills_dir: &Path) {
    let _ = fs::remove_dir_all(skills_dir.join(STAGING_DIR));
}

/// One staging folder; removed, with everything in it, when dropped.
#[derive(Debug)]
pub struct Stage {
    dir: PathBuf,
}

impl Stage {
    pub fn new(skills_dir: &Path) -> std::io::Result<Stage> {
        let dir = skills_dir
            .join(STAGING_DIR)
            .join(uuid::Uuid::new_v4().simple().to_string());
        fs::create_dir_all(&dir)?;
        Ok(Stage { dir })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
        // Only succeeds once no other stage is left.
        if let Some(parent) = self.dir.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}

/// An add the user has previewed but not yet confirmed.
#[derive(Debug)]
pub struct StagedAdd {
    pub token: String,
    pub stage: Stage,
    /// The staged skills' ids (their folder names).
    pub names: Vec<String>,
}

/// Moves `new` to `dest`, first moving an existing `dest` aside to `old`.
/// On failure the previous `dest` is put back. Returns whether there was one.
pub fn swap_into(new: &Path, dest: &Path, old: &Path) -> std::io::Result<bool> {
    let had_old = fs::symlink_metadata(dest).is_ok();
    if had_old {
        if let Some(parent) = old.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(dest, old)?;
    }
    if let Err(e) = fs::rename(new, dest) {
        if had_old {
            let _ = fs::rename(old, dest);
        }
        return Err(e);
    }
    Ok(had_old)
}

/// Undoes a `swap_into`, best effort.
pub fn swap_back(new: &Path, dest: &Path, old: &Path, had_old: bool) {
    let _ = fs::rename(dest, new);
    if had_old {
        let _ = fs::rename(old, dest);
    }
}

/// Total length of the regular files under `dir`; links are not followed.
pub fn tree_bytes(dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => tree_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}
