//! `read_file` and `list_dir`, plus the small argument helpers every read-only
//! tool shares.

use serde_json::Value;

use super::paths::{Project, invisible};
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
                    format!("Read {}", clean(&found.shown))
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
    let bytes = match loaded {
        Ok(b) => b,
        Err(e) => return fail(e, summary),
    };
    if bytes.contains(&0) {
        return fail(
            "That file looks like a binary file, so it cannot be read as text.".to_string(),
            summary,
        );
    }
    let text = String::from_utf8_lossy(&bytes);
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
        format!("Listed {}", found.shown)
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
}
