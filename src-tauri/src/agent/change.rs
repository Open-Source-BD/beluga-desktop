//! `edit_file` and `write_file`: plan a change without touching the disk,
//! then apply it safely.
//!
//! Planning validates the arguments and computes the new bytes. Applying
//! re-checks the path with `Project::for_write`, re-checks that the file is
//! still as it was read, creates missing folders one at a time without
//! following a link, writes a temporary file beside the target and renames it
//! into place. A folder swapped for a link in the instant between the last
//! check and the rename cannot be ruled out without `openat`-style calls the
//! standard library lacks.
//!
//! Text handling: files are UTF-8. A byte-order mark is kept. A file whose
//! every line ends in CRLF is edited as LF text and written back as CRLF; a
//! file with mixed or lone-CR endings is edited as it is, byte for byte.
//! `write_file` writes its content exactly as given.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};

use super::paths::{InProject, Project, UNUSABLE};
use super::read::{clean, fail, ok, path_summary, required_str};
use super::secrets::is_secret;
use super::{ToolOutcome, ToolStatus};

/// Largest file `edit_file` accepts, and largest content either tool writes.
const MAX_BYTES: u64 = 1_000_000;
const TOO_BIG: &str = "1,000,000 bytes";
/// Longest card line, in characters.
const MAX_LINE_CHARS: usize = 500;
/// Most changed lines per list.
const MAX_LIST: usize = 200;
/// Lines of a new file shown on the card.
const NEW_FILE_LINES: usize = 40;
/// Most candidate lines searched for the "closest lines" hint.
const MAX_CANDIDATES: usize = 5000;
const SECRET: &str = "That file usually holds secrets, so it cannot be changed here.";
const BOM: &[u8] = b"\xef\xbb\xbf";

/// What the log remembers about one file the model has read.
#[derive(Debug, Clone)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    /// Absent only for a file too big to edit; present otherwise, so a change
    /// that keeps both length and modified time is still noticed.
    hash: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadState {
    Unread,
    Stale,
    Fresh,
}

/// The files this chat's model has read (or written), by real path.
#[derive(Debug, Default)]
pub struct ReadLog {
    seen: HashMap<PathBuf, Stamp>,
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The whole file, or `None` when it cannot be read or is over the limit.
fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let f = fs::File::open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    f.take(MAX_BYTES + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= MAX_BYTES).then_some(buf)
}

impl ReadLog {
    /// Records the file's current length, modified time and content hash.
    pub fn note_read(&mut self, real: &Path) {
        // Metadata first: a change during the read then shows as stale.
        let Ok(meta) = fs::metadata(real) else {
            self.seen.remove(real);
            return;
        };
        let hash = if meta.len() <= MAX_BYTES {
            match read_limited(real) {
                Some(b) => Some(sha(&b)),
                None => {
                    self.seen.remove(real);
                    return;
                }
            }
        } else {
            None
        };
        self.seen.insert(
            real.to_path_buf(),
            Stamp {
                len: meta.len(),
                modified: meta.modified().ok(),
                hash,
            },
        );
    }

    /// Records a file this module just wrote, from the bytes written.
    fn note_written(&mut self, real: &Path, bytes: &[u8]) {
        match fs::metadata(real) {
            Ok(meta) => {
                self.seen.insert(
                    real.to_path_buf(),
                    Stamp {
                        len: bytes.len() as u64,
                        modified: meta.modified().ok(),
                        hash: Some(sha(bytes)),
                    },
                );
            }
            Err(_) => {
                self.seen.remove(real);
            }
        }
    }

    fn state(&self, real: &Path) -> ReadState {
        let Some(st) = self.seen.get(real) else {
            return ReadState::Unread;
        };
        let Ok(meta) = fs::metadata(real) else {
            return ReadState::Stale;
        };
        if meta.len() != st.len || meta.modified().ok() != st.modified {
            return ReadState::Stale;
        }
        match st.hash {
            Some(h) => match read_limited(real) {
                Some(b) if sha(&b) == h => ReadState::Fresh,
                _ => ReadState::Stale,
            },
            None => ReadState::Fresh,
        }
    }

    /// True when the file is as it was when last read or written here.
    pub fn fresh(&self, real: &Path) -> bool {
        self.state(real) == ReadState::Fresh
    }

    /// `Err(message)` unless the file is fresh.
    fn require_fresh(&self, real: &Path, shown: &str) -> Result<(), String> {
        match self.state(real) {
            ReadState::Fresh => Ok(()),
            ReadState::Unread => Err(format!("Read {shown} first.")),
            ReadState::Stale => Err(format!("{shown} changed since it was read. Read it again.")),
        }
    }
}

/// A validated change, not yet on disk.
#[derive(Debug, Clone)]
pub struct Change {
    pub project: Project,
    pub target: InProject,
    pub created: bool,
    pub new_bytes: Vec<u8>,
    /// Changed lines for the card: no line endings, each at most 500
    /// characters, each list at most 200 lines. For a created file `added`
    /// is its first 40 lines.
    pub removed: Vec<String>,
    pub added: Vec<String>,
    pub total_lines: u32,
    /// The true counts behind the (possibly cut) lists.
    pub removed_total: u32,
    pub added_total: u32,
}

fn refusal(text: impl Into<String>, created: bool) -> ToolOutcome {
    fail(
        text.into(),
        if created {
            "Created a file"
        } else {
            "Edited a file"
        }
        .to_string(),
    )
}

/// Everything that is the same for both tools: the path, the secret rule.
fn target_of(project: &Project, path: &str, created: bool) -> Result<InProject, ToolOutcome> {
    let target = project.for_write(path).map_err(|e| refusal(e, created))?;
    if is_secret(&target.shown) {
        return Err(refusal(SECRET, created));
    }
    Ok(target)
}

fn exists(real: &Path) -> bool {
    fs::symlink_metadata(real).is_ok()
}

struct Doc {
    bom: bool,
    /// Every line ended in CRLF; `text` is then in LF form.
    crlf: bool,
    text: String,
}

fn decode(bytes: &[u8]) -> Option<Doc> {
    if bytes.contains(&0) {
        return None;
    }
    let (bom, rest) = match bytes.strip_prefix(BOM) {
        Some(r) => (true, r),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(rest).ok()?;
    let lf = text.matches('\n').count();
    let crlf_n = text.matches("\r\n").count();
    let cr = text.matches('\r').count();
    if crlf_n > 0 && crlf_n == lf && cr == crlf_n {
        Some(Doc {
            bom,
            crlf: true,
            text: text.replace("\r\n", "\n"),
        })
    } else {
        Some(Doc {
            bom,
            crlf: false,
            text: text.to_string(),
        })
    }
}

fn encode(doc_text: &str, bom: bool, crlf: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(doc_text.len() + 3);
    if bom {
        out.extend_from_slice(BOM);
    }
    if crlf {
        out.extend_from_slice(doc_text.replace('\n', "\r\n").as_bytes());
    } else {
        out.extend_from_slice(doc_text.as_bytes());
    }
    out
}

/// Number of occurrences of `needle`, overlapping ones included.
fn count_overlapping(text: &str, needle: &str) -> usize {
    let mut n = 0;
    let mut from = 0;
    while let Some(i) = text[from..].find(needle) {
        n += 1;
        let at = from + i;
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    n
}

/// Byte ranges of `old` found line by line, ignoring trailing white space
/// (line endings included). `old` must cover whole lines.
fn loose_matches(text: &str, old: &str) -> Vec<(usize, usize)> {
    let ends_nl = old.ends_with('\n');
    let body = old.strip_suffix('\n').unwrap_or(old);
    let want: Vec<&str> = body.split('\n').map(str::trim_end).collect();
    // (start, content end without trailing white space, end with terminator)
    let mut lines = Vec::new();
    let mut start = 0;
    for piece in text.split_inclusive('\n') {
        let content = piece.strip_suffix('\n').unwrap_or(piece);
        let trimmed = content.trim_end();
        lines.push((start, start + trimmed.len(), start + piece.len(), trimmed));
        start += piece.len();
    }
    let mut found = Vec::new();
    if want.len() > lines.len() {
        return found;
    }
    for i in 0..=lines.len() - want.len() {
        if want.iter().enumerate().all(|(j, w)| lines[i + j].3 == *w) {
            let last = lines[i + want.len() - 1];
            found.push((lines[i].0, if ends_nl { last.2 } else { last.1 }));
        }
    }
    found
}

fn cut(line: &str) -> String {
    let line = line.trim_end_matches(['\n', '\r']);
    line.chars().take(MAX_LINE_CHARS).collect()
}

/// Removed and added lines between two texts: (removed, added, true counts).
fn diff_lines(old: &str, new: &str) -> (Vec<String>, Vec<String>, u32, u32) {
    let diff = TextDiff::from_lines(old, new);
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    let (mut rn, mut an) = (0u32, 0u32);
    for c in diff.iter_all_changes() {
        match c.tag() {
            ChangeTag::Delete => {
                rn += 1;
                if removed.len() < MAX_LIST {
                    removed.push(cut(c.value()));
                }
            }
            ChangeTag::Insert => {
                an += 1;
                if added.len() < MAX_LIST {
                    added.push(cut(c.value()));
                }
            }
            ChangeTag::Equal => {}
        }
    }
    (removed, added, rn, an)
}

fn line_count(text: &str) -> u32 {
    text.lines().count().min(u32::MAX as usize) as u32
}

/// Up to three lines of `text` most like the first non-blank line of `old`.
fn closest_lines(text: &str, old: &str) -> String {
    let Some(word) = old.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return String::new();
    };
    let word: String = word.chars().take(200).collect();
    let cands: Vec<(usize, String)> = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim().chars().take(200).collect::<String>()))
        .filter(|(_, l)| !l.is_empty())
        .take(MAX_CANDIDATES)
        .collect();
    let refs: Vec<&str> = cands.iter().map(|(_, l)| l.as_str()).collect();
    let mut used = vec![false; cands.len()];
    let mut out = String::new();
    for m in similar::get_close_matches::<str>(word.as_str(), &refs, 3, 0.3) {
        if let Some(i) = (0..cands.len()).find(|&i| !used[i] && cands[i].1 == m) {
            used[i] = true;
            out.push_str(&format!("\n{}: {}", cands[i].0, clean(m)));
        }
    }
    if out.is_empty() {
        out
    } else {
        format!("\nClosest lines in the file:{out}")
    }
}

fn replace_all_flag(args: &Value) -> Result<bool, String> {
    match args.get("replace_all") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err("The argument \"replace_all\" must be true or false.".to_string()),
    }
}

fn too_big_to_edit(shown: &str) -> String {
    format!("{shown} is larger than {TOO_BIG}, so it cannot be edited here.")
}

pub fn plan_edit(project: &Project, log: &ReadLog, args: &Value) -> Result<Change, ToolOutcome> {
    let args_err = |e: String| refusal(e, false);
    let path = required_str(args, "path").map_err(args_err)?;
    let old = required_str(args, "old_text").map_err(args_err)?;
    let new = required_str(args, "new_text").map_err(args_err)?;
    let all = replace_all_flag(args).map_err(args_err)?;
    if old.is_empty() {
        return Err(refusal(
            "The argument \"old_text\" must not be empty. Use write_file to create a file.",
            false,
        ));
    }
    if old == new {
        return Err(refusal(
            "old_text and new_text are the same, so nothing would change.",
            false,
        ));
    }
    let target = target_of(project, path, false)?;
    let shown = target.shown.clone();
    let meta = match fs::symlink_metadata(&target.real) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Err(refusal(UNUSABLE, false)),
        Err(_) => {
            return Err(refusal(
                format!("{shown} does not exist. Use write_file to create it."),
                false,
            ));
        }
    };
    if meta.len() > MAX_BYTES {
        return Err(refusal(too_big_to_edit(&shown), false));
    }
    log.require_fresh(&target.real, &shown)
        .map_err(|e| refusal(e, false))?;
    let bytes =
        read_limited(&target.real).ok_or_else(|| refusal(too_big_to_edit(&shown), false))?;
    let not_text = || {
        refusal(
            format!("{shown} is not UTF-8 text, so it cannot be edited here."),
            false,
        )
    };
    let doc = decode(&bytes).ok_or_else(not_text)?;

    let (old, new) = if doc.crlf {
        (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"))
    } else {
        (old.to_string(), new.to_string())
    };
    let n = count_overlapping(&doc.text, &old);
    let new_text = if n == 1 {
        doc.text.replacen(&old, &new, 1)
    } else if n > 1 {
        if !all {
            return Err(refusal(
                format!(
                    "old_text occurs {n} times in {shown}. Add more surrounding text or set replace_all."
                ),
                false,
            ));
        }
        doc.text.replace(&old, &new)
    } else {
        let loose = loose_matches(&doc.text, &old);
        match loose.as_slice() {
            [(s, e)] => format!("{}{}{}", &doc.text[..*s], new, &doc.text[*e..]),
            [] => {
                return Err(refusal(
                    format!(
                        "old_text was not found in {shown}.{}",
                        closest_lines(&doc.text, &old)
                    ),
                    false,
                ));
            }
            many => {
                return Err(refusal(
                    format!(
                        "old_text was not found exactly in {shown}; ignoring line endings and trailing spaces it occurs {} times. Add more surrounding text.",
                        many.len()
                    ),
                    false,
                ));
            }
        }
    };
    if new_text == doc.text {
        return Err(refusal("That edit would not change the file.", false));
    }
    let new_bytes = encode(&new_text, doc.bom, doc.crlf);
    if new_bytes.len() as u64 > MAX_BYTES {
        return Err(refusal(
            format!("The edited file would be larger than {TOO_BIG}, so it cannot be written."),
            false,
        ));
    }
    let (removed, added, removed_total, added_total) = diff_lines(&doc.text, &new_text);
    Ok(Change {
        project: project.clone(),
        target,
        created: false,
        new_bytes,
        removed,
        added,
        total_lines: line_count(&new_text),
        removed_total,
        added_total,
    })
}

pub fn plan_write(project: &Project, log: &ReadLog, args: &Value) -> Result<Change, ToolOutcome> {
    let args_err = |e: String| refusal(e, true);
    let path = required_str(args, "path").map_err(args_err)?;
    let content = required_str(args, "content").map_err(args_err)?;
    if content.len() as u64 > MAX_BYTES {
        return Err(refusal(
            format!("The content is larger than {TOO_BIG}, so it cannot be written."),
            true,
        ));
    }
    let target = target_of(project, path, true)?;
    let created = !exists(&target.real);
    let (removed, added, removed_total, added_total);
    if created {
        removed = Vec::new();
        added = content.lines().take(NEW_FILE_LINES).map(cut).collect();
        removed_total = 0;
        added_total = line_count(content);
    } else {
        log.require_fresh(&target.real, &target.shown)
            .map_err(|e| refusal(e, true))?;
        // For the card only; a file that cannot be read as text shows no
        // removed lines.
        let old = read_limited(&target.real)
            .map(|b| String::from_utf8_lossy(b.strip_prefix(BOM).unwrap_or(&b)).into_owned())
            .unwrap_or_default();
        (removed, added, removed_total, added_total) = diff_lines(&old, content);
    }
    Ok(Change {
        project: project.clone(),
        target,
        created,
        new_bytes: content.as_bytes().to_vec(),
        removed,
        added,
        total_lines: line_count(content),
        removed_total,
        added_total,
    })
}

/// Removes the temporary file unless it was moved into place.
struct TempGuard(PathBuf);

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// Creates the folders above `file` that do not exist, one at a time, each
/// checked to be a real directory (not a link) before going on.
fn make_parents(root: &Path, file: &Path) -> Result<(), String> {
    let rel = file
        .parent()
        .and_then(|p| p.strip_prefix(root).ok())
        .ok_or_else(|| UNUSABLE.to_string())?;
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur.push(comp);
        match fs::symlink_metadata(&cur) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(&cur) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("Could not create a folder: {e}")),
            },
            Err(_) => return Err(UNUSABLE.to_string()),
        }
        if !is_real_dir(&cur) {
            return Err(UNUSABLE.to_string());
        }
    }
    Ok(())
}

fn write_temp_and_place(change: &Change, real: &Path) -> Result<(), String> {
    let dir = real.parent().ok_or_else(|| UNUSABLE.to_string())?;
    let tmp = dir.join(format!(".beluga-{}.tmp", uuid::Uuid::new_v4().simple()));
    let io = |e: std::io::Error| format!("Could not write the file: {e}");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(io)?;
    let guard = TempGuard(tmp.clone());
    f.write_all(&change.new_bytes).map_err(io)?;
    f.flush().map_err(io)?;
    f.sync_all().map_err(io)?;
    drop(f);
    if change.created {
        // Never replace a file that appeared since the check.
        match fs::hard_link(&tmp, real) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!(
                    "{} already exists now. Read it first.",
                    change.target.shown
                ));
            }
            Err(_) => fs::rename(&tmp, real).map_err(io)?,
        }
    } else {
        #[cfg(unix)]
        {
            let perms = fs::metadata(real).map_err(io)?.permissions();
            fs::set_permissions(&tmp, perms).map_err(io)?;
        }
        fs::rename(&tmp, real).map_err(io)?;
    }
    drop(guard); // a no-op after a rename; removes the extra link otherwise
    Ok(())
}

pub fn apply(change: &Change, log: &mut ReadLog) -> ToolOutcome {
    let shown = &change.target.shown;
    let real = &change.target.real;
    let bad = |m: String| refusal(m, change.created);
    match change.project.for_write(shown) {
        Ok(t) if t.real == *real => {}
        Ok(_) => return bad(UNUSABLE.to_string()),
        Err(e) => return bad(e),
    }
    if is_secret(shown) {
        return bad(SECRET.to_string());
    }
    if change.created {
        if exists(real) {
            return bad(format!("{shown} already exists now. Read it first."));
        }
    } else if let Err(e) = log.require_fresh(real, shown) {
        return bad(e);
    }
    if let Err(e) = make_parents(&change.project.root, real) {
        return bad(e);
    }
    // Folders were just created: check the whole path once more.
    match change.project.for_write(shown) {
        Ok(t) if t.real == *real => {}
        Ok(_) => return bad(UNUSABLE.to_string()),
        Err(e) => return bad(e),
    }
    if let Err(e) = write_temp_and_place(change, real) {
        return bad(e);
    }
    log.note_written(real, &change.new_bytes);

    let mut detail = String::new();
    for l in &change.removed {
        detail.push_str(&format!("-{l}\n"));
    }
    if change.removed_total as usize > change.removed.len() {
        detail.push_str(&format!(
            "... {} more removed\n",
            change.removed_total as usize - change.removed.len()
        ));
    }
    for l in &change.added {
        detail.push_str(&format!("+{l}\n"));
    }
    if change.added_total as usize > change.added.len() {
        detail.push_str(&format!(
            "... {} more added\n",
            change.added_total as usize - change.added.len()
        ));
    }
    let (text, summary) = if change.created {
        (
            format!("Created {shown} ({} lines).", change.total_lines),
            format!("Created {}", path_summary(shown)),
        )
    } else {
        let (a, r) = (change.added_total, change.removed_total);
        (
            format!("Edited {shown} (+{a} \u{2212}{r})."),
            format!("Edited {} (+{a} \u{2212}{r})", path_summary(shown)),
        )
    };
    let mut out = ok(text, summary);
    out.status = ToolStatus::Ok;
    out.detail = Some(detail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolStatus;
    use serde_json::json;
    use std::fs;

    fn setup() -> (tempfile::TempDir, Project, ReadLog) {
        let d = tempfile::tempdir().unwrap();
        let p = Project::open(d.path().to_str().unwrap()).unwrap();
        (d, p, ReadLog::default())
    }

    /// Writes a file and records it as read.
    fn seed(d: &tempfile::TempDir, p: &Project, l: &mut ReadLog, name: &str, bytes: &[u8]) {
        let f = d.path().join(name);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, bytes).unwrap();
        l.note_read(&p.existing(name).unwrap().real);
    }

    fn edit(
        p: &Project,
        l: &ReadLog,
        path: &str,
        old: &str,
        new: &str,
    ) -> Result<Change, ToolOutcome> {
        plan_edit(
            p,
            l,
            &json!({"path": path, "old_text": old, "new_text": new}),
        )
    }

    fn err_text(r: Result<Change, ToolOutcome>) -> String {
        let o = r.expect_err("expected a refusal");
        assert_eq!(o.status, ToolStatus::Error);
        o.text
    }

    fn do_edit(
        d: &tempfile::TempDir,
        p: &Project,
        l: &mut ReadLog,
        name: &str,
        old: &str,
        new: &str,
    ) -> Vec<u8> {
        let c = edit(p, l, name, old, new).unwrap();
        let o = apply(&c, l);
        assert_eq!(o.status, ToolStatus::Ok, "{}", o.text);
        fs::read(d.path().join(name)).unwrap()
    }

    #[test]
    fn unique_match_is_replaced_and_the_rest_is_byte_identical() {
        let (d, p, mut l) = setup();
        seed(
            &d,
            &p,
            &mut l,
            "a.txt",
            b"one\ntwo\nthree\n\xc3\xa9 \t end\n",
        );
        let got = do_edit(&d, &p, &mut l, "a.txt", "two", "2\n2b");
        assert_eq!(got, b"one\n2\n2b\nthree\n\xc3\xa9 \t end\n");
    }

    #[test]
    fn summary_counts_and_detail() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"a\nb\nc\n");
        let c = edit(&p, &l, "a.txt", "b", "x\ny").unwrap();
        assert_eq!(
            (c.removed.clone(), c.added.clone()),
            (
                vec!["b".to_string()],
                vec!["x".to_string(), "y".to_string()]
            )
        );
        assert_eq!(c.total_lines, 4);
        let o = apply(&c, &mut l);
        assert_eq!(o.summary, "Edited a.txt (+2 \u{2212}1)");
        let detail = o.detail.unwrap();
        assert!(detail.contains("-b") && detail.contains("+x") && detail.contains("+y"));
    }

    #[test]
    fn not_found_names_the_closest_lines_and_several_matches_are_counted() {
        let (d, p, mut l) = setup();
        seed(
            &d,
            &p,
            &mut l,
            "a.txt",
            b"let value = 1;\nlet other = 2;\nlet value = 1;\n",
        );
        let t = err_text(edit(&p, &l, "a.txt", "let valve = 1;", "x"));
        assert!(t.starts_with("old_text was not found in a.txt."), "{t}");
        assert!(t.contains("let value = 1;"), "{t}");
        let t = err_text(edit(&p, &l, "a.txt", "let value = 1;", "x"));
        assert_eq!(
            t,
            "old_text occurs 2 times in a.txt. Add more surrounding text or set replace_all."
        );
        let c = plan_edit(
            &p,
            &l,
            &json!({"path":"a.txt","old_text":"let value = 1;","new_text":"V","replace_all":true}),
        )
        .unwrap();
        assert_eq!(apply(&c, &mut l).status, ToolStatus::Ok);
        assert_eq!(
            fs::read(d.path().join("a.txt")).unwrap(),
            b"V\nlet other = 2;\nV\n"
        );
    }

    #[test]
    fn replace_all_with_no_match_is_still_not_found() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"abc\n");
        let r = plan_edit(
            &p,
            &l,
            &json!({"path":"a.txt","old_text":"zzz","new_text":"y","replace_all":true}),
        );
        assert!(err_text(r).starts_with("old_text was not found in a.txt."));
    }

    #[test]
    fn overlapping_matches_count_as_several() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"aaa");
        assert!(err_text(edit(&p, &l, "a.txt", "aa", "b")).contains("occurs 2 times"));
    }

    #[test]
    fn empty_and_unchanged_old_text_are_errors() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"abc\n");
        assert!(err_text(edit(&p, &l, "a.txt", "", "x")).contains("old_text"));
        assert!(err_text(edit(&p, &l, "a.txt", "abc", "abc")).contains("same"));
        let r = plan_edit(&p, &l, &json!({"path":"a.txt","old_text":"abc"}));
        assert!(err_text(r).contains("new_text"));
    }

    #[test]
    fn unread_and_stale_files_are_refused() {
        let (d, p, mut l) = setup();
        fs::write(d.path().join("a.txt"), "abc\n").unwrap();
        assert_eq!(
            err_text(edit(&p, &l, "a.txt", "abc", "x")),
            "Read a.txt first."
        );
        l.note_read(&p.existing("a.txt").unwrap().real);
        fs::write(d.path().join("a.txt"), "abcd\n").unwrap();
        assert_eq!(
            err_text(edit(&p, &l, "a.txt", "abc", "x")),
            "a.txt changed since it was read. Read it again."
        );
    }

    #[test]
    fn a_same_length_change_with_the_same_mtime_is_still_stale() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"abc\n");
        let f = d.path().join("a.txt");
        let m = fs::metadata(&f).unwrap().modified().unwrap();
        fs::write(&f, "xyz\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(m)
            .unwrap();
        assert!(!l.fresh(&p.existing("a.txt").unwrap().real));
    }

    #[test]
    fn crlf_is_kept_bom_is_kept_and_no_final_newline_stays_none() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "c.txt", b"a\r\nb\r\nc\r\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "c.txt", "a\nb", "a\nB\nB2"),
            b"a\r\nB\r\nB2\r\nc\r\n"
        );
        seed(&d, &p, &mut l, "n.txt", b"a\nb");
        assert_eq!(do_edit(&d, &p, &mut l, "n.txt", "b", "c"), b"a\nc");
        seed(&d, &p, &mut l, "b.txt", b"\xef\xbb\xbfa\r\nb\r\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "b.txt", "b", "z"),
            b"\xef\xbb\xbfa\r\nz\r\n"
        );
    }

    #[test]
    fn mixed_line_endings_are_not_rewritten() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "m.txt", b"a\r\nb\nc\r\n");
        assert_eq!(do_edit(&d, &p, &mut l, "m.txt", "c", "d"), b"a\r\nb\nd\r\n");
    }

    #[test]
    fn trailing_space_differences_match_loosely_only_when_unique() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"foo\nbar\nbaz\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "a.txt", "foo  \nbar  ", "X"),
            b"X\nbaz\n"
        );
        seed(&d, &p, &mut l, "b.txt", b"foo\nbar\nfoo\nbar\n");
        let t = err_text(edit(&p, &l, "b.txt", "foo  \nbar  ", "X"));
        assert!(t.contains("2 times"), "{t}");
    }

    #[test]
    fn binary_invalid_utf8_and_large_files_are_refused_for_edit() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "zero.bin", b"ab\0cd");
        seed(&d, &p, &mut l, "bad.txt", b"ab\xffcd");
        seed(&d, &p, &mut l, "big.txt", &vec![b'a'; 1_000_001]);
        for n in ["zero.bin", "bad.txt", "big.txt"] {
            let t = err_text(edit(&p, &l, n, "a", "b"));
            assert!(t.contains("cannot be edited"), "{n}: {t}");
        }
    }

    #[test]
    fn an_edit_that_grows_past_the_limit_is_refused() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"abc");
        let big = "x".repeat(1_000_001);
        assert!(err_text(edit(&p, &l, "a.txt", "abc", &big)).contains("1,000,000"));
    }

    #[test]
    fn missing_file_and_secret_files_are_refused() {
        let (d, p, mut l) = setup();
        assert!(err_text(edit(&p, &l, "no.txt", "a", "b")).contains("write_file"));
        seed(&d, &p, &mut l, ".env", b"KEY=1\n");
        seed(&d, &p, &mut l, ".git/config", b"x\n");
        let msg = "That file usually holds secrets, so it cannot be changed here.";
        assert_eq!(err_text(edit(&p, &l, ".env", "1", "2")), msg);
        assert_eq!(err_text(edit(&p, &l, ".git/config", "x", "y")), msg);
        let r = plan_write(&p, &l, &json!({"path":".env","content":"a"}));
        assert_eq!(err_text(r), msg);
        let r = plan_write(&p, &l, &json!({"path":"sub/.git/hooks/x","content":"a"}));
        assert_eq!(err_text(r), msg);
        assert!(plan_write(&p, &l, &json!({"path":".env.example","content":"A=\n"})).is_ok());
    }

    #[test]
    fn refusal_summaries_are_fixed_lines() {
        let (_d, p, l) = setup();
        let o = plan_edit(
            &p,
            &l,
            &json!({"path":"../x\n\u{1b}[31m","old_text":"a","new_text":"b"}),
        )
        .unwrap_err();
        assert_eq!(o.summary, "Edited a file");
        let o = plan_write(&p, &l, &json!({"path":"../x","content":"b"})).unwrap_err();
        assert_eq!(o.summary, "Created a file");
    }

    #[test]
    fn write_creates_files_and_folders_and_refuses_unread_replacement() {
        let (d, p, mut l) = setup();
        let c = plan_write(&p, &l, &json!({"path":"a/b/new.txt","content":"hi\n"})).unwrap();
        assert!(c.created);
        assert!(
            !d.path().join("a").exists(),
            "planning must not touch the disk"
        );
        let o = apply(&c, &mut l);
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.summary, "Created a/b/new.txt");
        assert_eq!(fs::read(d.path().join("a/b/new.txt")).unwrap(), b"hi\n");
        // It counts as read, so it can be replaced now.
        let c = plan_write(&p, &l, &json!({"path":"a/b/new.txt","content":"yo\n"})).unwrap();
        assert!(!c.created);
        let o = apply(&c, &mut l);
        assert_eq!(o.summary, "Edited a/b/new.txt (+1 \u{2212}1)");
        // An existing file that was never read is refused.
        fs::write(d.path().join("old.txt"), "x").unwrap();
        let r = plan_write(&p, &l, &json!({"path":"old.txt","content":"y"}));
        assert_eq!(err_text(r), "Read old.txt first.");
        assert_eq!(fs::read(d.path().join("old.txt")).unwrap(), b"x");
    }

    #[test]
    fn write_outside_onto_a_directory_or_through_a_link_is_refused() {
        let (d, p, l) = setup();
        fs::create_dir(d.path().join("dir")).unwrap();
        for path in ["../x.txt", "dir"] {
            let r = plan_write(&p, &l, &json!({"path":path,"content":"y"}));
            assert!(err_text(r).starts_with("That path"), "{path}");
        }
        #[cfg(unix)]
        {
            let out = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(out.path(), d.path().join("lnk")).unwrap();
            let r = plan_write(&p, &l, &json!({"path":"lnk/x.txt","content":"y"}));
            assert!(err_text(r).starts_with("That path"));
            assert!(fs::read_dir(out.path()).unwrap().next().is_none());
        }
        assert_eq!(
            fs::read_dir(d.path()).unwrap().count(),
            if cfg!(unix) { 2 } else { 1 }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_became_a_link_between_plan_and_apply_is_refused() {
        let (d, p, mut l) = setup();
        let out = tempfile::tempdir().unwrap();
        fs::write(out.path().join("target"), "outside").unwrap();
        seed(&d, &p, &mut l, "a.txt", b"abc\n");
        let c = edit(&p, &l, "a.txt", "abc", "x").unwrap();
        fs::remove_file(d.path().join("a.txt")).unwrap();
        std::os::unix::fs::symlink(out.path().join("target"), d.path().join("a.txt")).unwrap();
        let o = apply(&c, &mut l);
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(fs::read(out.path().join("target")).unwrap(), b"outside");
        // A new file whose folder became a link.
        let c = plan_write(&p, &l, &json!({"path":"sub/n.txt","content":"y"})).unwrap();
        std::os::unix::fs::symlink(out.path(), d.path().join("sub")).unwrap();
        assert_eq!(apply(&c, &mut l).status, ToolStatus::Error);
        assert!(!out.path().join("n.txt").exists());
    }

    #[test]
    fn apply_refuses_when_the_file_changed_or_appeared_after_planning() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"abc\n");
        let c = edit(&p, &l, "a.txt", "abc", "x").unwrap();
        fs::write(d.path().join("a.txt"), "other content\n").unwrap();
        let o = apply(&c, &mut l);
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("changed since it was read"));
        assert_eq!(
            fs::read(d.path().join("a.txt")).unwrap(),
            b"other content\n"
        );
        let c = plan_write(&p, &l, &json!({"path":"new.txt","content":"y"})).unwrap();
        fs::write(d.path().join("new.txt"), "mine").unwrap();
        assert_eq!(apply(&c, &mut l).status, ToolStatus::Error);
        assert_eq!(fs::read(d.path().join("new.txt")).unwrap(), b"mine");
    }

    #[test]
    fn after_apply_the_file_counts_as_read() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"one two\n");
        do_edit(&d, &p, &mut l, "a.txt", "one", "1");
        assert_eq!(do_edit(&d, &p, &mut l, "a.txt", "two", "2"), b"1 2\n");
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"abc\n");
        do_edit(&d, &p, &mut l, "a.txt", "abc", "x");
        let c = plan_write(&p, &l, &json!({"path":"n.txt","content":"y"})).unwrap();
        apply(&c, &mut l);
        let c = edit(&p, &l, "a.txt", "x", "z").unwrap();
        fs::write(d.path().join("a.txt"), "zzzzzz").unwrap();
        apply(&c, &mut l); // refused
        let mut names: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["a.txt", "n.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn permission_bits_are_preserved_on_replace() {
        use std::os::unix::fs::PermissionsExt;
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "run.sh", b"#!/bin/sh\necho a\n");
        fs::set_permissions(d.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        l.note_read(&p.existing("run.sh").unwrap().real);
        do_edit(&d, &p, &mut l, "run.sh", "echo a", "echo b");
        let mode = fs::metadata(d.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn a_new_100_line_file_shows_40_lines_and_counts_100() {
        let (_d, p, l) = setup();
        let content: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let c = plan_write(&p, &l, &json!({"path":"n.txt","content":content})).unwrap();
        assert_eq!(c.added.len(), 40);
        assert_eq!(c.total_lines, 100);
        assert!(c.removed.is_empty());
        assert_eq!(c.added[0], "line 1");
    }

    #[test]
    fn card_lines_are_cut_to_500_characters_and_200_lines() {
        let (d, p, mut l) = setup();
        let old: String = (0..300).map(|i| format!("old {i}\n")).collect();
        seed(&d, &p, &mut l, "a.txt", old.as_bytes());
        let long = "y".repeat(900);
        let new: String = (0..300).map(|_| format!("{long}\n")).collect();
        let c = plan_write(&p, &l, &json!({"path":"a.txt","content":new})).unwrap();
        assert_eq!(c.removed.len(), 200);
        assert_eq!(c.added.len(), 200);
        assert_eq!(c.added[0].chars().count(), 500);
        assert_eq!((c.removed_total, c.added_total), (300, 300));
        let o = apply(&c, &mut l);
        assert_eq!(o.summary, "Edited a.txt (+300 \u{2212}300)");
    }

    #[test]
    fn content_over_the_limit_is_refused_and_the_limit_itself_is_allowed() {
        let (_d, p, l) = setup();
        let r = plan_write(
            &p,
            &l,
            &json!({"path":"n.txt","content":"a".repeat(1_000_001)}),
        );
        assert!(err_text(r).contains("1,000,000"));
        assert!(
            plan_write(
                &p,
                &l,
                &json!({"path":"n.txt","content":"a".repeat(1_000_000)})
            )
            .is_ok()
        );
    }

    #[test]
    fn bad_arguments_name_the_argument() {
        let (_d, p, l) = setup();
        assert!(err_text(plan_write(&p, &l, &json!({"path":"a"}))).contains("content"));
        assert!(
            err_text(plan_edit(&p, &l, &json!({"old_text":"a","new_text":"b"}))).contains("path")
        );
        let r = plan_edit(
            &p,
            &l,
            &json!({"path":"a","old_text":"a","new_text":"b","replace_all":"yes"}),
        );
        assert!(err_text(r).contains("replace_all"));
    }
}
