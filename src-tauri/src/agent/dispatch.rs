//! One tool call in, one outcome out: the single place that decides whether
//! the user must be asked, asks, and runs the tool.
//!
//! Fail closed: an error reading the store, a refused or cancelled card, a
//! panic in a tool, a chat that is gone or whose folder changed, all end in
//! an `Error` or `Declined` outcome with nothing run and nothing written.
//!
//! Order for a change: plan (no disk writes) -> the chat's edits flag -> the
//! card built from that plan -> `apply` of exactly that plan, which checks
//! the path, the secret rule and the content again. Order for a command:
//! check the string -> `inspect` -> a saved rule that `allowed` accepts, or a
//! card holding the string -> run that same string.
//!
//! Every call emits one `ToolStarted` (the pending form, before any card) and
//! one `ToolFinished`, on every path, also when the call's future is dropped.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::approval::{Approvals, Unanswered};
use super::background::{Background, STOPPED_BEFORE};
use super::change::{self, ReadLog};
use super::paths::{Project, invisible};
use super::read::{self, clean, fail, ok, path_summary, required_str, summary_text};
use super::run::{self, Limits};
use super::shell::Shell;
use super::{Answer, Approval, ToolCall, ToolOutcome, ToolStatus, plan, rules, search};
use crate::engine::EngineEvent;
use crate::openrouter::tool_calls::parse_arguments;
use crate::skills::library::Library;
use crate::store::Store;

/// The tools, in the order the schema lists them.
const TOOL_NAMES: [&str; 10] = [
    "read_file",
    "list_dir",
    "find_files",
    "search",
    "edit_file",
    "write_file",
    "run_command",
    "update_plan",
    "command_output",
    "stop_command",
];
/// Longest command accepted, in characters.
const MAX_COMMAND_CHARS: usize = 20_000;
const BUSY: &str = "Another request is already waiting for the user's answer in this chat, so this one was not done.";
const HIDDEN_CHARACTERS: &str =
    "That command contains hidden or control characters and cannot be run.";
const UNFINISHED: &str = "That did not finish, so nothing was done.";
const RULE_NOT_SAVED: &str =
    "The rule could not be saved, so this command will be asked about again next time.";
const EDITS_NOT_SAVED: &str = "Edits could not be allowed for the rest of this chat (the choice could not be saved), so the next change will ask again.";
/// The result of a call whose card was ended because the chat's folder was
/// closed or changed, and of calls the loop skips for that reason.
pub const FOLDER_CHANGED: &str = "Not run: the project folder was closed or changed.";
const MOVED: &str = "This chat's project folder was closed or changed, so this was not done.";
const CHAT_GONE: &str = "This chat no longer exists, so this was not done.";
const NOT_MARKED: &str = "Could not record that this was starting, so it was not done.";
/// Settings key prefix: a command was once allowed in this folder.
const COMMANDS_SEEN: &str = "commands_seen:";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Each chat's `ReadLog`, in memory only.
#[derive(Default)]
pub struct ReadLogs(Mutex<HashMap<String, Arc<Mutex<ReadLog>>>>);

impl ReadLogs {
    pub fn for_chat(&self, chat_id: &str) -> Arc<Mutex<ReadLog>> {
        lock(&self.0)
            .entry(chat_id.to_string())
            .or_default()
            .clone()
    }

    /// Drops the chat's log (the chat was deleted or its folder changed).
    pub fn forget(&self, chat_id: &str) {
        lock(&self.0).remove(chat_id);
    }
}

/// What one call needs. App-wide parts and this turn's parts, by reference.
pub struct Ctx<'a> {
    pub chat_id: &'a str,
    pub project: &'a Project,
    pub shell: &'a Shell,
    pub store: &'a Store,
    pub library: &'a Arc<Library>,
    pub approvals: &'a Approvals,
    pub background: &'a Background,
    pub read_log: &'a Arc<Mutex<ReadLog>>,
    pub emit: &'a (dyn Fn(EngineEvent) + Send + Sync),
    pub cancel: &'a CancellationToken,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dispatched {
    pub outcome: ToolOutcome,
    /// What the user wrote instead of approving, to be added as their message.
    pub user_message: Option<String>,
}

fn plain(outcome: ToolOutcome) -> Dispatched {
    Dispatched {
        outcome,
        user_message: None,
    }
}

/// Emits `ToolStarted` once and `ToolFinished` once, in that order, and the
/// finish also when the call is dropped part way.
struct Pair<'a> {
    emit: &'a (dyn Fn(EngineEvent) + Send + Sync),
    call_id: &'a str,
    name: String,
    started: bool,
    finished: bool,
}

impl Pair<'_> {
    fn start(&mut self, summary: String) {
        if !self.started {
            self.started = true;
            (self.emit)(EngineEvent::ToolStarted {
                call_id: self.call_id.to_string(),
                name: self.name.clone(),
                summary,
            });
        }
    }

    fn finish(&mut self, outcome: &ToolOutcome) {
        self.start(outcome.summary.clone());
        if !self.finished {
            self.finished = true;
            (self.emit)(EngineEvent::ToolFinished {
                call_id: self.call_id.to_string(),
                status: outcome.status,
                summary: outcome.summary.clone(),
                detail: outcome.detail.clone(),
            });
        }
    }
}

impl Drop for Pair<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(&ToolOutcome {
                status: ToolStatus::NotRun,
                text: "The task stopped before this finished.".to_string(),
                summary: format!("Did not finish {}", self.name),
                detail: None,
            });
        }
    }
}

/// Runs file-system work off the async workers. `None` when it panicked.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

fn unfinished(summary: &str) -> Dispatched {
    plain(fail(UNFINISHED.to_string(), summary.to_string()))
}

fn busy(summary: String) -> Dispatched {
    plain(fail(BUSY.to_string(), summary))
}

/// The outcome of an answer that is not an approval. `what` completes
/// "Declined: …".
fn refused_by_user(answer: Answer, what: &str) -> Dispatched {
    let (note, user_message) = match answer {
        Answer::Decline { note } => (note, None),
        Answer::Message { text } if text.trim().is_empty() => (None, None),
        Answer::Message { text } => (Some(text.clone()), Some(text)),
        // Never offered for this card; treated as a decline.
        _ => (None, None),
    };
    let text = match note.filter(|n| !n.trim().is_empty()) {
        Some(n) => format!("The user declined and said: {n}"),
        None => "The user declined.".to_string(),
    };
    Dispatched {
        outcome: ToolOutcome {
            status: ToolStatus::Declined,
            text,
            summary: format!("Declined: {what}"),
            detail: None,
        },
        user_message,
    }
}

/// The folder as shown and as the key for its rules: the real path without
/// a Windows `\\?\` prefix.
fn folder_text(root: &Path) -> String {
    let s = root.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => rest.to_string(),
        _ => s.into_owned(),
    }
}

/// A stop of the turn (or the chat's card dropped) before the action: not
/// the user's decline. `what` completes "Not run: …".
fn stopped(what: &str) -> Dispatched {
    plain(ToolOutcome {
        status: ToolStatus::NotRun,
        text: "Not run: the task was stopped.".to_string(),
        summary: format!("Not run: {what}"),
        detail: None,
    })
}

/// Why an approved action may no longer happen.
enum NotCurrent {
    Stopped,
    Gone(&'static str),
}

impl NotCurrent {
    fn outcome(self, what: &str, summary: String) -> Dispatched {
        match self {
            NotCurrent::Stopped => stopped(what),
            NotCurrent::Gone(text) => plain(fail(text.to_string(), summary)),
        }
    }
}

/// `Ok` while the chat exists and its folder is still this project's;
/// otherwise the reason (`CHAT_GONE` or `MOVED`).
async fn place_current(ctx: &Ctx<'_>) -> Result<(), &'static str> {
    if !matches!(ctx.store.get_chat(ctx.chat_id), Ok(Some(_))) {
        return Err(CHAT_GONE);
    }
    let Ok(Some(folder)) = ctx.store.folder(ctx.chat_id) else {
        return Err(MOVED);
    };
    let root = ctx.project.root.clone();
    let same = blocking(move || std::fs::canonicalize(&folder).is_ok_and(|r| r == root))
        .await
        .unwrap_or(false);
    if same { Ok(()) } else { Err(MOVED) }
}

/// `Ok` while the turn runs, the chat exists and its folder is still this
/// project's. Checked before every tool, after every card and before
/// anything is run or written. The stop is checked again last, after every
/// await in here.
async fn still_current(ctx: &Ctx<'_>) -> Result<(), NotCurrent> {
    if ctx.cancel.is_cancelled() {
        return Err(NotCurrent::Stopped);
    }
    place_current(ctx).await.map_err(NotCurrent::Gone)?;
    if ctx.cancel.is_cancelled() {
        return Err(NotCurrent::Stopped);
    }
    Ok(())
}

/// A card ended without an answer: by a folder change (closing or changing
/// the folder also stops the turn, so the folder is checked first), or by
/// Stop. Nothing was done either way.
async fn ended(ctx: &Ctx<'_>, what: &str) -> Dispatched {
    match place_current(ctx).await {
        Err(MOVED) => plain(ToolOutcome {
            status: ToolStatus::NotRun,
            text: FOLDER_CHANGED.to_string(),
            summary: format!("Not run: {what}"),
            detail: None,
        }),
        _ => stopped(what),
    }
}

/// Records, just before a call acts, that it started: a result lost after
/// this point is reported as "may have run". Fails closed.
fn mark_started(ctx: &Ctx<'_>, call_id: &str, summary: &str) -> Result<(), Dispatched> {
    ctx.store
        .mark_call_started(ctx.chat_id, call_id)
        .map_err(|_| plain(fail(NOT_MARKED.to_string(), summary.to_string())))
}

async fn ask(ctx: &Ctx<'_>, card: Approval) -> Result<Answer, Unanswered> {
    ctx.approvals
        .ask(ctx.chat_id, card, ctx.emit, ctx.cancel)
        .await
}

pub async fn run_call(ctx: &Ctx<'_>, call: &ToolCall) -> Dispatched {
    let mut pair = Pair {
        emit: ctx.emit,
        call_id: &call.id,
        name: summary_text(&call.name),
        started: false,
        finished: false,
    };
    let d = route(ctx, call, &mut pair).await;
    pair.finish(&d.outcome);
    d
}

async fn route(ctx: &Ctx<'_>, call: &ToolCall, pair: &mut Pair<'_>) -> Dispatched {
    let name = call.name.as_str();
    if !TOOL_NAMES.contains(&name) {
        return plain(fail(
            format!(
                "There is no tool named {}. The tools are: {}.",
                summary_text(name),
                TOOL_NAMES.join(", ")
            ),
            format!("Unknown tool {}", summary_text(name)),
        ));
    }
    let args = match parse_arguments(&call.arguments) {
        Ok(v) => v,
        Err(reason) => {
            return plain(fail(
                format!("The arguments of {name} could not be read: {reason}."),
                format!("Could not read the arguments of {name}"),
            ));
        }
    };
    // Nothing is read, listed or written for a chat whose folder was closed
    // or changed, or for a stopped turn.
    if let Err(e) = still_current(ctx).await {
        return e.outcome(
            &summary_text(name),
            format!("Did not run {}", summary_text(name)),
        );
    }
    match name {
        "read_file" => read_file(ctx, args, pair).await,
        "list_dir" | "find_files" | "search" => look(ctx, name, args, pair).await,
        "edit_file" | "write_file" => change_file(ctx, name == "edit_file", args, pair).await,
        "run_command" => command(ctx, args, pair).await,
        "update_plan" => update_plan(ctx, args, pair),
        "command_output" => command_output(ctx, args, pair),
        _ => stop_command(ctx, args, pair).await,
    }
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// `list_dir`, `find_files`, `search`: never asked about.
async fn look(ctx: &Ctx<'_>, name: &str, args: Value, pair: &mut Pair<'_>) -> Dispatched {
    let pending = match name {
        "list_dir" => match str_arg(&args, "path") {
            None | Some("" | ".") => "Listing the project folder".to_string(),
            Some(p) => format!("Listing {}", path_summary(p)),
        },
        "find_files" => match str_arg(&args, "pattern") {
            Some(p) => format!("Finding {}", summary_text(p)),
            None => "Finding files".to_string(),
        },
        _ => match str_arg(&args, "pattern") {
            Some(p) => format!("Searching for \"{}\"", summary_text(p)),
            None => "Searching the project".to_string(),
        },
    };
    pair.start(pending.clone());
    let project = ctx.project.clone();
    let name = name.to_string();
    let out = blocking(move || match name.as_str() {
        "list_dir" => read::list_dir(&project, &args),
        "find_files" => search::find_files(&project, &args),
        _ => search::search(&project, &args),
    })
    .await;
    out.map(plain).unwrap_or_else(|| unfinished(&pending))
}

async fn read_file(ctx: &Ctx<'_>, args: Value, pair: &mut Pair<'_>) -> Dispatched {
    let pending = match str_arg(&args, "path") {
        Some(p) => format!("Reading {}", path_summary(p)),
        None => "Reading a file".to_string(),
    };
    pair.start(pending.clone());
    let (project, library, log) = (
        ctx.project.clone(),
        ctx.library.clone(),
        ctx.read_log.clone(),
    );
    let a = args.clone();
    let first = blocking(move || {
        let out = read::read_file(&project, &library, &a, false);
        // A read of a project file (a partial one too) lets an edit follow.
        if out.status == ToolStatus::Ok
            && let Some(path) = str_arg(&a, "path")
            && !path.starts_with("skill:")
            && let Ok(found) = project.existing(path)
        {
            lock(&log).note_read(&found.real);
        }
        out
    })
    .await;
    let Some(first) = first else {
        return unfinished(&pending);
    };
    let secret = first.status == ToolStatus::Error && first.text == "needs_approval";
    if !secret {
        return plain(first);
    }
    let Some(path) = str_arg(&args, "path").map(str::to_string) else {
        return unfinished(&pending);
    };
    let project = ctx.project.clone();
    let p = path.clone();
    // The file the card names, and its identity at card time.
    let resolved = blocking(move || {
        let found = project.existing(&p).ok()?;
        let id = read::file_id(&found.real)?;
        Some((found, id))
    })
    .await;
    let Some(Some((asked, file))) = resolved else {
        return plain(fail(
            "That file could not be read.".to_string(),
            first.summary,
        ));
    };
    let what = format!("read {}", path_summary(&asked.shown));
    let card = Approval::Secret {
        id: String::new(),
        path: clean(&asked.shown),
    };
    match ask(ctx, card).await {
        Err(Unanswered::Busy) => busy(first.summary),
        Ok(Answer::Once) => {
            let project = ctx.project.clone();
            let summary = first.summary.clone();
            let out = blocking(move || {
                const SWAPPED: &str =
                    "That path no longer leads to the file the user approved, so it was not read.";
                // The path must still lead to the file on the card, before
                // and after the read; otherwise nothing read is returned.
                let same = |p: &Project| p.existing(&path).is_ok_and(|f| f == asked);
                if !same(&project) {
                    return fail(SWAPPED.to_string(), summary);
                }
                // Exactly the approved file: opened once, no resolving again.
                let out = read::read_resolved(&asked, &file, &args);
                if !same(&project) {
                    return fail(SWAPPED.to_string(), summary);
                }
                out
            })
            .await;
            out.map(plain).unwrap_or_else(|| unfinished(&pending))
        }
        Ok(other) => refused_by_user(other, &what),
        Err(Unanswered::Stopped) => ended(ctx, &what).await,
    }
}

// ---------------------------------------------------------------------------
// Changing files
// ---------------------------------------------------------------------------

async fn change_file(ctx: &Ctx<'_>, edit: bool, args: Value, pair: &mut Pair<'_>) -> Dispatched {
    let (project, log) = (ctx.project.clone(), ctx.read_log.clone());
    let planned = blocking(move || {
        let log = lock(&log);
        if edit {
            change::plan_edit(&project, &log, &args)
        } else {
            change::plan_write(&project, &log, &args)
        }
    })
    .await;
    let generic = if edit {
        "Edited a file"
    } else {
        "Wrote a file"
    };
    let change = match planned {
        None => return unfinished(generic),
        // Refused before anything was asked (secret files among them).
        Some(Err(refusal)) => return plain(refusal),
        Some(Ok(c)) => c,
    };
    let shown = path_summary(&change.target.shown);
    let verb = if change.created { "create" } else { "edit" };
    let what = format!("{verb} {shown}");
    let summary = if change.created {
        "Created a file"
    } else {
        "Edited a file"
    };
    pair.start(format!("Wants to {what}"));

    let allowed = match ctx.store.edits_allowed(ctx.chat_id) {
        Ok(a) => a,
        Err(_) => {
            return plain(fail(
                "Could not check whether edits are allowed in this chat, so nothing was changed."
                    .to_string(),
                summary.to_string(),
            ));
        }
    };
    let mut note = None;
    if !allowed {
        let card = Approval::Edit {
            id: String::new(),
            path: clean(&change.target.shown),
            created: change.created,
            removed: change.removed.clone(),
            added: change.added.clone(),
            removed_total: change.removed_total,
            added_total: change.added_total,
            total_lines: change.total_lines,
        };
        match ask(ctx, card).await {
            Err(Unanswered::Busy) => return busy(summary.to_string()),
            Ok(Answer::Once) => {}
            Ok(Answer::AllEdits) => {
                if ctx.store.set_edits_allowed(ctx.chat_id, true).is_err() {
                    note = Some(EDITS_NOT_SAVED);
                }
            }
            Ok(other) => return refused_by_user(other, &what),
            Err(Unanswered::Stopped) => return ended(ctx, &what).await,
        }
    }
    if let Err(e) = still_current(ctx).await {
        return e.outcome(&what, summary.to_string());
    }
    if let Err(d) = mark_started(ctx, pair.call_id, summary) {
        return d;
    }
    let log = ctx.read_log.clone();
    // Exactly the change whose lines were on the card.
    let out = blocking(move || change::apply(&change, &mut lock(&log))).await;
    let Some(mut out) = out else {
        return unfinished(summary);
    };
    if let Some(n) = note {
        out.text = format!("{}\n{n}", out.text);
    }
    plain(out)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

struct CommandArgs {
    command: String,
    timeout: Option<i64>,
    background: bool,
}

fn command_args(args: &Value) -> Result<CommandArgs, String> {
    let command = required_str(args, "command")?.to_string();
    let timeout = match args.get("timeout_seconds") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_i64()
                .ok_or("The argument \"timeout_seconds\" must be a whole number.")?,
        ),
    };
    let background = match args.get("background") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("The argument \"background\" must be true or false.".to_string()),
    };
    if command.trim().is_empty() {
        return Err("The argument \"command\" must not be empty.".to_string());
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err("The command is longer than 20,000 characters, so it was not run.".to_string());
    }
    if command.contains('\0') {
        return Err("The command contains a NUL character, so it was not run.".to_string());
    }
    // What the card shows must be what the shell runs: no character that is
    // invisible, changes direction or moves the cursor.
    if command
        .chars()
        .any(|c| (c.is_control() && c != '\n' && c != '\t') || invisible(c))
    {
        return Err(HIDDEN_CHARACTERS.to_string());
    }
    Ok(CommandArgs {
        command,
        timeout,
        background,
    })
}

async fn command(ctx: &Ctx<'_>, args: Value, pair: &mut Pair<'_>) -> Dispatched {
    let CommandArgs {
        command,
        timeout,
        background,
    } = match command_args(&args) {
        Ok(c) => c,
        Err(e) => return plain(fail(e, "Ran a command".to_string())),
    };
    let shown = summary_text(&command);
    let not_run = format!("Did not run {shown}");
    pair.start(format!("Wants to run {shown}"));

    let root = &ctx.project.root;
    let folder = folder_text(root);
    let saved = match ctx.store.rules(&folder) {
        Ok(r) => r,
        Err(_) => {
            return plain(fail(
                "Could not read the saved command rules, so the command was not run.".to_string(),
                not_run,
            ));
        }
    };
    let family = ctx.shell.family;
    let mut note = None;
    if !rules::allowed(&command, family, root, &saved) {
        let inspected = rules::inspect(&command, family, root);
        // A rule already saved is not offered again: it does not match this form.
        let offer = inspected.offer.filter(|o| !saved.contains(o));
        let seen_key = format!("{COMMANDS_SEEN}{folder}");
        // When the store cannot say, show the warning.
        let seen = matches!(ctx.store.setting(&seen_key), Ok(Some(_)));
        let card = Approval::Command {
            id: String::new(),
            command: command.clone(),
            folder: folder.clone(),
            rule: offer.clone(),
            outside: inspected.outside,
            first_in_project: saved.is_empty() && !seen,
            background,
        };
        match ask(ctx, card).await {
            Err(Unanswered::Busy) => return busy(not_run),
            Ok(Answer::Once) => {}
            Ok(Answer::Always) => {
                // The user approved this command: it runs once even when the
                // rule cannot be saved, and the model is told.
                let saved_rule =
                    offer.is_some_and(|rule| ctx.store.add_rule(&folder, &rule).is_ok());
                if !saved_rule {
                    note = Some(RULE_NOT_SAVED);
                }
            }
            Ok(other) => return refused_by_user(other, &format!("run {shown}")),
            Err(Unanswered::Stopped) => return ended(ctx, &format!("run {shown}")).await,
        }
        let _ = ctx.store.set_setting(&seen_key, "1");
    }
    if let Err(e) = still_current(ctx).await {
        return e.outcome(&format!("run {shown}"), not_run);
    }
    if let Err(d) = mark_started(ctx, pair.call_id, &not_run) {
        return d;
    }
    let mut d = if background {
        start_background(ctx, &command, &shown).await
    } else {
        run_foreground(ctx, &command, &shown, timeout, pair.call_id).await
    };
    if let Some(n) = note {
        d.outcome.text = format!("{}\n{n}", d.outcome.text);
    }
    d
}

async fn run_foreground(
    ctx: &Ctx<'_>,
    command: &str,
    shown: &str,
    timeout: Option<i64>,
    call_id: &str,
) -> Dispatched {
    let limits = Limits {
        timeout: run::clamp_timeout(timeout),
        to_model: 12_000,
        kept: 200_000,
    };
    let emit = ctx.emit;
    let on_output = |text: &str| {
        emit(EngineEvent::ToolOutput {
            call_id: call_id.to_string(),
            text: text.to_string(),
        })
    };
    let ran = run::run(
        ctx.shell,
        &ctx.project.root,
        command,
        &limits,
        &on_output,
        ctx.cancel,
    )
    .await;
    let secs = ran.seconds.round() as u64;
    let how = if ran.timed_out {
        format!("timed out after {} s", limits.timeout.as_secs())
    } else if ran.stopped {
        "stopped".to_string()
    } else if let Some(code) = ran.exit_code {
        format!("exit {code}, {secs} s")
    } else {
        format!("no exit code, {secs} s")
    };
    let status = if ran.exit_code == Some(0) && !ran.timed_out && !ran.stopped {
        ToolStatus::Ok
    } else {
        ToolStatus::Error
    };
    plain(ToolOutcome {
        status,
        text: run::for_model(&ran, &limits),
        summary: format!("Ran {shown} ({how})"),
        detail: Some(ran.kept),
    })
}

async fn start_background(ctx: &Ctx<'_>, command: &str, shown: &str) -> Dispatched {
    let info = match ctx
        .background
        .start(
            ctx.chat_id,
            ctx.shell,
            &ctx.project.root,
            command,
            ctx.cancel,
        )
        .await
    {
        Ok(info) => info,
        Err(e) if e == STOPPED_BEFORE => return stopped(&format!("run {shown}")),
        Err(e) => return plain(fail(e, format!("Did not start {shown}"))),
    };
    let mut text = format!(
        "Started in the background as {}. Use command_output to read what it prints and stop_command to stop it.",
        info.id
    );
    if !info.running {
        let ended = match info.exit_code {
            Some(code) => format!("It has already ended with exit code {code}."),
            None => "It has already ended.".to_string(),
        };
        let output = ctx
            .background
            .output(ctx.chat_id, &info.id)
            .map(|r| r.new_text)
            .unwrap_or_default();
        let output = if output.is_empty() {
            "(no output)".to_string()
        } else {
            output
        };
        text.push_str(&format!("\n{ended}\noutput:\n{output}"));
    }
    plain(ok(
        text,
        format!("Started {shown} in the background ({})", info.id),
    ))
}

fn command_output(ctx: &Ctx<'_>, args: Value, pair: &mut Pair<'_>) -> Dispatched {
    let id = match required_str(&args, "id") {
        Ok(id) => id,
        Err(e) => return plain(fail(e, "Read a command's output".to_string())),
    };
    let summary = format!("Read the output of {}", summary_text(id));
    pair.start(format!("Reading the output of {}", summary_text(id)));
    match ctx.background.output(ctx.chat_id, id) {
        Ok(r) => {
            let status = if r.running {
                "still running".to_string()
            } else if let Some(code) = r.exit_code {
                format!("exited with code {code}")
            } else {
                "ended".to_string()
            };
            let body = if r.new_text.is_empty() {
                "(no new output)".to_string()
            } else {
                r.new_text
            };
            plain(ok(format!("status: {status}\noutput:\n{body}"), summary))
        }
        Err(e) => plain(fail(e, summary)),
    }
}

async fn stop_command(ctx: &Ctx<'_>, args: Value, pair: &mut Pair<'_>) -> Dispatched {
    let id = match required_str(&args, "id") {
        Ok(id) => id,
        Err(e) => return plain(fail(e, "Stopped a command".to_string())),
    };
    let summary = format!("Stopped {}", summary_text(id));
    pair.start(format!("Stopping {}", summary_text(id)));
    if let Err(d) = mark_started(ctx, pair.call_id, &summary) {
        return d;
    }
    match ctx.background.stop(ctx.chat_id, id).await {
        Ok(()) => plain(ok(format!("Stopped {id}."), summary)),
        Err(e) => plain(fail(e, summary)),
    }
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

fn update_plan(ctx: &Ctx<'_>, args: Value, pair: &mut Pair<'_>) -> Dispatched {
    pair.start("Updating the plan".to_string());
    let (out, items) = plan::update_plan(&args);
    if out.status != ToolStatus::Ok {
        return plain(out);
    }
    if let Err(d) = mark_started(ctx, pair.call_id, &out.summary) {
        return d;
    }
    let saved = serde_json::to_string(&items)
        .ok()
        .is_some_and(|json| ctx.store.set_plan(ctx.chat_id, &json).is_ok());
    if !saved {
        return plain(fail(
            "The plan could not be saved.".to_string(),
            out.summary,
        ));
    }
    (ctx.emit)(EngineEvent::PlanUpdated { items });
    plain(out)
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
