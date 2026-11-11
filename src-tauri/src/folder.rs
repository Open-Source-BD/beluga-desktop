//! Reading the files of a chat's folder. Read-only: nothing here writes,
//! deletes or executes. Every path that reaches the disk is checked against
//! the folder first (see `resolve`).

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Files walked in one listing before it is cut.
pub const MAX_WALKED_FILES: usize = 5_000;
/// Lines in a folder's listing attachment before it is cut.
pub const MAX_LISTING_LINES: usize = 500;
pub const OUTSIDE: &str = "That file is outside the folder.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    /// Relative to the folder, with forward slashes.
    pub path: String,
    pub is_dir: bool,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileList {
    pub entries: Vec<FileEntry>,
    /// Entries matching the query among those walked.
    pub total: usize,
    /// The walk stopped at the file cap.
    pub cut: bool,
}

/// Directory names never listed or entered.
const SKIPPED_DIRS: [&str; 5] = ["node_modules", "target", "dist", "build", ".git"];

/// The entries under `base` (not `base` itself), in a stable order, with
/// paths relative to `base`. Stops once more than `max_files` files are met
/// and reports that as the second value. Never follows links.
fn walk(base: &Path, max_files: usize) -> (Vec<FileEntry>, bool) {
    let walker = ignore::WalkBuilder::new(base)
        .hidden(true)
        .git_ignore(true)
        .require_git(false)
        .git_global(false)
        .git_exclude(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| {
            !(e.depth() > 0
                && e.file_type().is_some_and(|t| t.is_dir())
                && SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        })
        .build();
    let mut entries = Vec::new();
    let mut files = 0;
    for item in walker {
        // An entry that cannot be read is skipped.
        let Ok(e) = item else { continue };
        if e.depth() == 0 {
            continue;
        }
        let Some(kind) = e.file_type() else { continue };
        let Ok(rel) = e.path().strip_prefix(base) else {
            continue;
        };
        let path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if kind.is_dir() {
            entries.push(FileEntry {
                path,
                is_dir: true,
                bytes: 0,
            });
        } else if kind.is_file() {
            let Ok(meta) = e.metadata() else { continue };
            if files == max_files {
                return (entries, true);
            }
            files += 1;
            entries.push(FileEntry {
                path,
                is_dir: false,
                bytes: meta.len(),
            });
        }
        // Symbolic links and other kinds are not listed.
    }
    (entries, false)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Entries of `root` whose path contains `query` (case-insensitive), best
/// first: file-name matches, then shorter paths. An empty query lists
/// everything shallowest first, then alphabetically.
pub fn list_files(root: &Path, query: &str, limit: usize) -> FileList {
    let (entries, cut) = walk(root, MAX_WALKED_FILES);
    let q = query.to_lowercase();
    let mut matched: Vec<(String, FileEntry)> = entries
        .into_iter()
        .map(|e| (e.path.to_lowercase(), e))
        .filter(|(lower, _)| lower.contains(&q))
        .collect();
    if q.is_empty() {
        matched.sort_by_cached_key(|(_, e)| (e.path.matches('/').count(), e.path.clone()));
    } else {
        matched.sort_by_cached_key(|(lower, e)| {
            (
                !file_name(lower).contains(&q),
                lower.chars().count(),
                e.path.clone(),
            )
        });
    }
    let total = matched.len();
    FileList {
        entries: matched.into_iter().take(limit).map(|(_, e)| e).collect(),
        total,
        cut,
    }
}

/// The non-empty path segments of `rel`, splitting on `/` and `\\`, without
/// `.` segments; `None` when `rel` is absolute or could leave the folder: a
/// leading separator, a NUL, a `..` segment, or a segment with `:` (a drive
/// or stream name).
pub fn segments(rel: &str) -> Option<Vec<&str>> {
    if rel.contains('\0') || rel.starts_with(['/', '\\']) || Path::new(rel).is_absolute() {
        return None;
    }
    let mut out = Vec::new();
    for seg in rel.split(['/', '\\']) {
        if seg == ".." || seg.contains(':') {
            return None;
        }
        if !seg.is_empty() && seg != "." {
            out.push(seg);
        }
    }
    Some(out)
}

/// The real path of `rel` inside `root`: validated segments joined onto
/// `root`, both canonicalized, and the result required to lie inside the
/// real `root`. A link that points outside fails here. Every failure is
/// `OUTSIDE`.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let segs = segments(rel).ok_or_else(|| OUTSIDE.to_string())?;
    let mut joined = root.to_path_buf();
    joined.extend(&segs);
    let (Ok(real_root), Ok(real)) = (std::fs::canonicalize(root), std::fs::canonicalize(&joined))
    else {
        return Err(OUTSIDE.to_string());
    };
    if real.starts_with(&real_root) {
        Ok(real)
    } else {
        Err(OUTSIDE.to_string())
    }
}

pub fn read_file(root: &Path, rel: &str) -> Result<Vec<u8>, String> {
    let path = resolve(root, rel)?;
    std::fs::read(path).map_err(|_| OUTSIDE.to_string())
}

/// One `path<TAB>bytes` line per file under the directory `rel` (paths
/// relative to `root`), up to 500, then a line saying the list was cut.
pub fn folder_listing(root: &Path, rel: &str) -> Result<String, String> {
    let dir = resolve(root, rel)?;
    if !dir.is_dir() {
        return Err(OUTSIDE.to_string());
    }
    let prefix = segments(rel).unwrap_or_default().join("/");
    let (mut entries, _) = walk(&dir, MAX_LISTING_LINES + 1);
    entries.retain(|e| !e.is_dir);
    let cut = entries.len() > MAX_LISTING_LINES;
    entries.truncate(MAX_LISTING_LINES);
    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| {
            let path = if prefix.is_empty() {
                e.path.clone()
            } else {
                format!("{prefix}/{}", e.path)
            };
            format!("{path}\t{}", e.bytes)
        })
        .collect();
    if cut {
        lines.push(format!("List cut at {MAX_LISTING_LINES} entries."));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn put(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn paths(l: &FileList) -> Vec<&str> {
        l.entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn lists_files_relative_with_forward_slashes() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "a.txt", "hello");
        put(d.path(), "src/deep/b.rs", "fn main() {}");
        let l = list_files(d.path(), "", 50);
        assert_eq!(paths(&l), ["a.txt", "src", "src/deep", "src/deep/b.rs"]);
        assert!(!l.cut);
        assert_eq!(l.total, 4);
        let a = &l.entries[0];
        assert_eq!((a.is_dir, a.bytes), (false, 5));
        let dir = &l.entries[1];
        assert_eq!((dir.is_dir, dir.bytes), (true, 0));
    }

    #[test]
    fn respects_gitignore_and_skips_node_modules_target_dist_build_dot_git_and_hidden() {
        let d = tempfile::tempdir().unwrap();
        // No `.git` directory: the ignore file still applies.
        put(d.path(), ".gitignore", "secret.txt\nlogs/\n");
        put(d.path(), "keep.txt", "x");
        put(d.path(), "secret.txt", "x");
        put(d.path(), "logs/a.log", "x");
        put(d.path(), ".hidden", "x");
        put(d.path(), ".git/config", "x");
        for dir in ["node_modules", "target", "dist", "build"] {
            put(d.path(), &format!("{dir}/f.txt"), "x");
            put(d.path(), &format!("sub/{dir}/f.txt"), "x");
        }
        let l = list_files(d.path(), "", 50);
        assert_eq!(paths(&l), ["keep.txt", "sub"]);
    }

    #[test]
    fn query_ranks_file_name_matches_first_then_shorter_paths() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "notes/readme.md", "x");
        put(d.path(), "readme/other.md", "x");
        put(d.path(), "README.txt", "x");
        put(d.path(), "docs/deep/dir/readme-long-name.md", "x");
        let l = list_files(d.path(), "ReadMe", 50);
        assert_eq!(
            paths(&l),
            [
                "readme",
                "README.txt",
                "notes/readme.md",
                "docs/deep/dir/readme-long-name.md",
                "readme/other.md",
            ]
        );
        assert_eq!(l.total, 5);
        let limited = list_files(d.path(), "readme", 2);
        assert_eq!(limited.entries.len(), 2);
        assert_eq!(limited.total, 5);
    }

    #[test]
    fn stops_at_5000_files_with_cut_true() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5_010 {
            fs::write(d.path().join(format!("f{i:05}.txt")), "x").unwrap();
        }
        let l = list_files(d.path(), "", 50);
        assert!(l.cut);
        assert_eq!(l.total, 5_000);
        assert_eq!(l.entries.len(), 50);

        let exact = tempfile::tempdir().unwrap();
        for i in 0..5_000 {
            fs::write(exact.path().join(format!("f{i:05}.txt")), "x").unwrap();
        }
        let l = list_files(exact.path(), "", 50);
        assert!(!l.cut);
        assert_eq!(l.total, 5_000);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_outside_is_not_listed_and_cannot_be_read() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        put(outside.path(), "secret.txt", "top secret");
        put(outside.path(), "dir/inner.txt", "inner");
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "ok.txt", "fine");
        symlink(outside.path().join("secret.txt"), d.path().join("link.txt")).unwrap();
        symlink(outside.path().join("dir"), d.path().join("linkdir")).unwrap();
        let l = list_files(d.path(), "", 50);
        assert_eq!(paths(&l), ["ok.txt"]);
        assert_eq!(read_file(d.path(), "link.txt"), Err(OUTSIDE.to_string()));
        assert_eq!(
            read_file(d.path(), "linkdir/inner.txt"),
            Err(OUTSIDE.to_string())
        );
        assert_eq!(
            folder_listing(d.path(), "linkdir"),
            Err(OUTSIDE.to_string())
        );
    }

    #[test]
    fn read_file_rejects_dot_dot_and_absolute_paths() {
        let outside = tempfile::tempdir().unwrap();
        put(outside.path(), "secret.txt", "top secret");
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "a/ok.txt", "fine");
        let abs = outside.path().join("secret.txt");
        let bad = [
            "../x".to_string(),
            "a/../../x".to_string(),
            "a/..".to_string(),
            "a\\..\\x".to_string(),
            abs.to_str().unwrap().to_string(),
            "/etc/passwd".to_string(),
            "\\windows\\win.ini".to_string(),
            "C:\\x".to_string(),
            "C:/x".to_string(),
            "a:b".to_string(),
            "a/b:c".to_string(),
            "a\0b".to_string(),
        ];
        for rel in bad {
            assert_eq!(
                read_file(d.path(), &rel),
                Err(OUTSIDE.to_string()),
                "{rel:?}"
            );
            assert_eq!(resolve(d.path(), &rel), Err(OUTSIDE.to_string()), "{rel:?}");
        }
        // A missing file is refused the same way.
        assert_eq!(read_file(d.path(), "a/nope.txt"), Err(OUTSIDE.to_string()));
    }

    #[test]
    fn read_file_inside_the_root_returns_exact_bytes() {
        let d = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..=255).collect();
        fs::create_dir_all(d.path().join("a")).unwrap();
        fs::write(d.path().join("a/bin.dat"), &bytes).unwrap();
        assert_eq!(read_file(d.path(), "a/bin.dat"), Ok(bytes.clone()));
        assert_eq!(read_file(d.path(), "./a//bin.dat"), Ok(bytes.clone()));
        assert_eq!(read_file(d.path(), "a\\bin.dat"), Ok(bytes));
    }

    #[test]
    fn folder_listing_is_capped_at_500_entries_and_says_so() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("sub")).unwrap();
        for i in 0..510 {
            fs::write(d.path().join(format!("sub/f{i:04}.txt")), "ab").unwrap();
        }
        put(d.path(), "other.txt", "x");
        let text = folder_listing(d.path(), "sub").unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 501);
        assert_eq!(lines[0], "sub/f0000.txt\t2");
        assert_eq!(lines[500], "List cut at 500 entries.");

        let small = folder_listing(d.path(), "").unwrap();
        assert_eq!(small.lines().count(), 501);
        let tiny = tempfile::tempdir().unwrap();
        put(tiny.path(), "a.txt", "abc");
        put(tiny.path(), "d/b.txt", "z");
        assert_eq!(
            folder_listing(tiny.path(), "").unwrap(),
            "a.txt\t3\nd/b.txt\t1"
        );
        assert!(folder_listing(tiny.path(), "a.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_entry_is_skipped_without_error() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "ok.txt", "x");
        put(d.path(), "locked/inner.txt", "x");
        let locked = d.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let l = list_files(d.path(), "", 50);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(paths(&l).contains(&"ok.txt"));
        assert!(!paths(&l).contains(&"locked/inner.txt"));
    }

    #[test]
    fn string_level_rejection_runs_everywhere() {
        for rel in ["..", "a/../b", "/abs", "C:\\x", "a:b", "x\0y"] {
            assert!(segments(rel).is_none(), "{rel:?}");
        }
        assert_eq!(segments("a/./b\\c//"), Some(vec!["a", "b", "c"]));
        assert_eq!(segments(""), Some(vec![]));
    }

    #[cfg(windows)]
    #[test]
    fn a_verbatim_prefixed_canonical_root_accepts_a_normal_relative_file() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "dir/a.txt", "hi");
        let root = fs::canonicalize(d.path()).unwrap();
        assert!(root.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(read_file(&root, "dir/a.txt"), Ok(b"hi".to_vec()));
        assert_eq!(read_file(&root, "dir\\a.txt"), Ok(b"hi".to_vec()));
    }
}
