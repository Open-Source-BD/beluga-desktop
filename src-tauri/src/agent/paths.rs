//! Turning a path the model supplied into a real path inside the project.
//!
//! The model (possibly misled by text inside a file) chooses these strings,
//! so nothing here trusts them. A path is first normalised to plain name
//! segments, then checked on the real, resolved path by `Path::components`
//! (`Path::starts_with`), never by string prefix. `~` is never expanded: it is
//! an ordinary directory name. Every refusal is one of two fixed messages.
//!
//! Time of check versus time of use: these functions return a path that was
//! valid at the moment they ran. A caller that writes must call `for_write`
//! again immediately before touching the disk, write only to the returned
//! `real`, and open the file without following a final link (for example
//! `O_NOFOLLOW`, or create-new for a new file). Nothing here creates or locks
//! anything.

use std::path::{Component, Path, PathBuf};

pub const OUTSIDE: &str = "That path is outside the project folder.";
pub const UNUSABLE: &str = "That path cannot be used.";

/// The canonical folder of a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
}

/// A resolved path. `shown` is relative to the root with forward slashes
/// (empty for the root itself) and never carries a drive or `\\?\` prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InProject {
    pub real: PathBuf,
    pub shown: String,
}

/// Names Windows treats as devices, with or without an extension.
const DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A name that is safe on every system: no `:` (drive or stream), no control
/// characters, no trailing space or dot (Windows drops them, so `.. ` would
/// become `..`), and not a device name.
fn plain_name(seg: &str) -> bool {
    if seg.is_empty()
        || seg.contains(':')
        || seg.chars().any(char::is_control)
        || seg.ends_with([' ', '.'])
    {
        return false;
    }
    let stem = seg.split('.').next().unwrap_or(seg).to_lowercase();
    !DEVICE_NAMES.contains(&stem.as_str())
}

/// `real` relative to `root` as forward-slash text.
fn shown_of(root: &Path, real: &Path) -> String {
    real.strip_prefix(root)
        .map(|rel| {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

impl Project {
    /// Opens a folder: it must exist and be a directory.
    pub fn open(folder: &str) -> Result<Project, String> {
        const GONE: &str = "The project folder is missing or is not a folder.";
        let root = std::fs::canonicalize(folder).map_err(|_| GONE.to_string())?;
        if root.is_dir() {
            Ok(Project { root })
        } else {
            Err(GONE.to_string())
        }
    }

    /// For reading: the path must exist and, once every link is followed,
    /// lie inside the project.
    pub fn existing(&self, given: &str) -> Result<InProject, String> {
        let segs = self.segments(given)?;
        let mut joined = self.root.clone();
        joined.extend(&segs);
        let real = std::fs::canonicalize(&joined).map_err(|_| UNUSABLE.to_string())?;
        if !real.starts_with(&self.root) {
            return Err(OUTSIDE.to_string());
        }
        let shown = shown_of(&self.root, &real);
        Ok(InProject { real, shown })
    }

    /// For writing a file that may not exist yet (and whose folders may not
    /// exist yet). The nearest existing ancestor is resolved and must be
    /// inside; the rest are plain names. The target, if present, must be a
    /// regular file, and no component, the last included, may be a link.
    /// Creates nothing. See the module note on re-checking before the write.
    pub fn for_write(&self, given: &str) -> Result<InProject, String> {
        if given.ends_with(['/', '\\']) {
            return Err(UNUSABLE.to_string()); // names a folder
        }
        let segs = self.segments(given)?;
        // The root must still be there, as itself, or "nothing exists below
        // it" would wrongly read as "free to create".
        if std::fs::canonicalize(&self.root).ok().as_ref() != Some(&self.root) {
            return Err(UNUSABLE.to_string());
        }
        if segs.is_empty() {
            return Err(UNUSABLE.to_string());
        }
        let mut cur = self.root.clone();
        for (i, seg) in segs.iter().enumerate() {
            let last = i + 1 == segs.len();
            let next = cur.join(seg);
            let meta = match std::fs::symlink_metadata(&next) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Nothing from here on exists; the names are plain.
                    let mut real = cur.clone();
                    real.extend(&segs[i..]);
                    let shown = shown_of(&self.root, &real);
                    return Ok(InProject { real, shown });
                }
                Err(_) => return Err(UNUSABLE.to_string()),
            };
            let real = std::fs::canonicalize(&next).map_err(|_| UNUSABLE.to_string())?;
            if !real.starts_with(&self.root) {
                return Err(OUTSIDE.to_string());
            }
            // A link (or a Windows junction, which resolves elsewhere) is
            // never written through, even when it points inside.
            if meta.file_type().is_symlink() || real.parent() != Some(cur.as_path()) {
                return Err(UNUSABLE.to_string());
            }
            let ok = if last { meta.is_file() } else { meta.is_dir() };
            if !ok {
                return Err(UNUSABLE.to_string());
            }
            cur = real;
        }
        let shown = shown_of(&self.root, &cur);
        Ok(InProject { real: cur, shown })
    }

    /// The plain name segments of `given` below the root.
    fn segments(&self, given: &str) -> Result<Vec<String>, String> {
        if given.is_empty() || given.contains('\0') {
            return Err(UNUSABLE.to_string());
        }
        // `\\server\share`, `//server/share`, `\\?\C:\x`, a rooted `\x`.
        if given.starts_with(['\\']) || given.starts_with("//") {
            return Err(UNUSABLE.to_string());
        }
        let path = Path::new(given);
        let segs: Vec<String> = if path.is_absolute() {
            self.absolute_segments(given, path)?
        } else {
            if given.starts_with('/') {
                return Err(UNUSABLE.to_string()); // rooted without a drive
            }
            let mut out = Vec::new();
            for seg in given.split(['/', '\\']) {
                match seg {
                    "" | "." => {}
                    ".." => return Err(OUTSIDE.to_string()),
                    s => out.push(s.to_string()),
                }
            }
            out
        };
        if segs.iter().all(|s| plain_name(s)) {
            Ok(segs)
        } else {
            Err(UNUSABLE.to_string())
        }
    }

    /// An absolute `given`, as segments below the root. The nearest existing
    /// ancestor is canonicalized (which also gives the on-disk spelling of
    /// the root) and must lie inside; the rest must be plain names.
    fn absolute_segments(&self, given: &str, path: &Path) -> Result<Vec<String>, String> {
        if cfg!(not(windows)) && given.contains('\\') {
            return Err(UNUSABLE.to_string());
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(OUTSIDE.to_string());
        }
        let mut anc = path;
        let mut tail = Vec::new();
        let base = loop {
            if let Ok(c) = std::fs::canonicalize(anc) {
                break c;
            }
            let name = anc.file_name().ok_or_else(|| UNUSABLE.to_string())?;
            tail.push(name.to_string_lossy().into_owned());
            anc = anc.parent().ok_or_else(|| UNUSABLE.to_string())?;
        };
        let rel = base
            .strip_prefix(&self.root)
            .map_err(|_| OUTSIDE.to_string())?;
        let mut segs: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        segs.extend(tail.into_iter().rev());
        Ok(segs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const OUT: &str = "That path is outside the project folder.";
    const BAD: &str = "That path cannot be used.";

    fn project() -> (tempfile::TempDir, Project) {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/a.ts"), "x").unwrap();
        let p = Project::open(d.path().to_str().unwrap()).unwrap();
        (d, p)
    }

    #[test]
    fn accepted_forms_resolve_to_the_same_file() {
        let (d, p) = project();
        let abs = d.path().join("src").join("a.ts");
        for given in [
            "src/a.ts",
            "./src/a.ts",
            "src\\a.ts",
            "src//a.ts",
            abs.to_str().unwrap(),
        ] {
            let r = p.existing(given).unwrap_or_else(|e| panic!("{given}: {e}"));
            assert_eq!(r.shown, "src/a.ts", "{given}");
            assert_eq!(r.real, fs::canonicalize(&abs).unwrap(), "{given}");
        }
        let dir = p.existing("src/").unwrap();
        assert_eq!(dir.shown, "src");
        assert_eq!(p.existing(".").unwrap().shown, "");
    }

    #[test]
    fn paths_that_leave_the_project_are_outside() {
        let (d, p) = project();
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(elsewhere.path().join("f"), "x").unwrap();
        let abs_out = elsewhere.path().join("f");
        let parent = d.path().parent().unwrap().to_str().unwrap().to_string();
        for given in [
            "../x",
            "a/../../x",
            "src/../../x",
            "..\\x",
            abs_out.to_str().unwrap(),
            parent.as_str(),
        ] {
            assert_eq!(p.existing(given).unwrap_err(), OUT, "read {given}");
            assert_eq!(p.for_write(given).unwrap_err(), OUT, "write {given}");
        }
    }

    #[test]
    fn a_tilde_is_an_ordinary_name() {
        let (d, p) = project();
        assert!(p.existing("~/.ssh/id_rsa").is_err());
        let w = p.for_write("~/.ssh/id_rsa").unwrap();
        assert_eq!(w.shown, "~/.ssh/id_rsa");
        assert!(w.real.starts_with(fs::canonicalize(d.path()).unwrap()));
    }

    #[test]
    fn unusable_forms_are_refused() {
        let (_d, p) = project();
        for given in [
            "",
            "   ",
            "a\0b",
            "C:\\x",
            "C:x",
            "\\\\server\\share\\x",
            "//server/share/x",
            "a.txt:stream",
            "src/a.ts:",
            "a /b",
            "a./b",
            "con",
            "src/NUL.txt",
            "a\nb",
        ] {
            assert_eq!(p.existing(given).unwrap_err(), BAD, "read {given:?}");
            assert_eq!(p.for_write(given).unwrap_err(), BAD, "write {given:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_pointing_outside_is_refused_for_reading_and_writing() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("s.txt"), "secret").unwrap();
        let (d, p) = project();
        symlink(outside.path().join("s.txt"), d.path().join("l.txt")).unwrap();
        symlink(outside.path(), d.path().join("ld")).unwrap();
        assert_eq!(p.existing("l.txt").unwrap_err(), OUT);
        assert_eq!(p.existing("ld/s.txt").unwrap_err(), OUT);
        assert!(p.for_write("l.txt").is_err());
        assert!(p.for_write("ld/s.txt").is_err());
        assert!(p.for_write("ld/new.txt").is_err());
        assert!(!outside.path().join("new.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_file_inside_is_readable_but_not_writable() {
        use std::os::unix::fs::symlink;
        let (d, p) = project();
        symlink(d.path().join("src/a.ts"), d.path().join("l.ts")).unwrap();
        symlink(d.path().join("src"), d.path().join("lsrc")).unwrap();
        assert_eq!(p.existing("l.ts").unwrap().shown, "src/a.ts");
        assert!(p.for_write("l.ts").is_err());
        assert!(p.for_write("lsrc/a.ts").is_err());
        assert!(p.for_write("lsrc/b.ts").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_not_written_through() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        let (d, p) = project();
        symlink(outside.path().join("new.txt"), d.path().join("dangling")).unwrap();
        assert!(p.for_write("dangling").is_err());
        assert!(!outside.path().join("new.txt").exists());
    }

    #[test]
    fn for_write_accepts_new_files_and_new_folders() {
        let (d, p) = project();
        let real_root = fs::canonicalize(d.path()).unwrap();
        let a = p.for_write("src/new.ts").unwrap();
        assert_eq!(a.shown, "src/new.ts");
        assert_eq!(a.real, real_root.join("src").join("new.ts"));
        let b = p.for_write("deep\\er/still/f.txt").unwrap();
        assert_eq!(b.shown, "deep/er/still/f.txt");
        assert_eq!(b.real, real_root.join("deep/er/still/f.txt"));
        assert!(!real_root.join("deep").exists(), "nothing is created here");
        assert_eq!(p.for_write("src/a.ts").unwrap().shown, "src/a.ts");
        let abs = d.path().join("src").join("abs.ts");
        assert_eq!(
            p.for_write(abs.to_str().unwrap()).unwrap().shown,
            "src/abs.ts"
        );
    }

    #[test]
    fn for_write_refuses_directories_and_files_used_as_folders() {
        let (_d, p) = project();
        assert_eq!(p.for_write("src").unwrap_err(), BAD);
        assert_eq!(p.for_write("src/").unwrap_err(), BAD);
        assert_eq!(p.for_write("newdir/").unwrap_err(), BAD);
        assert_eq!(p.for_write("src/a.ts/b.ts").unwrap_err(), BAD);
        assert_eq!(p.for_write(".").unwrap_err(), BAD);
    }

    #[test]
    fn a_missing_file_cannot_be_read() {
        let (_d, p) = project();
        assert_eq!(p.existing("src/nope.ts").unwrap_err(), BAD);
    }

    #[test]
    fn a_deleted_root_fails_to_open_with_a_plain_message() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("gone");
        fs::create_dir(&path).unwrap();
        let s = path.to_str().unwrap().to_string();
        fs::remove_dir(&path).unwrap();
        let e = Project::open(&s).unwrap_err();
        assert!(!e.is_empty() && !e.contains(&s), "{e}");
        let file = d.path().join("f");
        fs::write(&file, "x").unwrap();
        assert!(Project::open(file.to_str().unwrap()).is_err());
    }

    #[test]
    fn a_root_deleted_after_opening_refuses_everything() {
        let (d, p) = project();
        let keep = d.path().to_path_buf();
        fs::remove_dir_all(&keep).unwrap();
        assert!(p.existing("src/a.ts").is_err());
        assert!(p.for_write("src/b.ts").is_err());
        fs::create_dir(&keep).unwrap(); // let TempDir clean up
    }

    /// Whether this file system treats differently-cased names as one.
    fn case_insensitive(dir: &std::path::Path) -> bool {
        fs::write(dir.join("CaseProbe"), "x").unwrap();
        let same = dir.join("caseprobe").exists();
        fs::remove_file(dir.join("CaseProbe")).unwrap();
        same
    }

    #[test]
    fn a_differently_cased_spelling_resolves_to_the_same_file() {
        let (d, p) = project();
        if !case_insensitive(d.path()) {
            eprintln!("skipped: this file system is case-sensitive");
            return;
        }
        let r = p.existing("SRC/A.ts").unwrap();
        assert_eq!(r.shown, "src/a.ts");
        assert_eq!(r.real, p.existing("src/a.ts").unwrap().real);
        let w = p.for_write("SRC/A.TS").unwrap();
        assert_eq!(w.shown, "src/a.ts");
        let w2 = p.for_write("SRC/New.ts").unwrap();
        assert_eq!(w2.shown, "src/New.ts");
        // An absolute path with the root spelled in another case.
        let abs = d.path().to_str().unwrap().to_uppercase() + "/src/a.ts";
        if std::path::Path::new(&abs).exists() {
            assert_eq!(p.existing(&abs).unwrap().shown, "src/a.ts");
        }
        // Case tricks do not reach outside.
        assert_eq!(p.existing("SRC/../../x").unwrap_err(), OUT);
    }

    #[test]
    fn a_root_opened_with_another_case_still_contains_its_files() {
        let (d, _p) = project();
        if !case_insensitive(d.path()) {
            eprintln!("skipped: this file system is case-sensitive");
            return;
        }
        let upper = d.path().to_str().unwrap().to_uppercase();
        if !std::path::Path::new(&upper).is_dir() {
            eprintln!("skipped: upper-cased temp path not resolvable");
            return;
        }
        let p = Project::open(&upper).unwrap();
        assert_eq!(p.existing("src/a.ts").unwrap().shown, "src/a.ts");
        assert_eq!(p.for_write("src/n.ts").unwrap().shown, "src/n.ts");
    }

    #[test]
    fn shown_is_relative_and_forward_slash() {
        let (_d, p) = project();
        for r in [
            p.existing("src\\a.ts").unwrap(),
            p.for_write("x\\y\\z.txt").unwrap(),
        ] {
            assert!(
                !r.shown.contains('\\') && !r.shown.starts_with('/'),
                "{}",
                r.shown
            );
        }
    }
}
