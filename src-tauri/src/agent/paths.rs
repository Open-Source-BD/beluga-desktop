//! Turning a path the model supplied into a real path inside the project.
//!
//! The model (possibly misled by text inside a file) chooses these strings,
//! so nothing here trusts them. A path is first normalised to plain name
//! segments, then checked on the real, resolved path by `Path::components`
//! (`Path::starts_with`), never by string prefix. `~` is never expanded: it is
//! an ordinary directory name. Every refusal is one of three fixed messages;
//! the third (`PRIVATE`) is for the app's own data folder.
//!
//! Time of check versus time of use: these functions return a path that was
//! valid at the moment they ran. A caller that writes must call `for_write`
//! again immediately before touching the disk, write only to the returned
//! `real`, and open the file without following a final link (for example
//! `O_NOFOLLOW`, or create-new for a new file). Nothing here creates or locks
//! anything.
//!
//! More for callers:
//! - A reader must open the file immediately after `existing` and check, on
//!   the open handle, that it is a regular file (a path can be swapped for a
//!   pipe or device between the check and the open).
//! - A writer must call `for_write` again right before writing, and create
//!   missing folders one at a time without following a link.
//! - A hard link to a file outside the root cannot be detected by path; it
//!   looks like any other file inside the project and is accepted.

use std::path::{Component, Path, PathBuf};

pub const OUTSIDE: &str = "That path is outside the project folder.";
pub const UNUSABLE: &str = "That path cannot be used.";
pub const PRIVATE: &str = "That path is in the app's own data folder, so it cannot be used.";

/// The canonical folder of a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    /// The app's own data folder (its database, key file and installed
    /// skills), real path. Nothing in it is read, listed, searched or
    /// written, even when the project folder contains it.
    private: Option<PathBuf>,
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

/// Format (Cf) and line/paragraph separator characters: invisible or
/// direction-changing, so two names could look identical on an approval
/// card. A fixed list; no crate.
pub(super) fn invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2
        | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// The purely lexical check that runs before any file-system call on a
/// model-given path. Canonicalizing a network name would open a connection to
/// a server the model chose (and could leak the user's credentials), so:
/// - a path whose first two characters are both a slash or backslash, in any
///   mix (`//`, `\\`, `/\`, `\/`), is refused on every system (Windows reads
///   `/` as `\` in a prefix), as is any path starting with a single `\`;
/// - a path whose parsed prefix is anything but a plain drive (UNC, verbatim,
///   device) is refused.
fn refuse_network_forms(given: &str) -> Result<(), String> {
    let mut chars = given.chars();
    let sep = |c: Option<char>| matches!(c, Some('/' | '\\'));
    let first = chars.next();
    if first == Some('\\') || (sep(first) && sep(chars.next())) {
        return Err(UNUSABLE.to_string());
    }
    let prefix = match Path::new(given).components().next() {
        Some(Component::Prefix(p)) => Some(p.kind()),
        _ => None,
    };
    if prefix.is_some_and(|k| !matches!(k, std::path::Prefix::Disk(_))) {
        return Err(UNUSABLE.to_string());
    }
    Ok(())
}

/// A name that is safe on every system: no `:` (drive or stream), no control
/// characters, no trailing space or dot (Windows drops them, so `.. ` would
/// become `..`), and not a device name.
fn plain_name(seg: &str) -> bool {
    if seg.is_empty()
        || seg.contains(':')
        || seg.chars().any(|c| c.is_control() || invisible(c))
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

/// `path` is `folder` or inside it, compared by component; names ignore
/// case on macOS and Windows.
pub(crate) fn is_within(path: &Path, folder: &Path) -> bool {
    let same = |a: &std::ffi::OsStr, b: &std::ffi::OsStr| {
        if cfg!(any(target_os = "macos", windows)) {
            a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
        } else {
            a == b
        }
    };
    let mut inner = path.components();
    folder.components().all(|f| {
        inner
            .next()
            .is_some_and(|c| same(c.as_os_str(), f.as_os_str()))
    })
}

impl Project {
    /// The same project, refusing every path in the app's data folder
    /// `data_dir`, by its real path. Should the folder not exist yet, its
    /// nearest existing ancestor is resolved and the rest kept as given.
    pub fn keeping_out(self, data_dir: &Path) -> Project {
        let mut missing = Vec::new();
        let mut at = data_dir;
        let private = loop {
            if let Ok(real) = std::fs::canonicalize(at) {
                break missing.iter().rev().fold(real, |p, name| p.join(name));
            }
            match (at.parent(), at.file_name()) {
                (Some(parent), Some(name)) => {
                    missing.push(name.to_os_string());
                    at = parent;
                }
                _ => break data_dir.to_path_buf(),
            }
        };
        Project {
            private: Some(private),
            ..self
        }
    }

    /// `real` (a real path) is the app's data folder or inside it.
    pub fn is_private(&self, real: &Path) -> bool {
        self.private.as_deref().is_some_and(|d| is_within(real, d))
    }

    /// The app's data folder, for walks to skip.
    pub fn private(&self) -> Option<&Path> {
        self.private.as_deref()
    }

    /// Opens a folder: it must exist and be a directory.
    pub fn open(folder: &str) -> Result<Project, String> {
        const GONE: &str = "The project folder is missing or is not a folder.";
        let root = std::fs::canonicalize(folder).map_err(|_| GONE.to_string())?;
        if root.is_dir() {
            Ok(Project {
                root,
                private: None,
            })
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
        let real = match std::fs::canonicalize(&joined) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && self.no_link_on(&segs) => {
                return Err(format!("There is no file or folder at {}.", segs.join("/")));
            }
            Err(_) => return Err(UNUSABLE.to_string()),
        };
        if !real.starts_with(&self.root) {
            return Err(OUTSIDE.to_string());
        }
        if self.is_private(&real) {
            return Err(PRIVATE.to_string());
        }
        let shown = shown_of(&self.root, &real);
        Ok(InProject { real, shown })
    }

    /// True when no existing component of `segs` below the root is a link,
    /// so a missing path is plainly missing and not a dangling link.
    fn no_link_on(&self, segs: &[String]) -> bool {
        let mut cur = self.root.clone();
        for seg in segs {
            cur.push(seg);
            match std::fs::symlink_metadata(&cur) {
                Ok(m) if m.file_type().is_symlink() => return false,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        true
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
                    if self.is_private(&real) {
                        return Err(PRIVATE.to_string());
                    }
                    let shown = shown_of(&self.root, &real);
                    return Ok(InProject { real, shown });
                }
                Err(_) => return Err(UNUSABLE.to_string()),
            };
            let real = std::fs::canonicalize(&next).map_err(|_| UNUSABLE.to_string())?;
            if !real.starts_with(&self.root) {
                return Err(OUTSIDE.to_string());
            }
            if self.is_private(&real) {
                return Err(PRIVATE.to_string());
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
        refuse_network_forms(given)?;
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

    /// An absolute `given`, as segments below the root, taken lexically: the
    /// shortest leading part of `given` that resolves to the root is the
    /// root, and the names after it are kept as given, so the caller walks
    /// them link by link exactly like a relative path. Nothing below the
    /// root is resolved here. A path not under the root is outside.
    fn absolute_segments(&self, given: &str, path: &Path) -> Result<Vec<String>, String> {
        if cfg!(not(windows)) && given.contains('\\') {
            return Err(UNUSABLE.to_string());
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(OUTSIDE.to_string());
        }
        let comps: Vec<Component> = path.components().collect();
        let mut prefix = PathBuf::new();
        for (i, c) in comps.iter().enumerate() {
            prefix.push(c.as_os_str());
            if matches!(c, Component::CurDir | Component::Prefix(_)) {
                continue;
            }
            if std::fs::canonicalize(&prefix).is_ok_and(|r| r == self.root) {
                return Ok(comps[i + 1..]
                    .iter()
                    .filter(|c| !matches!(c, Component::CurDir))
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect());
            }
        }
        Err(OUTSIDE.to_string())
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

    #[test]
    fn two_leading_separators_in_any_mix_are_refused_before_the_disk() {
        let (_d, p) = project();
        for given in [
            "//server/share/x",
            "\\\\server\\share\\x",
            "/\\server\\share\\x",
            "\\/server/share/x",
            "/\\?\\C:\\x",
            "\\/.\\pipe\\x",
            "//",
            "/\\",
        ] {
            // The lexical check runs first and alone: no file-system call.
            assert_eq!(refuse_network_forms(given).unwrap_err(), BAD, "{given}");
            assert_eq!(p.existing(given).unwrap_err(), BAD, "read {given}");
            assert_eq!(p.for_write(given).unwrap_err(), BAD, "write {given}");
        }
        assert!(refuse_network_forms("/home/x").is_ok());
        assert!(refuse_network_forms("src/a.ts").is_ok());
    }

    #[test]
    fn a_drive_path_is_refused_everywhere() {
        let (_d, p) = project();
        // An absolute path on Windows (outside), a odd relative name elsewhere.
        assert!(p.existing("C:\\x").is_err());
        assert!(p.for_write("C:\\x").is_err());
        #[cfg(unix)]
        {
            assert_eq!(p.existing("C:\\x").unwrap_err(), BAD);
            assert_eq!(p.for_write("C:\\x").unwrap_err(), BAD);
        }
    }

    #[test]
    fn invisible_and_direction_changing_characters_are_refused() {
        let (_d, p) = project();
        for c in [
            '\u{200b}', '\u{202e}', '\u{feff}', '\u{00ad}', '\u{2060}', '\u{2066}', '\u{200f}',
        ] {
            for given in [
                format!("src/a.ts{c}"),
                format!("sr{c}c/a.ts"),
                format!("{c}x"),
            ] {
                assert_eq!(p.existing(&given).unwrap_err(), BAD, "read {given:?}");
                assert_eq!(p.for_write(&given).unwrap_err(), BAD, "write {given:?}");
            }
        }
    }

    #[test]
    fn a_missing_path_inside_names_itself_but_outside_reveals_nothing() {
        let (d, p) = project();
        assert_eq!(
            p.existing("./src\\nope.ts").unwrap_err(),
            "There is no file or folder at src/nope.ts."
        );
        assert_eq!(
            p.existing("nodir/deeper/x").unwrap_err(),
            "There is no file or folder at nodir/deeper/x."
        );
        let abs = d.path().join("src").join("nope.ts");
        assert_eq!(
            p.existing(abs.to_str().unwrap()).unwrap_err(),
            "There is no file or folder at src/nope.ts."
        );
        assert_eq!(p.existing("../nope").unwrap_err(), OUT);
        assert_eq!(p.existing("src/../../nope").unwrap_err(), OUT);
        let elsewhere = tempfile::tempdir().unwrap();
        let out = elsewhere.path().join("missing");
        assert_eq!(p.existing(out.to_str().unwrap()).unwrap_err(), OUT);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_keeps_the_cannot_be_used_message_for_reading() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        let (d, p) = project();
        symlink(outside.path().join("gone"), d.path().join("dangling")).unwrap();
        symlink(d.path().join("src/none"), d.path().join("dangling_in")).unwrap();
        assert_eq!(p.existing("dangling").unwrap_err(), BAD);
        assert_eq!(p.existing("dangling/x").unwrap_err(), BAD);
        assert_eq!(p.existing("dangling_in").unwrap_err(), BAD);
    }

    #[cfg(unix)]
    fn links(d: &std::path::Path) {
        use std::os::unix::fs::symlink;
        fs::write(d.join(".env"), "S=1").unwrap();
        symlink(d.join("src/a.ts"), d.join("l.ts")).unwrap();
        symlink(d.join("src"), d.join("lsrc")).unwrap();
        symlink(d.join("l.ts"), d.join("chain")).unwrap();
        symlink(d.join(".env"), d.join("innocent")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn writing_through_a_link_is_refused_in_absolute_form_too() {
        let (d, p) = project();
        links(d.path());
        let root = d.path().to_str().unwrap();
        for rel in ["l.ts", "lsrc/a.ts", "lsrc/new.ts", "chain", "innocent"] {
            assert!(p.for_write(rel).is_err(), "relative {rel}");
            let abs = format!("{root}/{rel}");
            assert!(p.for_write(&abs).is_err(), "absolute {abs}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_inside_still_read_in_absolute_form() {
        let (d, p) = project();
        links(d.path());
        let root = d.path().to_str().unwrap();
        for rel in ["l.ts", "lsrc/a.ts", "chain"] {
            let a = p.existing(&format!("{root}/{rel}")).unwrap();
            assert_eq!(a.shown, p.existing(rel).unwrap().shown, "{rel}");
        }
    }

    #[test]
    fn an_absolute_path_to_a_plain_file_works_for_read_and_write() {
        let (d, p) = project();
        let a = d.path().join("src/a.ts");
        assert_eq!(p.existing(a.to_str().unwrap()).unwrap().shown, "src/a.ts");
        assert_eq!(p.for_write(a.to_str().unwrap()).unwrap().shown, "src/a.ts");
        let n = d.path().join("fresh/n.ts");
        assert_eq!(
            p.for_write(n.to_str().unwrap()).unwrap().shown,
            "fresh/n.ts"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_root_reached_through_a_symlinked_parent_works() {
        use std::os::unix::fs::symlink;
        let (d, _p) = project();
        let outer = tempfile::tempdir().unwrap();
        let via = outer.path().join("via");
        symlink(d.path(), &via).unwrap();
        let p = Project::open(via.to_str().unwrap()).unwrap();
        let a = via.join("src/a.ts");
        assert_eq!(p.existing(a.to_str().unwrap()).unwrap().shown, "src/a.ts");
        let n = via.join("src/n.ts");
        assert_eq!(p.for_write(n.to_str().unwrap()).unwrap().shown, "src/n.ts");
        // The real spelling works as well.
        let r = d.path().join("src/a.ts");
        assert_eq!(p.existing(r.to_str().unwrap()).unwrap().shown, "src/a.ts");
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
        assert_eq!(
            p.existing("src/nope.ts").unwrap_err(),
            "There is no file or folder at src/nope.ts."
        );
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

    /// A project folder holding the app's data folder `app` with its key
    /// file and database.
    fn project_with_data() -> (tempfile::TempDir, Project) {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("app/skills")).unwrap();
        fs::write(d.path().join("app/key"), "sk-or-v1-FILEKEY123").unwrap();
        fs::write(d.path().join("app/chats.db"), "db").unwrap();
        fs::write(d.path().join("mine.txt"), "x").unwrap();
        let p = Project::open(d.path().to_str().unwrap())
            .unwrap()
            .keeping_out(&d.path().join("app"));
        (d, p)
    }

    #[test]
    fn the_apps_data_folder_is_refused_for_reading_and_writing() {
        let (d, p) = project_with_data();
        let abs = d.path().join("app").join("key");
        for given in [
            "app",
            "app/",
            "app/key",
            "./app/chats.db",
            "app\\key",
            "app/skills",
            abs.to_str().unwrap(),
        ] {
            assert_eq!(p.existing(given).unwrap_err(), PRIVATE, "read {given}");
        }
        for given in [
            "app/key",
            "app/chats.db",
            "app/new.txt",
            "app/new/deeper.txt",
        ] {
            assert_eq!(p.for_write(given).unwrap_err(), PRIVATE, "write {given}");
        }
        assert!(!d.path().join("app/new.txt").exists());
        // The rest of the project is unaffected.
        assert_eq!(p.existing("mine.txt").unwrap().shown, "mine.txt");
        assert_eq!(p.existing(".").unwrap().shown, "");
        assert_eq!(p.for_write("mine.txt").unwrap().shown, "mine.txt");
        assert_eq!(p.for_write("apps/x.txt").unwrap().shown, "apps/x.txt");
        assert!(!p.is_private(&p.root));
    }

    #[test]
    fn the_apps_data_folder_in_another_case_is_refused_where_names_ignore_case() {
        let (d, p) = project_with_data();
        if !case_insensitive(d.path()) {
            eprintln!("skipped: this file system is case-sensitive");
            return;
        }
        assert_eq!(p.existing("APP/KEY").unwrap_err(), PRIVATE);
        assert_eq!(p.for_write("App/Key").unwrap_err(), PRIVATE);
        assert_eq!(p.for_write("APP/fresh.txt").unwrap_err(), PRIVATE);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_into_the_apps_data_folder_is_refused() {
        use std::os::unix::fs::symlink;
        let (d, p) = project_with_data();
        symlink(d.path().join("app/key"), d.path().join("innocent")).unwrap();
        symlink(d.path().join("app"), d.path().join("door")).unwrap();
        assert_eq!(p.existing("innocent").unwrap_err(), PRIVATE);
        assert_eq!(p.existing("door/chats.db").unwrap_err(), PRIVATE);
        assert!(p.for_write("door/new.txt").is_err());
    }

    #[test]
    fn a_project_without_a_data_folder_is_unaffected() {
        let (_d, p) = project();
        let elsewhere = tempfile::tempdir().unwrap();
        let p = p.keeping_out(elsewhere.path());
        assert_eq!(p.existing("src/a.ts").unwrap().shown, "src/a.ts");
        assert_eq!(p.for_write("src/n.ts").unwrap().shown, "src/n.ts");
    }

    #[cfg(windows)]
    mod windows {
        use super::*;

        fn junction(link: &std::path::Path, target: &std::path::Path) {
            let st = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(st.status.success(), "mklink failed: {st:?}");
        }

        #[test]
        fn drive_and_unc_and_device_forms_are_refused() {
            let (_d, p) = project();
            let other = tempfile::tempdir().unwrap();
            let elsewhere = other.path().join("x");
            assert!(p.existing("C:\\x").is_err());
            assert!(p.for_write("C:\\x").is_err());
            assert!(p.for_write(elsewhere.to_str().unwrap()).is_err());
            for given in [
                "C:x",
                "\\\\server\\share\\x",
                "\\\\?\\C:\\x",
                "\\\\.\\pipe\\x",
                "a.txt:stream",
                "a.txt::$DATA",
            ] {
                assert_eq!(p.existing(given).unwrap_err(), BAD, "read {given}");
                assert_eq!(p.for_write(given).unwrap_err(), BAD, "write {given}");
            }
        }

        #[test]
        fn mixed_separator_network_forms_are_refused() {
            let (_d, p) = project();
            for given in [
                "/\\server\\share\\x",
                "\\/server\\share\\x",
                "/\\?\\C:\\x",
                "/\\.\\pipe\\x",
            ] {
                assert_eq!(refuse_network_forms(given).unwrap_err(), BAD, "{given}");
                assert_eq!(p.existing(given).unwrap_err(), BAD, "read {given}");
                assert_eq!(p.for_write(given).unwrap_err(), BAD, "write {given}");
            }
        }

        #[test]
        fn only_a_plain_drive_prefix_passes_the_lexical_check() {
            assert!(refuse_network_forms("C:\\x").is_ok());
            assert!(refuse_network_forms("c:/x").is_ok());
            for given in [
                "\\\\?\\C:\\x",
                "\\\\.\\pipe\\x",
                "\\\\s\\h\\x",
                "\\\\?\\UNC\\s\\h\\x",
            ] {
                assert_eq!(refuse_network_forms(given).unwrap_err(), BAD, "{given}");
            }
        }

        #[test]
        fn a_junction_is_not_written_through() {
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("s.txt"), "secret").unwrap();
            let (d, p) = project();
            junction(&d.path().join("jout"), outside.path());
            junction(&d.path().join("jin"), &d.path().join("src"));
            assert!(p.existing("jout/s.txt").is_err());
            assert!(p.for_write("jout/s.txt").is_err());
            assert!(p.for_write("jout/new.txt").is_err());
            assert!(p.for_write("jin/a.ts").is_err());
            assert!(p.for_write("jin/new.ts").is_err());
            let abs = d.path().join("jin").join("a.ts");
            assert!(p.for_write(abs.to_str().unwrap()).is_err());
            let abs_out = d.path().join("jout").join("s.txt");
            assert!(p.existing(abs_out.to_str().unwrap()).is_err());
            assert!(!outside.path().join("new.txt").exists());
        }

        #[test]
        fn a_different_drive_letter_case_works() {
            let (d, p) = project();
            let s = d
                .path()
                .join("src")
                .join("a.ts")
                .to_str()
                .unwrap()
                .to_string();
            let mut chars = s.chars();
            let first = chars.next().unwrap();
            let flipped = if first.is_ascii_uppercase() {
                first.to_ascii_lowercase()
            } else {
                first.to_ascii_uppercase()
            };
            let other = format!("{flipped}{}", chars.as_str());
            assert_eq!(p.existing(&other).unwrap().shown, "src/a.ts");
            assert_eq!(p.for_write(&other).unwrap().shown, "src/a.ts");
        }

        #[test]
        fn shown_has_no_backslash_or_verbatim_prefix() {
            let (d, p) = project();
            let abs = d.path().join("src").join("a.ts");
            for r in [
                p.existing(abs.to_str().unwrap()).unwrap(),
                p.existing("src\\a.ts").unwrap(),
                p.for_write("n\\m.txt").unwrap(),
            ] {
                assert!(
                    !r.shown.contains('\\') && !r.shown.contains("?"),
                    "{}",
                    r.shown
                );
            }
        }
    }
}
