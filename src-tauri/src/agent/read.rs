//! `read_file` and `list_dir`, plus the small argument helpers every read-only
//! tool shares.

use serde_json::Value;

use super::paths::{InProject, Project, invisible};
use super::secrets::is_secret;
use super::{ToolOutcome, ToolStatus};
use crate::folder::{MAX_READ_BYTES, read_bounded};
use crate::skills::library::Library;

/// Most lines one `read_file` call returns.
const MAX_LINES: usize = 400;
/// Most characters of file text one `read_file` call returns.
const MAX_CHARS: usize = 16_000;
/// Most entries one `list_dir` call returns.
const MAX_ENTRIES: usize = 500;
/// Longest model-given text shown in a summary line.
const MAX_SUMMARY_CHARS: usize = 80;
/// Longest project path shown in a summary line.
const MAX_PATH_SUMMARY_CHARS: usize = 200;
const SKILL_PREFIX: &str = "skill:";

pub(super) fn ok(text: String, summary: String) -> ToolOutcome {
    ToolOutcome {
        status: ToolStatus::Ok,
        text,
        summary,
        detail: None,
    }
}

pub(super) fn fail(text: String, summary: String) -> ToolOutcome {
    ToolOutcome {
        status: ToolStatus::Error,
        text,
        summary,
        detail: None,
    }
}

/// A required text argument; the message names it.
pub(super) fn required_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    match args.get(name) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(format!("The argument \"{name}\" must be text.")),
        None => Err(format!("The argument \"{name}\" is missing.")),
    }
}

/// An optional text argument; `null` counts as absent.
pub(super) fn optional_str<'a>(args: &'a Value, name: &str) -> Result<Option<&'a str>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("The argument \"{name}\" must be text.")),
    }
}

/// An optional whole number of at least zero; `null` counts as absent.
pub(super) fn optional_uint(args: &Value, name: &str) -> Result<Option<u64>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("The argument \"{name}\" must be a whole number.")),
    }
}

/// The optional `path` of a folder tool: absent, empty and "." all mean the
/// project folder.
pub(super) fn folder_arg(args: &Value) -> Result<&str, String> {
    Ok(match optional_str(args, "path")? {
        None | Some("") => ".",
        Some(p) => p,
    })
}

/// Text from a model argument made safe to show: control and format characters
/// become `?`.
pub(super) fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || invisible(c) {
                '?'
            } else {
                c
            }
        })
        .collect()
}

/// `clean`, cut to 80 characters with an ellipsis, for a summary line.
pub(super) fn summary_text(s: &str) -> String {
    let s = clean(s);
    if s.chars().count() > MAX_SUMMARY_CHARS {
        let head: String = s.chars().take(MAX_SUMMARY_CHARS).collect();
        format!("{head}…")
    } else {
        s
    }
}

/// A project path made safe to show in a summary line: cleaned and cut at
/// 200 characters. Every summary that holds a path goes through this.
pub(super) fn path_summary(shown: &str) -> String {
    let s = clean(shown);
    if s.chars().count() > MAX_PATH_SUMMARY_CHARS {
        let head: String = s.chars().take(MAX_PATH_SUMMARY_CHARS).collect();
        format!("{head}…")
    } else {
        s
    }
}

/// `n` plural-aware: "1 line", "2 lines".
fn lines_word(n: usize) -> String {
    format!("{n} line{}", if n == 1 { "" } else { "s" })
}

pub fn read_file(
    project: &Project,
    library: &Library,
    args: &Value,
    allow_secret: bool,
) -> ToolOutcome {
    let generic = "Read a file".to_string();
    let path = match required_str(args, "path") {
        Ok(p) => p,
        Err(e) => return fail(e, generic),
    };
    let (offset, limit) = match window(args) {
        Ok(w) => w,
        Err(e) => return fail(e, generic),
    };
    let (summary, loaded) = if let Some(rest) = path.strip_prefix(SKILL_PREFIX) {
        (
            format!("Read {}", summary_text(path)),
            load_skill(library, rest),
        )
    } else {
        match project.existing(path) {
            Err(e) => return fail(e, generic),
            Ok(found) => {
                let summary = if found.shown.is_empty() {
                    generic
                } else {
                    format!("Read {}", path_summary(&found.shown))
                };
                let loaded = if is_secret(&found.shown) && !allow_secret {
                    Err("needs_approval".to_string())
                } else {
                    load_file(&found.real)
                };
                (summary, loaded)
            }
        }
    };
    match loaded {
        Ok(bytes) => numbered(&bytes, offset, limit, summary),
        Err(e) => fail(e, summary),
    }
}

/// A file's bytes as numbered lines from `offset`, at most `limit` lines
/// and `MAX_CHARS` characters.
fn numbered(bytes: &[u8], offset: usize, limit: usize, summary: String) -> ToolOutcome {
    if bytes.contains(&0) {
        return fail(
            "That file looks like a binary file, so it cannot be read as text.".to_string(),
            summary,
        );
    }
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return ok("(empty file)".to_string(), summary);
    }
    let total = lines.len();
    if offset > total {
        return fail(
            format!(
                "The file has {}; the offset {offset} is past the end.",
                lines_word(total)
            ),
            summary,
        );
    }
    let mut out: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut last = offset - 1;
    for (i, line) in lines.iter().enumerate().skip(offset - 1).take(limit) {
        let n = i + 1;
        let len = line.chars().count();
        let room = MAX_CHARS - used;
        if len > room {
            if out.is_empty() {
                // The first line alone is longer than a whole call may return.
                let kept: String = line.chars().take(room).collect();
                out.push(format!(
                    "{n}\t{kept} [line cut at {MAX_CHARS} of {len} characters]"
                ));
                last = n;
            }
            break;
        }
        used += len;
        out.push(format!("{n}\t{line}"));
        last = n;
    }
    if last < total {
        out.push(format!("(lines {offset}-{last} of {total}; more remain)"));
    }
    ok(out.join("\n"), summary)
}

/// Which file a path led to when an approval card was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileId {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

const SWAPPED: &str = "That file changed after the user approved reading it, so it was not read.";

/// The identity of the regular file at `real`, without following a link.
/// `None` for a link, a folder, a special file or a missing path.
pub fn file_id(real: &std::path::Path) -> Option<FileId> {
    let meta = std::fs::symlink_metadata(real).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileId {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(not(unix))]
    Some(FileId {})
}

/// `read_file` for a file the user approved on a card: reads `target.real`
/// as resolved then, with no further resolution. The file is opened once,
/// without following a link and without blocking on a pipe; on the open
/// handle it must be a regular file and, on unix, the same file (device and
/// inode) as `id`. On Windows the identity is not compared on the handle;
/// the caller's check of the path before and after the read stands in.
pub fn read_resolved(target: &InProject, id: &FileId, args: &Value) -> ToolOutcome {
    let summary = format!("Read {}", path_summary(&target.shown));
    let (offset, limit) = match window(args) {
        Ok(w) => w,
        Err(e) => return fail(e, summary),
    };
    match load_resolved(&target.real, id) {
        Ok(bytes) => numbered(&bytes, offset, limit, summary),
        Err(e) => fail(e, summary),
    }
}

fn load_resolved(real: &std::path::Path, id: &FileId) -> Result<Vec<u8>, String> {
    const BIG: &str = "That file is larger than 5 MB, so it cannot be read.";
    // Checked before opening: opening a pipe that took its place would block.
    if file_id(real).as_ref() != Some(id) {
        return Err(SWAPPED.to_string());
    }
    let file = open_no_follow(real).map_err(|_| SWAPPED.to_string())?;
    let meta = file.metadata().map_err(|_| SWAPPED.to_string())?;
    if !meta.is_file() {
        return Err(SWAPPED.to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.dev() != id.dev || meta.ino() != id.ino {
            return Err(SWAPPED.to_string());
        }
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(BIG.to_string());
    }
    read_bounded(file, MAX_READ_BYTES).ok_or_else(|| BIG.to_string())
}

#[cfg(unix)]
fn open_no_follow(real: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(real)
}

#[cfg(not(unix))]
fn open_no_follow(real: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(real)
}

/// The `offset` (1-based) and `limit` arguments.
fn window(args: &Value) -> Result<(usize, usize), String> {
    let offset = optional_uint(args, "offset")?.unwrap_or(1);
    if offset < 1 {
        return Err("The argument \"offset\" must be 1 or more.".to_string());
    }
    let limit = optional_uint(args, "limit")?.unwrap_or(MAX_LINES as u64);
    if limit < 1 {
        return Err("The argument \"limit\" must be 1 or more.".to_string());
    }
    Ok((
        offset.min(usize::MAX as u64) as usize,
        limit.min(MAX_LINES as u64) as usize,
    ))
}

/// A file of a skill: the id may contain a colon, so split at the first slash.
fn load_skill(library: &Library, rest: &str) -> Result<Vec<u8>, String> {
    let (id, file) = rest
        .split_once('/')
        .ok_or_else(|| "A skill path looks like skill:{id}/{file}.".to_string())?;
    library.skill_file(id, file).map_err(|e| e.message)
}

/// The bytes of the regular file at `real`. A path that is not a regular file
/// is refused by `stat` before it is opened (opening a named pipe blocks), and
/// again on the open handle.
fn load_file(real: &std::path::Path) -> Result<Vec<u8>, String> {
    const UNREADABLE: &str = "That file could not be read.";
    const BIG: &str = "That file is larger than 5 MB, so it cannot be read.";
    match std::fs::metadata(real) {
        Ok(m) if m.is_dir() => {
            return Err(
                "That is a folder, not a file. Use list_dir to see what is in it.".to_string(),
            );
        }
        Ok(m) if !m.is_file() => return Err("That is not a regular file.".to_string()),
        Ok(_) => {}
        Err(_) => return Err(UNREADABLE.to_string()),
    }
    let file = std::fs::File::open(real).map_err(|_| UNREADABLE.to_string())?;
    let meta = file.metadata().map_err(|_| UNREADABLE.to_string())?;
    if !meta.is_file() {
        return Err("That is not a regular file.".to_string());
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(BIG.to_string());
    }
    read_bounded(file, MAX_READ_BYTES).ok_or_else(|| BIG.to_string())
}

pub fn list_dir(project: &Project, args: &Value) -> ToolOutcome {
    let given = match folder_arg(args) {
        Ok(p) => p,
        Err(e) => return fail(e, "Listed a folder".to_string()),
    };
    let found = match project.existing(given) {
        Ok(f) => f,
        Err(e) => return fail(e, "Listed a folder".to_string()),
    };
    let summary = if found.shown.is_empty() {
        "Listed the project folder".to_string()
    } else {
        format!("Listed {}", path_summary(&found.shown))
    };
    let shown = if found.shown.is_empty() {
        "the project folder"
    } else {
        &found.shown
    };
    if !found.real.is_dir() {
        return fail(format!("{shown} is not a folder."), summary);
    }
    let Ok(read) = std::fs::read_dir(&found.real) else {
        return fail("That folder could not be read.".to_string(), summary);
    };
    // (sort key, line); links and special files are not listed.
    let mut rows: Vec<(String, String)> = Vec::new();
    for entry in read.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let shown = clean(&name);
        if kind.is_dir() {
            rows.push((name.clone(), format!("{shown}/")));
        } else if kind.is_file() {
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            rows.push((name.clone(), format!("{shown}\t{bytes}")));
        }
    }
    rows.sort_by_cached_key(|(name, _)| (name.to_lowercase(), name.clone()));
    if rows.is_empty() {
        return ok("(empty folder)".to_string(), summary);
    }
    let total = rows.len();
    let mut lines: Vec<String> = rows.into_iter().take(MAX_ENTRIES).map(|(_, l)| l).collect();
    if total > MAX_ENTRIES {
        lines.push(format!("(first {MAX_ENTRIES} of {total} entries shown)"));
    }
    ok(lines.join("\n"), summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn resolved(p: &Project, name: &str) -> (InProject, FileId) {
        let target = p.existing(name).unwrap();
        let id = file_id(&target.real).unwrap();
        (target, id)
    }

    #[test]
    fn read_resolved_reads_the_approved_file_like_read_file() {
        let (t, p, lib) = setup();
        fs::write(t.path().join(".env"), "A=1\nB=2\nC=3\n").unwrap();
        let (target, id) = resolved(&p, ".env");
        let args = json!({"path": ".env", "offset": 2, "limit": 1});
        assert_eq!(
            read_resolved(&target, &id, &args),
            read_file(&p, &lib, &args, true)
        );
        assert!(
            read_resolved(&target, &id, &json!({}))
                .text
                .contains("1\tA=1")
        );
        // Bad window arguments are still errors.
        let bad = read_resolved(&target, &id, &json!({"offset": 0}));
        assert_eq!(bad.status, ToolStatus::Error);
    }

    #[cfg(unix)]
    #[test]
    fn read_resolved_refuses_a_file_swapped_after_the_card() {
        const SWAPPED: &str =
            "That file changed after the user approved reading it, so it was not read.";
        let (t, p, _) = setup();
        let env = t.path().join(".env");
        fs::write(t.path().join("other.pem"), "PRIVATE\n").unwrap();
        // Replaced by another file (a different inode at the same path).
        fs::write(&env, "A=1\n").unwrap();
        let (target, id) = resolved(&p, ".env");
        fs::write(t.path().join("swap"), "PRIVATE\n").unwrap();
        fs::rename(t.path().join("swap"), &env).unwrap();
        let out = read_resolved(&target, &id, &json!({}));
        assert_eq!(
            (out.status, out.text.as_str()),
            (ToolStatus::Error, SWAPPED)
        );
        // Replaced by a link to another secret.
        fs::write(&env, "A=1\n").unwrap();
        let (target, id) = resolved(&p, ".env");
        fs::remove_file(&env).unwrap();
        std::os::unix::fs::symlink(t.path().join("other.pem"), &env).unwrap();
        let out = read_resolved(&target, &id, &json!({}));
        assert_eq!(
            (out.status, out.text.as_str()),
            (ToolStatus::Error, SWAPPED)
        );
        // Replaced by a named pipe: refused without blocking.
        fs::remove_file(&env).unwrap();
        fs::write(&env, "A=1\n").unwrap();
        let (target, id) = resolved(&p, ".env");
        fs::remove_file(&env).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(&env)
            .status()
            .unwrap();
        assert!(made.success());
        let out = read_resolved(&target, &id, &json!({}));
        assert_eq!(out.status, ToolStatus::Error);
        assert!(!out.text.contains("PRIVATE"));
        // The same file, changed in place, is still the approved file.
        fs::remove_file(&env).unwrap();
        fs::write(&env, "A=1\n").unwrap();
        let (target, id) = resolved(&p, ".env");
        fs::write(&env, "A=2\n").unwrap();
        assert!(read_resolved(&target, &id, &json!({})).text.contains("A=2"));
    }

    fn setup() -> (TempDir, Project, Library) {
        let t = tempfile::tempdir().unwrap();
        let p = Project::open(t.path().to_str().unwrap()).unwrap();
        let none = t.path().join("none");
        (t, p, Library::load(&none, &none))
    }

    fn read(p: &Project, l: &Library, args: Value) -> ToolOutcome {
        read_file(p, l, &args, false)
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn read_file_numbers_lines_from_one_with_a_tab() {
        let (t, p, l) = setup();
        fs::write(t.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let o = read(&p, &l, json!({"path": "a.txt"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "1\talpha\n2\tbeta\n3\tgamma");
        assert_eq!(o.summary, "Read a.txt");
        assert_eq!(o.detail, None);
    }

    #[test]
    fn read_file_offset_and_limit_select_a_window() {
        let (t, p, l) = setup();
        fs::write(t.path().join("a.txt"), numbered(10)).unwrap();
        let o = read(&p, &l, json!({"path": "a.txt", "offset": 3, "limit": 2}));
        assert_eq!(
            o.text,
            "3\tline 3\n4\tline 4\n(lines 3-4 of 10; more remain)"
        );
    }

    #[test]
    fn read_file_returns_400_lines_and_says_more_remain() {
        let (t, p, l) = setup();
        fs::write(t.path().join("a.txt"), numbered(401)).unwrap();
        let o = read(&p, &l, json!({"path": "a.txt"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert!(o.text.contains("400\tline 400"));
        assert!(!o.text.contains("401\t"));
        assert!(o.text.ends_with("(lines 1-400 of 401; more remain)"));
    }

    #[test]
    fn read_file_cuts_a_huge_line_at_16000_characters_with_a_notice() {
        let (t, p, l) = setup();
        fs::write(t.path().join("min.js"), "x".repeat(20_000)).unwrap();
        let o = read(&p, &l, json!({"path": "min.js"}));
        assert_eq!(o.status, ToolStatus::Ok);
        let body = o.text.strip_prefix("1\t").unwrap();
        assert_eq!(body.chars().take_while(|c| *c == 'x').count(), 16_000);
        assert!(body.contains("cut at 16000 of 20000 characters"));
    }

    #[test]
    fn read_file_stops_a_window_at_16000_characters_across_lines() {
        let (t, p, l) = setup();
        let line = "y".repeat(99);
        fs::write(t.path().join("a.txt"), format!("{line}\n").repeat(300)).unwrap();
        let o = read(&p, &l, json!({"path": "a.txt"}));
        assert!(o.text.contains("more remain)"));
        let content: usize = o
            .text
            .lines()
            .filter(|s| s.contains('\t'))
            .map(|s| s.split_once('\t').unwrap().1.chars().count())
            .sum();
        assert!(content <= 16_000, "{content}");
    }

    #[test]
    fn read_file_refuses_binary_big_and_directories_in_plain_words() {
        let (t, p, l) = setup();
        fs::write(t.path().join("b.bin"), b"ab\0cd").unwrap();
        let o = read(&p, &l, json!({"path": "b.bin"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("binary"), "{}", o.text);
        assert_eq!(o.summary, "Read b.bin");

        let big = fs::File::create(t.path().join("big.txt")).unwrap();
        big.set_len(5_000_001).unwrap();
        let o = read(&p, &l, json!({"path": "big.txt"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("5 MB"), "{}", o.text);

        fs::create_dir(t.path().join("d")).unwrap();
        let o = read(&p, &l, json!({"path": "d"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("folder"), "{}", o.text);
    }

    #[test]
    fn read_file_accepts_a_file_of_exactly_5_mb() {
        let (t, p, l) = setup();
        let f = fs::File::create(t.path().join("edge.txt")).unwrap();
        f.set_len(5_000_000).unwrap(); // all NUL: binary, but not "too big"
        drop(f);
        let o = read(&p, &l, json!({"path": "edge.txt"}));
        assert!(!o.text.contains("5 MB"), "{}", o.text);
    }

    #[test]
    fn read_file_handles_crlf_and_hides_a_bom() {
        let (t, p, l) = setup();
        fs::write(t.path().join("w.txt"), "\u{feff}one\r\ntwo\r\n").unwrap();
        let o = read(&p, &l, json!({"path": "w.txt"}));
        assert_eq!(o.text, "1\tone\n2\ttwo");
    }

    #[test]
    fn read_file_empty_file_and_offset_past_the_end() {
        let (t, p, l) = setup();
        fs::write(t.path().join("e.txt"), "").unwrap();
        let o = read(&p, &l, json!({"path": "e.txt"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "(empty file)");

        fs::write(t.path().join("a.txt"), numbered(3)).unwrap();
        let o = read(&p, &l, json!({"path": "a.txt", "offset": 9}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("3 lines"), "{}", o.text);
    }

    #[test]
    fn read_file_secret_needs_approval_unless_allowed() {
        let (t, p, l) = setup();
        fs::write(t.path().join(".env"), "KEY=1\n").unwrap();
        fs::write(t.path().join(".env.example"), "KEY=\n").unwrap();
        let o = read(&p, &l, json!({"path": ".env"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(o.text, "needs_approval");
        let o = read_file(&p, &l, &json!({"path": ".env"}), true);
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "1\tKEY=1");
        let o = read(&p, &l, json!({"path": ".env.example"}));
        assert_eq!(o.status, ToolStatus::Ok);
    }

    #[test]
    fn read_file_path_errors_come_from_the_path_check() {
        let (_t, p, l) = setup();
        let o = read(&p, &l, json!({"path": "../x"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(o.text, crate::agent::paths::OUTSIDE);
        let o = read(&p, &l, json!({"path": "nope.txt"}));
        assert_eq!(o.text, "There is no file or folder at nope.txt.");
    }

    fn skill_library(root: &Path) -> Library {
        let builtin = root.join("builtin");
        let dir = builtin.join("superpowers").join("skills/brainstorming");
        fs::create_dir_all(&dir).unwrap();
        fs::write(builtin.join("default-skills.json"), r#"{"on":[]}"#).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: brainstorming\n---\nBody\n",
        )
        .unwrap();
        fs::write(dir.join("notes.md"), "skill notes\n").unwrap();
        fs::write(dir.join("hidden.md"), "not listed\n").unwrap();
        let manifest = json!({
            "id": "superpowers", "title": "T", "repo": "o/r", "commit": "abc",
            "license": "MIT", "license_files": [], "skipped": [], "checksum": "x",
            "skills": [{
                "id": "superpowers:brainstorming", "pack": "superpowers",
                "name": "brainstorming", "description": "d",
                "dir": "skills/brainstorming", "acts_on_computer": false,
                "body_chars": 10, "files": ["notes.md"], "license": null, "broken": null
            }],
        });
        fs::write(builtin.join("superpowers/pack.json"), manifest.to_string()).unwrap();
        Library::load(&builtin, &root.join("data"))
    }

    #[test]
    fn read_file_reads_a_listed_skill_file_and_refuses_an_unlisted_one() {
        let t = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let p = Project::open(proj.path().to_str().unwrap()).unwrap();
        let l = skill_library(t.path());
        let o = read(
            &p,
            &l,
            json!({"path": "skill:superpowers:brainstorming/notes.md"}),
        );
        assert_eq!(o.status, ToolStatus::Ok, "{}", o.text);
        assert_eq!(o.text, "1\tskill notes");
        assert_eq!(o.summary, "Read skill:superpowers:brainstorming/notes.md");
        let o = read(
            &p,
            &l,
            json!({"path": "skill:superpowers:brainstorming/hidden.md"}),
        );
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(o.text, "That file is not part of this skill.");
        let o = read(&p, &l, json!({"path": "skill:superpowers:brainstorming"}));
        assert_eq!(o.status, ToolStatus::Error);
    }

    #[test]
    fn list_dir_shows_sizes_and_slashes_sorted() {
        let (t, p, _l) = setup();
        fs::write(t.path().join("b.txt"), "12345").unwrap();
        fs::write(t.path().join("a.txt"), "").unwrap();
        fs::create_dir(t.path().join("src")).unwrap();
        fs::create_dir(t.path().join("src/deep")).unwrap();
        fs::write(t.path().join("src/m.rs"), "fn").unwrap();
        let o = list_dir(&p, &json!({}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "a.txt\t0\nb.txt\t5\nsrc/");
        assert_eq!(o.summary, "Listed the project folder");
        for root in [json!({"path": ""}), json!({"path": "."})] {
            assert_eq!(list_dir(&p, &root).text, "a.txt\t0\nb.txt\t5\nsrc/");
        }
        let o = list_dir(&p, &json!({"path": "src"}));
        assert_eq!(o.text, "deep/\nm.rs\t2");
        assert_eq!(o.summary, "Listed src");
    }

    #[test]
    fn list_dir_errors_for_missing_and_for_a_file_and_caps_at_500() {
        let (t, p, _l) = setup();
        let o = list_dir(&p, &json!({"path": "ghost"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(o.text, "There is no file or folder at ghost.");
        fs::write(t.path().join("f.txt"), "x").unwrap();
        let o = list_dir(&p, &json!({"path": "f.txt"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("not a folder"));
        fs::create_dir(t.path().join("many")).unwrap();
        for i in 0..501 {
            fs::write(t.path().join(format!("many/f{i:03}.txt")), "").unwrap();
        }
        let o = list_dir(&p, &json!({"path": "many"}));
        assert_eq!(o.text.lines().filter(|l| l.contains('\t')).count(), 500);
        assert!(o.text.ends_with("(first 500 of 501 entries shown)"));
        let o = list_dir(&p, &json!({"path": 5}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("path"));
    }

    #[test]
    fn read_file_without_a_usable_path_names_it() {
        let (_t, p, l) = setup();
        for args in [json!({}), json!({"path": 3}), json!(null)] {
            let o = read(&p, &l, args);
            assert_eq!(o.status, ToolStatus::Error);
            assert!(o.text.contains("\"path\""), "{}", o.text);
        }
        let o = read(&p, &l, json!({"path": "a", "offset": "x"}));
        assert!(o.text.contains("\"offset\""));
        let o = read(&p, &l, json!({"path": "a", "limit": 0}));
        assert!(o.text.contains("\"limit\""));
    }

    #[test]
    fn clean_replaces_control_and_format_characters() {
        assert_eq!(clean("a\nb\tc\u{202e}d\u{200b}e\0f"), "a?b?c?d?e?f");
        assert_eq!(clean("plain é/ü.rs"), "plain é/ü.rs");
    }

    #[test]
    fn summary_text_cleans_and_cuts_at_80_characters() {
        assert_eq!(summary_text("short"), "short");
        let long = "é".repeat(100);
        let got = summary_text(&long);
        assert_eq!(got.chars().count(), 81);
        assert!(got.ends_with('…'));
        assert_eq!(summary_text("a\nb"), "a?b");
        assert_eq!(summary_text(&"x".repeat(80)), "x".repeat(80));
    }

    #[test]
    fn read_summary_uses_the_resolved_path_never_the_raw_string() {
        let (t, p, l) = setup();
        fs::create_dir(t.path().join("src")).unwrap();
        fs::write(t.path().join("src/a.txt"), "x\n").unwrap();
        let o = read(&p, &l, json!({"path": "./src//a.txt"}));
        assert_eq!(o.summary, "Read src/a.txt");
        // Refused before resolution: a fixed line.
        let o = read(&p, &l, json!({"path": "../x\nEvil"}));
        assert_eq!(o.summary, "Read a file");
        let o = read(&p, &l, json!({"path": "no\nsuch.txt"}));
        assert_eq!(o.summary, "Read a file");
        // Refused after resolution: the shown path.
        fs::write(t.path().join("b.bin"), b"\0").unwrap();
        let o = read(&p, &l, json!({"path": "./b.bin"}));
        assert_eq!(o.summary, "Read b.bin");
        let o = read(&p, &l, json!({"path": "skill:x/\u{202e}y\nz"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(!o.summary.contains(['\n', '\u{202e}']), "{}", o.summary);
    }

    #[cfg(unix)]
    #[test]
    fn read_file_refuses_a_named_pipe_promptly() {
        let (t, p, l) = setup();
        let fifo = t.path().join("pipe.txt");
        if !std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success())
        {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_file(&p, &l, &json!({"path": "pipe.txt"}), false));
        });
        let o = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("read_file hung on a FIFO");
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("not a regular file"), "{}", o.text);
    }

    #[cfg(unix)]
    #[test]
    fn list_dir_replaces_control_characters_in_names() {
        let (t, p, _l) = setup();
        fs::write(t.path().join("a\nforged\t99"), "").unwrap();
        let o = list_dir(&p, &json!({}));
        assert_eq!(o.text, "a?forged?99\t0");
    }

    #[test]
    fn path_summary_cleans_and_cuts_at_200_characters() {
        assert_eq!(path_summary("src/a.rs"), "src/a.rs");
        assert_eq!(path_summary("a\nb\u{202e}"), "a?b?");
        let got = path_summary(&"d/".repeat(150));
        assert_eq!(got.chars().count(), 201);
        assert!(got.ends_with('…'));
    }

    #[cfg(unix)]
    #[test]
    fn no_tool_summary_holds_a_control_or_format_character() {
        use crate::agent::paths::invisible;
        use crate::agent::search::{find_files, search};
        let (t, p, l) = setup();
        let dir = "d\n\u{202e}x";
        let file = "f\n\u{202e}.txt";
        fs::create_dir(t.path().join(dir)).unwrap();
        fs::write(t.path().join(dir).join(file), "needle\n").unwrap();
        // Names with such characters cannot be typed as a path, but a link can
        // lead to them, and then `shown` carries them.
        std::os::unix::fs::symlink(t.path().join(dir), t.path().join("link")).unwrap();
        let both = format!("{dir}/{file}");
        let through = format!("link/{file}");
        let outcomes = [
            read(&p, &l, json!({"path": through})),
            list_dir(&p, &json!({"path": "link"})),
            find_files(&p, &json!({"pattern": "*.txt", "path": "link"})),
            search(&p, &json!({"pattern": "needle", "path": "link"})),
            search(&p, &json!({"pattern": "needle", "path": through})),
            read(&p, &l, json!({"path": both})),
            read(&p, &l, json!({"path": dir})),
            list_dir(&p, &json!({"path": dir})),
            list_dir(&p, &json!({"path": both})),
            find_files(&p, &json!({"pattern": "*.txt", "path": dir})),
            find_files(&p, &json!({"pattern": "*", "path": both})),
            search(&p, &json!({"pattern": "needle", "path": dir})),
            search(&p, &json!({"pattern": "needle", "path": both})),
            search(&p, &json!({"pattern": "needle"})),
            crate::agent::plan::update_plan(&json!({"items": [{"text": "a\nb", "state": "done"}]}))
                .0,
        ];
        for o in outcomes {
            assert!(
                !o.summary.chars().any(|c| c.is_control() || invisible(c)),
                "{:?}",
                o.summary
            );
        }
    }

    #[test]
    fn read_file_on_a_git_path_needs_approval() {
        let (t, p, l) = setup();
        fs::create_dir_all(t.path().join("sub/.git")).unwrap();
        fs::write(t.path().join("sub/.git/HEAD"), "ref: x\n").unwrap();
        fs::write(t.path().join(".gitignore"), "x\n").unwrap();
        let o = read(&p, &l, json!({"path": "sub/.git/HEAD"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(o.text, "needs_approval");
        let o = read_file(&p, &l, &json!({"path": "sub/.git/HEAD"}), true);
        assert_eq!(o.text, "1\tref: x");
        assert_eq!(
            read(&p, &l, json!({"path": ".gitignore"})).status,
            ToolStatus::Ok
        );
    }
}
