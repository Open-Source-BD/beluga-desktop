//! `find_files` and `search`: walks of the project that respect `.gitignore`,
//! never follow links and skip secret files.

use globset::{GlobBuilder, GlobMatcher};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use serde_json::Value;

use super::paths::{InProject, Project};
use super::read::{clean, fail, folder_arg, ok, optional_str, required_str, summary_text};
use super::secrets::is_secret;
use super::{ToolOutcome, ToolStatus};
use crate::folder::{FileEntry, MAX_READ_BYTES, MAX_WALKED_FILES, walk_with};

/// Most paths one `find_files` call returns.
const MAX_FOUND: usize = 200;
/// Most lines one `search` call returns.
const MAX_MATCHES: usize = 100;
/// Longest regular expression `search` accepts.
const MAX_PATTERN_CHARS: usize = 1_000;
/// Longest line `search` returns before cutting it.
const MAX_LINE_CHARS: usize = 500;
/// Longest glob accepted.
const MAX_GLOB_CHARS: usize = 500;
const INCOMPLETE: &str =
    "(The folder is large, so the walk stopped early and this result is incomplete.)";

/// A glob. Without a `/` it matches the file name at any depth (ripgrep's
/// rule); with one it matches the forward-slash path below the searched
/// folder, where `*` does not cross a `/` and `**` does.
struct Filter {
    matcher: GlobMatcher,
    by_name: bool,
}

impl Filter {
    fn new(pattern: &str, name: &str) -> Result<Filter, String> {
        if pattern.chars().count() > MAX_GLOB_CHARS {
            return Err(format!(
                "The argument \"{name}\" is longer than {MAX_GLOB_CHARS} characters."
            ));
        }
        GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map(|g| Filter {
                matcher: g.compile_matcher(),
                by_name: !pattern.contains('/'),
            })
            .map_err(|e| format!("The argument \"{name}\" is not a valid glob: {}", e.kind()))
    }

    fn is_match(&self, rel: &str) -> bool {
        let target = if self.by_name {
            rel.rsplit('/').next().unwrap_or(rel)
        } else {
            rel
        };
        self.matcher.is_match(target)
    }
}

/// Project-relative display path of `rel` found below `base`.
fn shown_path(base: &InProject, rel: &str) -> String {
    if base.shown.is_empty() {
        rel.to_string()
    } else {
        format!("{}/{rel}", base.shown)
    }
}

/// The folder the walk starts from, from the optional `path` argument.
fn folder_base(project: &Project, args: &Value) -> Result<InProject, String> {
    let given = folder_arg(args)?;
    let found = project.existing(given)?;
    if !found.real.is_dir() {
        return Err(format!("{} is not a folder.", found.shown));
    }
    Ok(found)
}

/// Files below `base` that are not secret: (path below base, entry).
fn candidate_files(base: &InProject) -> (Vec<FileEntry>, bool) {
    let (entries, cut) = walk_with(&base.real, MAX_WALKED_FILES, true);
    let files = entries
        .into_iter()
        .filter(|e| !e.is_dir)
        .filter(|e| {
            let shown = shown_path(base, &e.path);
            // `.git` is never searched, even when it is the folder given.
            !is_secret(&shown) && !shown.split('/').any(|seg| seg == ".git")
        })
        .collect();
    (files, cut)
}

pub fn find_files(project: &Project, args: &Value) -> ToolOutcome {
    let pattern = match required_str(args, "pattern") {
        Ok(p) => p,
        Err(e) => return fail(e, "Found files".to_string()),
    };
    let found_none = |n: usize| format!("Found {n} files for {}", summary_text(pattern));
    let matcher = match Filter::new(pattern, "pattern") {
        Ok(m) => m,
        Err(e) => return fail(e, found_none(0)),
    };
    let base = match folder_base(project, args) {
        Ok(b) => b,
        Err(e) => return fail(e, found_none(0)),
    };
    let (files, cut) = candidate_files(&base);
    let matches: Vec<String> = files
        .iter()
        .filter(|e| matcher.is_match(&e.path))
        .map(|e| clean(&shown_path(&base, &e.path)))
        .collect();
    let total = matches.len();
    let mut lines: Vec<String> = matches.into_iter().take(MAX_FOUND).collect();
    if total == 0 {
        lines.push("No files matched.".to_string());
    }
    if total > MAX_FOUND {
        lines.push(format!(
            "(first {MAX_FOUND} of {total} matching files shown)"
        ));
    }
    if cut {
        lines.push(INCOMPLETE.to_string());
    }
    ok(lines.join("\n"), found_none(total))
}

/// Collects the matching lines of one file.
struct Collect {
    lines: Vec<(u64, String)>,
    room: usize,
    binary: bool,
}

impl Sink for Collect {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let n = m.line_number().unwrap_or(0);
        for raw in m.lines() {
            let text = String::from_utf8_lossy(raw);
            let text = text.trim_end_matches(['\n', '\r']);
            let line = if text.chars().count() > MAX_LINE_CHARS {
                let head: String = text.chars().take(MAX_LINE_CHARS).collect();
                format!("{head}…")
            } else {
                text.to_string()
            };
            self.lines.push((n, line));
            if self.lines.len() >= self.room {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn binary_data(&mut self, _: &Searcher, _: u64) -> Result<bool, Self::Error> {
        self.binary = true;
        Ok(false)
    }
}

pub fn search(project: &Project, args: &Value) -> ToolOutcome {
    let pattern = match required_str(args, "pattern") {
        Ok(p) => p,
        Err(e) => return fail(e, "Searched the project".to_string()),
    };
    let summary = format!("Searched for \"{}\"", summary_text(pattern));
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return fail(
            "The pattern is longer than 1,000 characters.".to_string(),
            summary,
        );
    }
    let glob_text = optional_str(args, "glob").ok().flatten();
    let filter = match optional_str(args, "glob") {
        Ok(None) => None,
        Ok(Some(g)) => match Filter::new(g, "glob") {
            Ok(m) => Some(m),
            Err(e) => return fail(e, summary),
        },
        Err(e) => return fail(e, summary),
    };
    let matcher = match RegexMatcherBuilder::new().build(pattern) {
        Ok(m) => m,
        Err(e) => {
            return fail(
                format!("The pattern is not a valid regular expression: {e}"),
                summary,
            );
        }
    };
    let given = match folder_arg(args) {
        Ok(p) => p,
        Err(e) => return fail(e, summary),
    };
    let base = match project.existing(given) {
        Ok(b) => b,
        Err(e) => return fail(e, summary),
    };
    // (display path, real path, bytes) of every file to search.
    let (targets, cut): (Vec<(String, std::path::PathBuf, u64)>, bool) = if base.real.is_dir() {
        let (files, cut) = candidate_files(&base);
        let targets = files
            .into_iter()
            .filter(|e| filter.as_ref().is_none_or(|g| g.is_match(&e.path)))
            .map(|e| {
                let mut real = base.real.clone();
                real.extend(e.path.split('/'));
                (clean(&shown_path(&base, &e.path)), real, e.bytes)
            })
            .collect();
        (targets, cut)
    } else if is_secret(&base.shown) {
        (Vec::new(), false)
    } else {
        // Only a regular file is opened (opening a named pipe blocks).
        let meta = std::fs::metadata(&base.real).ok().filter(|m| m.is_file());
        let one = meta.map(|m| (clean(&base.shown), base.real.clone(), m.len()));
        (one.into_iter().collect(), false)
    };

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(0))
        .build();
    let mut out: Vec<String> = Vec::new();
    let mut more = false;
    let mut skipped = 0usize;
    'files: for (shown, real, bytes) in targets {
        if bytes > MAX_READ_BYTES {
            skipped += 1;
            continue;
        }
        let mut sink = Collect {
            lines: Vec::new(),
            // One more than the cap, to know whether more exist.
            room: MAX_MATCHES + 1 - out.len(),
            binary: false,
        };
        if searcher.search_path(&matcher, &real, &mut sink).is_err() || sink.binary {
            skipped += 1;
            continue;
        }
        for (n, line) in sink.lines {
            if out.len() == MAX_MATCHES {
                more = true;
                break 'files;
            }
            out.push(format!("{shown}:{n}:{line}"));
        }
    }
    if out.is_empty() {
        out.push(match glob_text {
            Some(g) => format!("No matches. (Only files matching {g} were searched.)"),
            None => "No matches.".to_string(),
        });
    }
    if more {
        out.push(format!(
            "(first {MAX_MATCHES} matching lines shown; more exist)"
        ));
    }
    if skipped > 0 {
        out.push(format!(
            "({skipped} files skipped: binary, unreadable or over 5 MB)"
        ));
    }
    if cut {
        out.push(INCOMPLETE.to_string());
    }
    ToolOutcome {
        status: ToolStatus::Ok,
        text: out.join("\n"),
        summary,
        detail: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Project) {
        let t = tempfile::tempdir().unwrap();
        let p = Project::open(t.path().to_str().unwrap()).unwrap();
        (t, p)
    }

    fn put(t: &TempDir, rel: &str, body: &str) {
        let path = t.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn find_files_matches_nested_and_skips_ignored_and_skipped_dirs() {
        let (t, p) = setup();
        put(&t, "a.ts", "");
        put(&t, "src/b.ts", "");
        put(&t, "src/deep/c.ts", "");
        put(&t, "src/d.rs", "");
        put(&t, "node_modules/x/e.ts", "");
        put(&t, "gen/f.ts", "");
        put(&t, ".gitignore", "gen/\n");
        let o = find_files(&p, &json!({"pattern": "**/*.ts"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "a.ts\nsrc/b.ts\nsrc/deep/c.ts");
        assert_eq!(o.summary, "Found 3 files for **/*.ts");
        assert_eq!(o.detail, None);
    }

    #[test]
    fn find_files_in_a_subfolder_shows_project_relative_paths() {
        let (t, p) = setup();
        put(&t, "src/b.ts", "");
        put(&t, "src/deep/c.ts", "");
        put(&t, "other/z.ts", "");
        let o = find_files(&p, &json!({"pattern": "*.ts", "path": "src"}));
        assert_eq!(o.text, "src/b.ts\nsrc/deep/c.ts");
        let o = find_files(&p, &json!({"pattern": "deep/*.ts", "path": "src"}));
        assert_eq!(o.text, "src/deep/c.ts");
        let o = find_files(&p, &json!({"pattern": "**/*.ts", "path": "src"}));
        assert_eq!(o.text, "src/b.ts\nsrc/deep/c.ts");
    }

    #[test]
    fn find_files_skips_secret_files_and_reports_no_match() {
        let (t, p) = setup();
        put(&t, ".env", "K=1");
        put(&t, "server.pem", "x");
        let o = find_files(&p, &json!({"pattern": "**/*"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "No files matched.");
        assert_eq!(o.summary, "Found 0 files for **/*");
    }

    #[test]
    fn find_files_caps_at_200_and_says_so() {
        let (t, p) = setup();
        for i in 0..205 {
            put(&t, &format!("f{i:03}.txt"), "");
        }
        let o = find_files(&p, &json!({"pattern": "*.txt"}));
        assert_eq!(o.text.lines().filter(|l| l.ends_with(".txt")).count(), 200);
        assert!(o.text.ends_with("(first 200 of 205 matching files shown)"));
        assert_eq!(o.summary, "Found 205 files for *.txt");
    }

    #[test]
    fn find_files_errors_name_the_problem() {
        let (t, p) = setup();
        put(&t, "f.txt", "");
        let o = find_files(&p, &json!({}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("\"pattern\""));
        let o = find_files(&p, &json!({"pattern": "[a-"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("pattern"), "{}", o.text);
        let o = find_files(&p, &json!({"pattern": "*", "path": "f.txt"}));
        assert!(o.text.contains("not a folder"));
        let o = find_files(&p, &json!({"pattern": "*", "path": "../x"}));
        assert_eq!(o.text, crate::agent::paths::OUTSIDE);
        let o = find_files(&p, &json!({"pattern": "*", "path": 4}));
        assert!(o.text.contains("\"path\""));
    }

    #[test]
    fn search_finds_lines_with_path_and_number() {
        let (t, p) = setup();
        put(&t, "a.rs", "fn main() {}\nlet needle = 1;\n");
        put(&t, "src/b.rs", "needle again\r\nnothing\n");
        put(&t, "node_modules/n.js", "needle");
        let o = search(&p, &json!({"pattern": "needle"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "a.rs:2:let needle = 1;\nsrc/b.rs:1:needle again");
        assert_eq!(o.summary, "Searched for \"needle\"");
        assert_eq!(o.detail, None);
    }

    #[test]
    fn search_path_and_glob_narrow_the_search() {
        let (t, p) = setup();
        put(&t, "a.rs", "hit\n");
        put(&t, "a.ts", "hit\n");
        put(&t, "src/b.rs", "hit\n");
        let o = search(&p, &json!({"pattern": "hit", "glob": "*.rs"}));
        assert_eq!(o.text, "a.rs:1:hit\nsrc/b.rs:1:hit");
        let o = search(&p, &json!({"pattern": "hit", "path": "src"}));
        assert_eq!(o.text, "src/b.rs:1:hit");
        let o = search(&p, &json!({"pattern": "hit", "path": "a.ts"}));
        assert_eq!(o.text, "a.ts:1:hit");
        let o = search(&p, &json!({"pattern": "zzz"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.text, "No matches.");
    }

    #[test]
    fn search_invalid_pattern_is_an_error_naming_the_problem() {
        let (_t, p) = setup();
        let o = search(&p, &json!({"pattern": "(unclosed"}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(
            o.text
                .starts_with("The pattern is not a valid regular expression"),
            "{}",
            o.text
        );
        let o = search(&p, &json!({"pattern": "x".repeat(1001)}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("1,000"), "{}", o.text);
        let o = search(&p, &json!({"pattern": "ok", "glob": "[a-"}));
        assert!(o.text.contains("\"glob\""), "{}", o.text);
        let o = search(&p, &json!({}));
        assert!(o.text.contains("\"pattern\""));
    }

    #[test]
    fn search_caps_at_100_lines() {
        let (t, p) = setup();
        put(&t, "many.txt", &"hit\n".repeat(150));
        let o = search(&p, &json!({"pattern": "hit"}));
        assert_eq!(o.text.lines().filter(|l| l.contains(":hit")).count(), 100);
        assert!(
            o.text
                .ends_with("(first 100 matching lines shown; more exist)")
        );
    }

    #[test]
    fn search_skips_secret_and_binary_files() {
        let (t, p) = setup();
        put(&t, ".env", "needle=1\n");
        put(&t, "key.pem", "needle\n");
        fs::write(t.path().join("bin.dat"), b"needle\nzz\0\x01needle\n").unwrap();
        put(&t, "ok.txt", "needle\n");
        let o = search(&p, &json!({"pattern": "needle"}));
        assert_eq!(
            o.text,
            "ok.txt:1:needle\n(1 files skipped: binary, unreadable or over 5 MB)"
        );
    }

    #[test]
    fn search_cuts_a_long_line_at_500_characters() {
        let (t, p) = setup();
        put(&t, "min.js", &format!("needle{}\n", "z".repeat(2000)));
        let o = search(&p, &json!({"pattern": "needle"}));
        let line = o.text.strip_prefix("min.js:1:").unwrap();
        assert_eq!(line.chars().count(), 501);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn search_is_case_sensitive_and_supports_regex() {
        let (t, p) = setup();
        put(&t, "a.txt", "Foo\nfoo\nfo0\n");
        assert_eq!(search(&p, &json!({"pattern": "foo"})).text, "a.txt:2:foo");
        assert_eq!(
            search(&p, &json!({"pattern": "^f\\w\\d$"})).text,
            "a.txt:3:fo0"
        );
    }

    fn glob_fixture() -> (TempDir, Project) {
        let (t, p) = setup();
        put(&t, "a.rs", "hit\n");
        put(&t, "src/b.rs", "hit\n");
        put(&t, "src/deep/c.rs", "hit\n");
        (t, p)
    }

    #[test]
    fn a_glob_without_a_slash_matches_the_file_name_at_any_depth() {
        let (_t, p) = glob_fixture();
        let got = search(&p, &json!({"pattern": "hit", "glob": "*.rs"})).text;
        assert_eq!(got, "a.rs:1:hit\nsrc/b.rs:1:hit\nsrc/deep/c.rs:1:hit");
        let got = find_files(&p, &json!({"pattern": "c.rs"})).text;
        assert_eq!(got, "src/deep/c.rs");
    }

    #[test]
    fn a_glob_with_a_slash_matches_the_path_and_star_stays_in_a_folder() {
        let (_t, p) = glob_fixture();
        let got = search(&p, &json!({"pattern": "hit", "glob": "src/*.rs"})).text;
        assert_eq!(got, "src/b.rs:1:hit");
        let got = search(&p, &json!({"pattern": "hit", "glob": "**/*.rs"})).text;
        assert_eq!(got, "a.rs:1:hit\nsrc/b.rs:1:hit\nsrc/deep/c.rs:1:hit");
    }

    #[test]
    fn a_glob_that_matched_nothing_is_named_in_the_result() {
        let (_t, p) = glob_fixture();
        let o = search(&p, &json!({"pattern": "hit", "glob": "*.zzz"}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(
            o.text,
            "No matches. (Only files matching *.zzz were searched.)"
        );
        let o = search(&p, &json!({"pattern": "hit", "glob": "x".repeat(501)}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(
            o.text.contains("\"glob\"") && o.text.contains("500"),
            "{}",
            o.text
        );
    }

    #[test]
    fn hidden_files_are_found_and_searched_except_git_and_secrets() {
        let (t, p) = setup();
        put(&t, ".github/workflows/ci.yml", "needle\n");
        put(&t, ".eslintrc.json", "needle\n");
        put(&t, ".git/config", "needle\n");
        put(&t, ".env", "needle\n");
        put(&t, "node_modules/x.js", "needle\n");
        let o = find_files(&p, &json!({"pattern": "**/*"}));
        assert_eq!(o.text, ".eslintrc.json\n.github/workflows/ci.yml");
        let o = search(&p, &json!({"pattern": "needle"}));
        assert_eq!(
            o.text,
            ".eslintrc.json:1:needle\n.github/workflows/ci.yml:1:needle"
        );
        let o = find_files(&p, &json!({"pattern": "*.yml", "path": ".github"}));
        assert_eq!(o.text, ".github/workflows/ci.yml");
        let o = find_files(&p, &json!({"pattern": "*", "path": ".git"}));
        assert_eq!(o.text, "No files matched.");
    }

    #[test]
    fn skipped_files_are_counted_in_one_line() {
        let (t, p) = setup();
        put(&t, "ok.txt", "needle\n");
        fs::write(t.path().join("bin.dat"), b"needle\0").unwrap();
        let big = fs::File::create(t.path().join("big.txt")).unwrap();
        big.set_len(5_000_001).unwrap();
        drop(big);
        let o = search(&p, &json!({"pattern": "needle"}));
        assert_eq!(
            o.text,
            "ok.txt:1:needle\n(2 files skipped: binary, unreadable or over 5 MB)"
        );
    }

    #[test]
    fn summaries_never_carry_raw_control_characters() {
        let (_t, p) = setup();
        let o = search(&p, &json!({"pattern": "a\nb\u{202e}"}));
        assert_eq!(o.summary, "Searched for \"a?b?\"");
        let o = find_files(&p, &json!({"pattern": "x\ny"}));
        assert_eq!(o.summary, "Found 0 files for x?y");
        let o = search(&p, &json!({"pattern": "é".repeat(200)}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert!(o.summary.chars().count() < 100);
    }

    #[cfg(unix)]
    #[test]
    fn control_characters_in_names_are_replaced_in_output() {
        let (t, p) = setup();
        put(&t, "a\nforged.txt", "needle\n");
        let o = search(&p, &json!({"pattern": "needle"}));
        assert_eq!(o.text, "a?forged.txt:1:needle");
        let o = find_files(&p, &json!({"pattern": "*.txt"}));
        assert_eq!(o.text, "a?forged.txt");
    }
}
