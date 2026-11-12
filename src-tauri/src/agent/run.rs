//! Run one shell command in the project folder: capture its output with
//! bounded memory, enforce a time limit, and stop the whole process tree on
//! timeout or cancel.
//!
//! Output is read from both pipes at once and merged in arrival order (the
//! order between the two streams is not exact). Only the first quarter and the
//! last part of what fits in `Limits::kept` is held; the pipes are still read
//! to the end so the command never blocks on a full pipe.
//!
//! Stopping reaches everything the command started in its own process group.
//! On macOS and Linux the command runs in a new session; a process that
//! deliberately moves itself into another session (`setsid`, daemonizing) is
//! NOT reached by the stop. On Windows the command runs in a Job Object, which
//! does reach such processes.

use std::path::Path;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

#[cfg(unix)]
use process_wrap::tokio::ProcessSession;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
#[cfg(windows)]
use process_wrap::tokio::{CreationFlags, JobObject};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::paths::invisible;
use super::shell::Shell;

/// Longest a command may run when the model gives no time limit.
const DEFAULT_TIMEOUT_SECS: i64 = 120;
const MAX_TIMEOUT_SECS: i64 = 600;
/// Room kept free in `kept` for the "left out" notice once it is cut.
const NOTICE_ROOM: usize = 64;
/// `on_output` is called at most this often...
const FLUSH_EVERY: Duration = Duration::from_millis(100);
/// ...with at most this many characters (skipped-notice line included).
const FLUSH_CHARS: usize = 4_000;
/// How long output is still collected after the child exited, waiting for the
/// pipes to close, before the rest of its tree is stopped.
const EXIT_GRACE: Duration = Duration::from_secs(1);
/// How long output is still collected after the tree was stopped.
const DRAIN_AFTER_STOP: Duration = Duration::from_millis(300);
/// On unix, how long the tree gets to end after SIGTERM before SIGKILL.
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_millis(500);
/// Longest escape sequence given up on as a sequence (so an unterminated one
/// cannot swallow the rest of the output).
const MAX_SEQUENCE: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeout: Duration,
    /// Characters of output sent to the model.
    pub to_model: usize,
    /// Characters of output kept for the user.
    pub kept: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: clamp_timeout(None),
            to_model: 12_000,
            kept: 200_000,
        }
    }
}

/// The time limit for a model-given `timeout_seconds`: 1 to 600, default 120.
pub fn clamp_timeout(requested: Option<i64>) -> Duration {
    let secs = requested
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    Duration::from_secs(secs as u64)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ran {
    /// The command's own code. `None` when it was stopped, died from a signal
    /// or never started.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// Stopped because the user cancelled.
    pub stopped: bool,
    /// The output (or a plain message), cut in the middle when it was long.
    pub kept: String,
    /// All the cleaned output characters the command produced, including
    /// those `kept` left out.
    pub total_chars: u64,
    pub seconds: f32,
}

// ---------------------------------------------------------------------------
// Turning bytes into text
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Esc {
    #[default]
    Text,
    /// After ESC.
    Start,
    /// Inside `ESC [ ...`.
    Csi,
    /// Inside a string: `ESC ]` (OSC), `ESC P` (DCS), `ESC _` (APC), `ESC ^`
    /// (PM) or `ESC X` (SOS), up to BEL or ST.
    Str,
    /// Inside such a string, after ESC (the start of ST).
    Terminating,
    /// After `ESC (` and its kin: one more character belongs to it.
    Charset,
}

/// Decodes one pipe's bytes into plain text: lossy UTF-8 that never splits a
/// character across chunks, no ANSI escape sequences, no control characters
/// except newline and tab, and every line ending as `\n`.
#[derive(Default)]
struct Cleaner {
    /// An incomplete character at the end of the last chunk.
    carry: Vec<u8>,
    esc: Esc,
    in_sequence: usize,
    after_cr: bool,
}

impl Cleaner {
    fn feed(&mut self, bytes: &[u8]) -> String {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(bytes);
        let mut out = String::with_capacity(buf.len());
        let mut rest: &[u8] = &buf;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.push_text(text, &mut out);
                    break;
                }
                Err(err) => {
                    let (good, bad) = rest.split_at(err.valid_up_to());
                    // The prefix was just checked to be valid.
                    self.push_text(std::str::from_utf8(good).unwrap_or(""), &mut out);
                    match err.error_len() {
                        Some(skip) => {
                            self.push_char('\u{FFFD}', &mut out);
                            rest = &bad[skip..];
                        }
                        None => {
                            self.carry = bad.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// The end of the stream: a character cut off by it is one replacement.
    fn finish(&mut self) -> String {
        if self.carry.is_empty() {
            String::new()
        } else {
            self.carry.clear();
            "\u{FFFD}".to_string()
        }
    }

    fn push_text(&mut self, text: &str, out: &mut String) {
        for c in text.chars() {
            self.push_char(c, out);
        }
    }

    fn push_char(&mut self, c: char, out: &mut String) {
        match self.esc {
            Esc::Text => {}
            Esc::Start => {
                self.in_sequence = 0;
                self.esc = match c {
                    '[' => Esc::Csi,
                    ']' | 'P' | '_' | '^' | 'X' => Esc::Str,
                    '(' | ')' | '*' | '+' => Esc::Charset,
                    _ => Esc::Text,
                };
                return;
            }
            Esc::Csi => {
                self.in_sequence += 1;
                if ('\u{40}'..='\u{7e}').contains(&c) || self.in_sequence > MAX_SEQUENCE {
                    self.esc = Esc::Text;
                }
                return;
            }
            Esc::Str => {
                self.in_sequence += 1;
                // BEL, or U+009C (the 8-bit ST), ends it.
                if c == '\u{7}' || c == '\u{9c}' || self.in_sequence > MAX_SEQUENCE {
                    self.esc = Esc::Text;
                } else if c == '\u{1b}' {
                    self.esc = Esc::Terminating;
                }
                return;
            }
            Esc::Terminating => {
                if c == '\\' {
                    self.esc = Esc::Text;
                } else {
                    // Not a terminator: ESC began another sequence.
                    self.esc = Esc::Start;
                    self.push_char(c, out);
                }
                return;
            }
            Esc::Charset => {
                self.esc = Esc::Text;
                return;
            }
        }
        if std::mem::take(&mut self.after_cr) && c == '\n' {
            return;
        }
        match c {
            '\u{1b}' => self.esc = Esc::Start,
            // The 8-bit forms: CSI, and DCS, SOS, OSC, PM, APC.
            '\u{9b}' => {
                self.esc = Esc::Csi;
                self.in_sequence = 0;
            }
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => {
                self.esc = Esc::Str;
                self.in_sequence = 0;
            }
            '\r' => {
                out.push('\n');
                self.after_cr = true;
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            // Invisible or direction-changing: shown as a visible mark.
            c if invisible(c) => out.push('?'),
            c => out.push(c),
        }
    }
}

/// `"\n[… {n} characters left out …]\n"`
fn left_out(n: u64) -> String {
    format!("\n[… {n} characters left out …]\n")
}

/// The first part and the last part of an unbounded stream of text. Up to
/// `kept` characters are held before it is known whether the output must be
/// cut, so output that fits is never cut.
struct Collector {
    head: String,
    head_chars: usize,
    head_cap: usize,
    tail: String,
    tail_chars: usize,
    /// How much the tail holds while the output might still fit whole.
    tail_hold: usize,
    /// How much of the tail remains once the output is cut (room is left for
    /// the notice).
    tail_keep: usize,
    total: u64,
}

impl Collector {
    fn new(kept: usize) -> Self {
        let head_cap = kept / 4;
        Self {
            head: String::new(),
            head_chars: 0,
            head_cap,
            tail: String::new(),
            tail_chars: 0,
            tail_hold: kept - head_cap,
            tail_keep: kept.saturating_sub(head_cap + NOTICE_ROOM),
            total: 0,
        }
    }

    fn push(&mut self, text: &str) {
        let n = text.chars().count();
        self.total += n as u64;
        let mut rest = text;
        if self.head_chars < self.head_cap {
            let room = self.head_cap - self.head_chars;
            let at = text.char_indices().nth(room).map_or(text.len(), |(i, _)| i);
            self.head.push_str(&text[..at]);
            self.head_chars += n.min(room);
            rest = &text[at..];
        }
        if rest.is_empty() {
            return;
        }
        self.tail.push_str(rest);
        self.tail_chars += rest.chars().count();
        self.trim_tail(self.tail_hold);
    }

    fn trim_tail(&mut self, keep: usize) {
        if self.tail_chars > keep {
            let excess = self.tail_chars - keep;
            let at = self
                .tail
                .char_indices()
                .nth(excess)
                .map_or(self.tail.len(), |(i, _)| i);
            self.tail.drain(..at);
            self.tail_chars = keep;
        }
    }

    /// Adds `line` on a line of its own.
    #[cfg(unix)]
    fn push_line(&mut self, line: &str) {
        let last = self
            .tail
            .chars()
            .last()
            .or_else(|| self.head.chars().last());
        if last.is_some_and(|c| c != '\n') {
            self.push("\n");
        }
        self.push(line);
        self.push("\n");
    }

    /// The kept text and the number of characters the command produced.
    fn finish(mut self) -> (String, u64) {
        let total = self.total;
        if total > (self.head_chars + self.tail_chars) as u64 {
            self.trim_tail(self.tail_keep);
            let left = total - (self.head_chars + self.tail_chars) as u64;
            self.head.push_str(&left_out(left));
        }
        self.head.push_str(&self.tail);
        (self.head, total)
    }
}

/// Hands output to `on_output` at a bounded rate: at most one call per
/// 100 ms, each with at most 4,000 characters. When more arrived than fits,
/// the newest characters are passed on, after a line saying how many were
/// skipped.
struct Relay {
    /// The newest output not yet passed on (never more than the cap).
    pending: String,
    chars: usize,
    skipped: u64,
    since: Option<Instant>,
    last_call: Option<Instant>,
}

impl Relay {
    fn new() -> Self {
        Self {
            pending: String::new(),
            chars: 0,
            skipped: 0,
            since: None,
            last_call: None,
        }
    }

    /// When the next call may be made, if there is anything to pass on.
    fn due(&self) -> Option<Instant> {
        if self.pending.is_empty() {
            return None;
        }
        let from = self.last_call.or(self.since)?;
        Some(from + FLUSH_EVERY)
    }

    /// Drops the oldest pending characters until at most `keep` remain.
    fn drop_oldest(&mut self, keep: usize) {
        if self.chars > keep {
            let excess = self.chars - keep;
            let at = self
                .pending
                .char_indices()
                .nth(excess)
                .map_or(self.pending.len(), |(i, _)| i);
            self.pending.drain(..at);
            self.chars = keep;
            self.skipped += excess as u64;
        }
    }

    fn add(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.pending.push_str(text);
        self.chars += text.chars().count();
        self.since.get_or_insert_with(Instant::now);
        self.drop_oldest(FLUSH_CHARS);
    }

    /// Makes the call now (the rate limit is the caller's business).
    fn flush(&mut self, on_output: &(dyn Fn(&str) + Send + Sync)) {
        if self.pending.is_empty() {
            return;
        }
        let mut notice = String::new();
        while self.skipped > 0 {
            notice = format!("[… {} characters skipped …]\n", self.skipped);
            let room = FLUSH_CHARS.saturating_sub(notice.chars().count());
            if self.chars <= room {
                break;
            }
            self.drop_oldest(room);
        }
        on_output(&format!("{notice}{}", self.pending));
        self.pending.clear();
        self.chars = 0;
        self.skipped = 0;
        self.since = None;
        self.last_call = Some(Instant::now());
    }
}

/// Reads one pipe to its end (or until the receiver is gone).
async fn pump<R: AsyncRead + Unpin>(mut pipe: R, tx: mpsc::Sender<String>) {
    let mut cleaner = Cleaner::default();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let text = cleaner.feed(&buf[..n]);
                if !text.is_empty() && tx.send(text).await.is_err() {
                    return;
                }
            }
        }
    }
    let rest = cleaner.finish();
    if !rest.is_empty() {
        let _ = tx.send(rest).await;
    }
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

fn not_run(started: Instant, message: String, stopped: bool) -> Ran {
    Ran {
        exit_code: None,
        timed_out: false,
        stopped,
        total_chars: message.chars().count() as u64,
        kept: message,
        seconds: started.elapsed().as_secs_f32(),
    }
}

/// Kills the command's process group if the run is dropped half-way (the
/// task is aborted, or a panic unwinds). Unix only: on Windows the Job Object
/// is closed with the child (`KillOnDrop`), which ends everything in it.
///
/// How each path is covered, and why no stale pid is ever signalled:
/// - Only the group is signalled, never the leader's pid alone. A group id
///   cannot be reused while any member is alive; if none is alive the signal
///   fails with ESRCH, which is harmless.
/// - The leader is reaped only through tokio's own `Child` (never through
///   process-wrap's group `wait`, which reaps behind tokio's back), so tokio
///   knows when it has ended and its kill-on-drop does nothing afterwards.
///   Its exit is first only observed (`leader_ended`, `WNOWAIT`), so it stays
///   a zombie and its pid and group id stay its own until the group has been
///   signalled; only then is it reaped.
/// - Normal exit, timeout, cancel: `stop_tree` sends the final SIGKILL to the
///   group and only then disarms this guard (after a SIGTERM grace the guard
///   stays armed throughout), so nothing is signalled again on drop.
/// - Dropped or panicking mid-run, at any point before that SIGKILL: the
///   guard is still armed and kills the group; the leader has not been
///   reaped, so the group id is still valid.
#[cfg(unix)]
struct GroupGuard(Option<i32>);

#[cfg(unix)]
impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(group) = self.0.take() {
            kill_group(group, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
fn kill_group(group: i32, signal: i32) {
    // SAFETY: `killpg` only sends a signal; any failure (the group is gone) is
    // ignored on purpose.
    unsafe {
        libc::killpg(group, signal);
    }
}

/// Has the command's own process ended? On unix this only looks
/// (`waitid` with `WNOWAIT`): the leader stays a zombie, so its pid and its
/// group id cannot be reused until `stop_tree` has signalled the group and
/// tokio's child has reaped it. On Windows the exit status is taken here.
#[cfg(unix)]
fn leader_ended(child: &mut dyn ChildWrapper, _status: &mut Option<ExitStatus>) -> bool {
    let Some(pid) = child.id() else {
        // Already reaped (not expected): nothing left to wait for.
        return true;
    };
    // SAFETY: `siginfo_t` is plain data that `waitid` fills in; zeroed it
    // means "nothing to report".
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let flags = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
        if libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) != 0 {
            return true;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let reported = info.si_pid();
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let reported = info.si_pid;
        reported != 0
    }
}

#[cfg(windows)]
fn leader_ended(child: &mut dyn ChildWrapper, status: &mut Option<ExitStatus>) -> bool {
    match child.inner_mut().try_wait() {
        Ok(Some(st)) => {
            *status = Some(st);
            true
        }
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Ends everything the command started and returns the leader's status when
/// it could be collected (unix). With `graceful`: SIGTERM to the group, up to
/// 500 ms for the leader to end, then SIGKILL to the group. The guard stays
/// armed until the SIGKILL has been sent, so a drop at any moment before that
/// still kills the group; the leader is reaped (through tokio) only after the
/// group was signalled.
#[cfg(unix)]
async fn stop_tree(
    child: &mut dyn ChildWrapper,
    group: &mut GroupGuard,
    graceful: bool,
) -> Option<ExitStatus> {
    let id = group.0?;
    if graceful {
        kill_group(id, libc::SIGTERM);
        let until = Instant::now() + TERM_GRACE;
        let mut ignored = None;
        while Instant::now() < until && !leader_ended(child, &mut ignored) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    kill_group(id, libc::SIGKILL);
    group.0 = None;
    // Reap the leader through tokio, so its drop does not signal the pid.
    tokio::time::timeout(Duration::from_millis(300), child.inner_mut().wait())
        .await
        .ok()
        .and_then(Result::ok)
}

/// Windows: terminate the Job Object.
#[cfg(windows)]
async fn stop_tree(child: &mut dyn ChildWrapper, _graceful: bool) -> Option<ExitStatus> {
    let _ = child.start_kill();
    None
}

enum End {
    Exited,
    TimedOut,
    Cancelled,
}

/// Runs `command` in `project_root` with the given shell. Returns when it
/// ends, when `limits.timeout` passes, or when `cancel` fires; in the last two
/// cases the whole process tree is stopped first. `on_output` gets the text as
/// it arrives (see `Relay` for the rate) and is never called after this
/// returns.
pub async fn run(
    shell: &Shell,
    project_root: &Path,
    command: &str,
    limits: &Limits,
    on_output: &(dyn Fn(&str) + Send + Sync),
    cancel: &CancellationToken,
) -> Ran {
    let started = Instant::now();
    if cancel.is_cancelled() {
        return not_run(started, String::new(), true);
    }
    if !project_root.is_dir() {
        return not_run(
            started,
            format!(
                "The project folder {} does not exist, so the command was not run.",
                project_root.display()
            ),
            false,
        );
    }

    let mut cmd = shell.command(command, std::env::vars_os());
    cmd.current_dir(project_root);
    let mut wrap = CommandWrap::from(cmd);
    wrap.wrap(KillOnDrop);
    #[cfg(unix)]
    wrap.wrap(ProcessSession);
    #[cfg(windows)]
    {
        wrap.wrap(CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        wrap.wrap(JobObject);
    }
    let mut child = match wrap.spawn() {
        Ok(child) => child,
        Err(err) => {
            return not_run(
                started,
                format!("Could not start {}: {err}", shell.program.display()),
                false,
            );
        }
    };
    // The leader of its own session and group: the group id is its pid.
    #[cfg(unix)]
    let mut group = GroupGuard(child.id().and_then(|pid| i32::try_from(pid).ok()));

    let (tx, mut rx) = mpsc::channel::<String>(32);
    let mut readers = Vec::new();
    if let Some(pipe) = child.stdout().take() {
        readers.push(tokio::spawn(pump(pipe, tx.clone())));
    }
    if let Some(pipe) = child.stderr().take() {
        readers.push(tokio::spawn(pump(pipe, tx.clone())));
    }
    drop(tx);

    let mut collector = Collector::new(limits.kept);
    let mut relay = Relay::new();
    let timeout = tokio::time::sleep(limits.timeout);
    tokio::pin!(timeout);
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut status: Option<ExitStatus> = None;
    let mut exited_at: Option<Instant> = None;
    let mut closed = false;

    let end = loop {
        if let Some(at) = exited_at
            && (closed || at.elapsed() >= EXIT_GRACE)
        {
            break End::Exited;
        }
        let flush_due = relay.due();
        let flush = async {
            match flush_due {
                Some(at) => tokio::time::sleep_until(at.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break End::Cancelled,
            _ = &mut timeout => break End::TimedOut,
            _ = tick.tick() => {
                if exited_at.is_none() {
                    // Unix: only looks; the leader is reaped after the
                    // group has been signalled (see `stop_tree`).
                    if leader_ended(&mut *child, &mut status) {
                        exited_at = Some(Instant::now());
                    }
                }
            }
            _ = flush => relay.flush(on_output),
            msg = rx.recv(), if !closed => match msg {
                Some(text) => {
                    collector.push(&text);
                    relay.add(&text);
                }
                None => closed = true,
            },
        }
    };

    let natural = matches!(end, End::Exited);
    #[cfg(unix)]
    let reaped = stop_tree(&mut *child, &mut group, !natural).await;
    #[cfg(windows)]
    let reaped = stop_tree(&mut *child, !natural).await;
    status = status.or(reaped);

    // Whatever the stopped tree still wrote, but not for long: a grandchild
    // that holds the pipes open must not hold this up.
    if !closed {
        let until = tokio::time::Instant::now() + DRAIN_AFTER_STOP;
        while let Ok(Some(text)) = tokio::time::timeout_at(until, rx.recv()).await {
            collector.push(&text);
            relay.add(&text);
        }
    }
    for reader in &readers {
        reader.abort();
    }
    relay.flush(on_output);

    let (timed_out, stopped) = (matches!(end, End::TimedOut), matches!(end, End::Cancelled));
    let mut exit_code = None;
    if natural && let Some(st) = status {
        exit_code = st.code();
        #[cfg(unix)]
        if exit_code.is_none() {
            use std::os::unix::process::ExitStatusExt;
            if let Some(sig) = st.signal() {
                collector.push_line(&format!("killed by signal {sig}"));
            }
        }
    }

    let (kept, total_chars) = collector.finish();
    Ran {
        exit_code,
        timed_out,
        stopped,
        kept,
        total_chars,
        seconds: started.elapsed().as_secs_f32(),
    }
}

/// What the model is told: how the command ended on the first line, then
/// `output:` and the output (cut in the middle when longer than
/// `limits.to_model`). After a cut or long output (over 20 lines) the status
/// is repeated on a last line, so output that imitates a status line cannot
/// stand in for the real one.
pub fn for_model(ran: &Ran, limits: &Limits) -> String {
    const LONG_LINES: usize = 20;
    let status = if ran.timed_out {
        format!("stopped after {} s time limit", limits.timeout.as_secs())
    } else if ran.stopped {
        "stopped by the user".to_string()
    } else if let Some(code) = ran.exit_code {
        format!("exit code {code}")
    } else {
        "ended without an exit code".to_string()
    };
    let shown = ran.kept.chars().count();
    let cut = ran.total_chars > shown as u64 || shown > limits.to_model;
    let body = if ran.kept.is_empty() {
        "(no output)".to_string()
    } else {
        cut_middle(&ran.kept, limits.to_model, ran.total_chars)
    };
    let mut text = format!("{status}\noutput:\n{body}");
    if cut || ran.kept.lines().count() > LONG_LINES {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("(end of output; {status})"));
    }
    if ran.timed_out {
        text.push_str("\nCommands get no keyboard input. Use non-interactive flags.");
    }
    text
}

/// All of `text` when it fits in `max` characters, else its first sixth and
/// the rest of the budget from its end, around a notice that counts what was
/// left out of all `total` characters.
fn cut_middle(text: &str, max: usize, total: u64) -> String {
    let have = text.chars().count();
    if have <= max {
        return text.to_string();
    }
    let head = max / 6;
    let tail = max - head;
    let head_end = text.char_indices().nth(head).map_or(text.len(), |(i, _)| i);
    let tail_start = text
        .char_indices()
        .nth(have - tail)
        .map_or(text.len(), |(i, _)| i);
    let left = total.max(have as u64) - max as u64;
    format!(
        "{}{}{}",
        &text[..head_end],
        left_out(left),
        &text[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::rules::ShellFamily;
    use crate::agent::shell::detect;
    use std::sync::Mutex;
    use std::time::Instant;

    const NOTICE_LINE: &str = "Commands get no keyboard input. Use non-interactive flags.";

    fn limits(secs: u64) -> Limits {
        Limits {
            timeout: Duration::from_secs(secs),
            to_model: 12_000,
            kept: 200_000,
        }
    }

    /// Generous on Windows, where PowerShell start-up alone can take seconds.
    fn slow() -> bool {
        cfg!(windows)
    }

    fn long(secs: u64) -> u64 {
        if slow() { secs * 6 } else { secs }
    }

    fn pick<'a>(shell: &Shell, posix: &'a str, ps: &'a str) -> &'a str {
        match shell.family {
            ShellFamily::Posix => posix,
            ShellFamily::PowerShell => ps,
        }
    }

    async fn go(shell: &Shell, dir: &Path, command: &str, limits: &Limits) -> Ran {
        run(
            shell,
            dir,
            command,
            limits,
            &|_| {},
            &CancellationToken::new(),
        )
        .await
    }

    /// A short, char-safe view of text for assertion messages.
    fn shown(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= 160 {
            return format!("{text:?}");
        }
        let head: String = chars[..80].iter().collect();
        let tail: String = chars[chars.len() - 80..].iter().collect();
        format!("{head:?} ... {tail:?} ({} chars)", chars.len())
    }

    fn pos(haystack: &str, needle: &str) -> usize {
        haystack
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {haystack:?}"))
    }

    fn alive(pid: u32) -> bool {
        #[cfg(unix)]
        {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
            !stat.is_empty() && !stat.starts_with('Z')
        }
        #[cfg(windows)]
        {
            let script = format!(
                "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ 'alive' }} else {{ 'gone' }}"
            );
            let out = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).contains("alive")
        }
    }

    async fn wait_gone(pid: u32) -> bool {
        for _ in 0..100 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    async fn wait_for_pid_file(path: &Path) -> u32 {
        for _ in 0..400 {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("pid file {} never appeared", path.display());
    }

    // ---- the output cleaner -------------------------------------------------

    fn clean_all(parts: &[&[u8]]) -> String {
        let mut c = Cleaner::default();
        let mut out = String::new();
        for part in parts {
            out.push_str(&c.feed(part));
        }
        out.push_str(&c.finish());
        out
    }

    #[test]
    fn cleaner_keeps_a_multibyte_character_split_across_chunks() {
        let bytes = "aé✓b".as_bytes();
        for cut in 1..bytes.len() {
            let (a, b) = bytes.split_at(cut);
            assert_eq!(clean_all(&[a, b]), "aé✓b", "cut at {cut}");
        }
        assert_eq!(clean_all(&[b"a", &[0xE2], &[0x9C], &[0x93], b"b"]), "a✓b");
    }

    #[test]
    fn cleaner_replaces_invalid_bytes_once_each() {
        assert_eq!(clean_all(&[b"a\xFF\xFEb"]), "a\u{FFFD}\u{FFFD}b");
        assert_eq!(
            clean_all(&[b"a", &[0xE2, 0x9C]]),
            "a\u{FFFD}",
            "cut-off character at the end"
        );
    }

    #[test]
    fn cleaner_strips_ansi_sequences_even_when_split() {
        let text = b"\x1b[31mred\x1b[0m \x1b]0;title\x07ok \x1b]8;;http://x\x1b\\link\x1b(B!";
        assert_eq!(clean_all(&[text]), "red ok link!");
        for cut in 1..text.len() {
            let (a, b) = text.split_at(cut);
            assert_eq!(clean_all(&[a, b]), "red ok link!", "cut at {cut}");
        }
    }

    #[test]
    fn cleaner_handles_carriage_returns_and_control_characters() {
        assert_eq!(clean_all(&[b"a\r\nb\n"]), "a\nb\n");
        assert_eq!(
            clean_all(&[b"a\r", b"\nb"]),
            "a\nb",
            "CR LF split across chunks"
        );
        assert_eq!(clean_all(&[b"10%\r20%\r30%\n"]), "10%\n20%\n30%\n");
        assert_eq!(clean_all(&[b"a\x00b\x07c\x08d\te\x7f"]), "abcd\te");
    }

    #[test]
    fn cleaner_strips_dcs_apc_pm_and_sos_strings_even_when_split() {
        let text = "a\x1bPdata\x1b\\b\x1b_apc\x07c\x1b^pm\x1b\\d\x1bXsos\x1b\\e".as_bytes();
        assert_eq!(clean_all(&[text]), "abcde");
        for cut in 1..text.len() {
            let (a, b) = text.split_at(cut);
            assert_eq!(clean_all(&[a, b]), "abcde", "cut at {cut}");
        }
    }

    #[test]
    fn cleaner_gives_up_on_an_unterminated_string() {
        let mut text = b"before\x1b_".to_vec();
        text.extend(std::iter::repeat_n(b'z', 5_000));
        text.extend_from_slice(b"tail");
        let out = clean_all(&[&text]);
        assert!(out.starts_with("before"));
        assert!(out.ends_with("ztail"), "{}", &out[out.len() - 20..]);
        assert!(out.len() < 1_000, "{}", out.len());
    }

    #[test]
    fn cleaner_strips_eight_bit_csi_and_other_c1_controls() {
        let text = "a\u{9b}31mb\u{85}c\u{9d}0;title\u{9c}d\u{90}dcs\u{9c}e\u{80}f".as_bytes();
        assert_eq!(clean_all(&[text]), "abcdef");
        for cut in 1..text.len() {
            let (a, b) = text.split_at(cut);
            assert_eq!(clean_all(&[a, b]), "abcdef", "cut at {cut}");
        }
    }

    #[test]
    fn cleaner_replaces_invisible_format_characters_with_a_question_mark() {
        let text = "a\u{200b}b\u{202e}c\u{feff}d\u{2028}e".as_bytes();
        assert_eq!(clean_all(&[text]), "a?b?c?d?e");
        for cut in 1..text.len() {
            let (a, b) = text.split_at(cut);
            assert_eq!(clean_all(&[a, b]), "a?b?c?d?e", "cut at {cut}");
        }
    }

    // ---- limits and the model's view ---------------------------------------

    #[test]
    fn timeout_is_clamped_to_one_through_six_hundred_and_defaults_to_120() {
        assert_eq!(clamp_timeout(None), Duration::from_secs(120));
        assert_eq!(clamp_timeout(Some(0)), Duration::from_secs(1));
        assert_eq!(clamp_timeout(Some(-5)), Duration::from_secs(1));
        assert_eq!(clamp_timeout(Some(30)), Duration::from_secs(30));
        assert_eq!(clamp_timeout(Some(600)), Duration::from_secs(600));
        assert_eq!(clamp_timeout(Some(9_999)), Duration::from_secs(600));
    }

    fn ran(code: Option<i32>, kept: &str) -> Ran {
        Ran {
            exit_code: code,
            timed_out: false,
            stopped: false,
            kept: kept.into(),
            total_chars: kept.chars().count() as u64,
            seconds: 0.5,
        }
    }

    #[test]
    fn for_model_reports_status_then_labelled_output() {
        let l = limits(120);
        assert_eq!(
            for_model(&ran(Some(0), "hello\n"), &l),
            "exit code 0\noutput:\nhello\n"
        );
        assert_eq!(
            for_model(&ran(Some(2), ""), &l),
            "exit code 2\noutput:\n(no output)"
        );
        let mut t = ran(None, "partial\n");
        t.timed_out = true;
        assert_eq!(
            for_model(&t, &limits(5)),
            format!("stopped after 5 s time limit\noutput:\npartial\n\n{NOTICE_LINE}")
        );
        let mut s = ran(None, "");
        s.stopped = true;
        let text = for_model(&s, &l);
        assert_eq!(text, "stopped by the user\noutput:\n(no output)");
        assert!(
            !text.contains(NOTICE_LINE),
            "the line is only for a time-out"
        );
        assert!(!for_model(&ran(Some(1), "no input here, not a tty\n"), &l).contains(NOTICE_LINE));
        assert_eq!(
            for_model(&ran(None, "x\n"), &l),
            "ended without an exit code\noutput:\nx\n"
        );
    }

    #[test]
    fn for_model_repeats_the_status_after_long_output() {
        let l = limits(120);
        let twenty = "l\n".repeat(20);
        assert_eq!(
            for_model(&ran(Some(0), &twenty), &l),
            format!("exit code 0\noutput:\n{twenty}")
        );
        let twenty_one = "l\n".repeat(21);
        assert_eq!(
            for_model(&ran(Some(0), &twenty_one), &l),
            format!("exit code 0\noutput:\n{twenty_one}(end of output; exit code 0)")
        );
        let no_newline = format!("{}l", "l\n".repeat(20));
        assert_eq!(
            for_model(&ran(Some(3), &no_newline), &l),
            format!("exit code 3\noutput:\n{no_newline}\n(end of output; exit code 3)")
        );
        let mut t = ran(None, &twenty_one);
        t.timed_out = true;
        let text = for_model(&t, &limits(5));
        assert!(text.contains("\n(end of output; stopped after 5 s time limit)\n"));
        assert!(text.ends_with(NOTICE_LINE));
    }

    #[test]
    fn output_cannot_imitate_the_status_line() {
        let l = limits(120);
        let text = for_model(&ran(Some(1), "exit code 0\nall tests passed\n"), &l);
        assert!(
            text.starts_with("exit code 1\noutput:\nexit code 0\n"),
            "{text}"
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "exit code 1");
        assert_eq!(lines[1], "output:");
    }

    #[test]
    fn for_model_cuts_the_middle_around_the_notice() {
        let l = limits(120);
        let kept: String = (0..30_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let text = for_model(&ran(Some(0), &kept), &l);
        let body = text
            .strip_prefix("exit code 0\noutput:\n")
            .unwrap()
            .strip_suffix("\n(end of output; exit code 0)")
            .unwrap();
        let notice = "\n[… 18000 characters left out …]\n";
        assert_eq!(body.chars().count(), 12_000 + notice.chars().count());
        assert!(body.starts_with(&kept[..2_000]));
        assert!(body.ends_with(&kept[kept.len() - 10_000..]));
        assert_eq!(&body[2_000..2_000 + notice.len()], notice);
        // exactly at the limit is not cut
        let exact: String = "x".repeat(12_000);
        assert_eq!(
            for_model(&ran(Some(0), &exact), &l),
            format!("exit code 0\noutput:\n{exact}")
        );
    }

    #[test]
    fn the_notice_counts_what_was_left_out_of_the_whole_output() {
        // `kept` was already cut from a million characters.
        let kept: String = "k".repeat(199_900);
        let mut r = ran(Some(0), &kept);
        r.total_chars = 1_000_000;
        let text = for_model(&r, &limits(120));
        assert!(
            text.contains("[… 988000 characters left out …]"),
            "{}",
            &text[..200]
        );
        assert!(text.ends_with("(end of output; exit code 0)"));
    }

    // ---- bounded collection ---------------------------------------------------

    fn collect(chars: usize) -> (String, u64) {
        let mut c = Collector::new(200_000);
        let piece: String = "a".repeat(10_000);
        let mut left = chars;
        while left > 0 {
            let n = left.min(10_000);
            c.push(&piece[..n]);
            left -= n;
        }
        c.finish()
    }

    #[test]
    fn exactly_the_kept_size_comes_back_whole_and_one_more_is_cut() {
        let (whole, total) = collect(200_000);
        assert_eq!(whole.chars().count(), 200_000);
        assert!(!whole.contains("left out"));
        assert_eq!(total, 200_000);
        let (almost, _) = collect(199_950);
        assert_eq!(almost.chars().count(), 199_950);
        assert!(!almost.contains("left out"));
        let (cut, total) = collect(200_001);
        assert_eq!(total, 200_001);
        assert!(cut.contains("characters left out"));
        assert!(cut.chars().count() <= 200_000, "{}", cut.chars().count());
        assert!(cut.starts_with(&"a".repeat(50_000)));
    }

    #[test]
    fn collection_stays_bounded_and_counts_everything() {
        let mut c = Collector::new(1_000);
        for _ in 0..1_000 {
            c.push(&"é".repeat(100));
        }
        assert!(c.head.chars().count() + c.tail.chars().count() <= 1_000);
        let (text, total) = c.finish();
        assert_eq!(total, 100_000);
        assert!(text.chars().count() <= 1_000);
        assert!(text.contains(&format!(
            "{} characters left out",
            100_000 - 250 - (1_000 - 250 - NOTICE_ROOM)
        )));
    }

    // ---- real processes -----------------------------------------------------

    #[tokio::test]
    async fn echo_prints_and_exits_zero() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let r = go(&shell, dir.path(), "echo hello", &limits(long(20))).await;
        assert_eq!(r.exit_code, Some(0), "{}", shown(&r.kept));
        assert!(!r.timed_out && !r.stopped);
        assert_eq!(r.kept.trim(), "hello", "{}", shown(&r.kept));
        assert!(r.seconds > 0.0);
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_code_and_both_streams_in_order() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "echo out1; echo err1 1>&2; echo out2; echo err2 1>&2; echo out3; exit 3",
            "'out1'; [Console]::Error.WriteLine('err1'); 'out2'; [Console]::Error.WriteLine('err2'); 'out3'; exit 3",
        );
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        assert_eq!(r.exit_code, Some(3));
        // Within one stream the order is kept; across streams it is not promised.
        assert!(pos(&r.kept, "out1") < pos(&r.kept, "out2"));
        assert!(pos(&r.kept, "out2") < pos(&r.kept, "out3"));
        assert!(pos(&r.kept, "err1") < pos(&r.kept, "err2"));
        let text = for_model(&r, &limits(20));
        assert!(text.starts_with("exit code 3\n"));
        assert!(text.contains("err1") && text.contains("out3"));
    }

    #[tokio::test]
    async fn the_working_directory_is_the_project_root() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let cmd = pick(&shell, "pwd", "(Get-Location).Path");
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        assert_eq!(r.exit_code, Some(0));
        assert!(r.kept.trim().ends_with(&name), "{:?} vs {name}", r.kept);
    }

    #[tokio::test]
    async fn stdin_is_closed_so_a_reading_command_ends() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "read line; echo \"done:[$line]\"",
            "$l = [Console]::In.ReadLine(); \"done:[$l]\"",
        );
        let started = Instant::now();
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        assert!(!r.timed_out);
        assert!(r.kept.contains("done:["), "{}", shown(&r.kept));
        assert!(started.elapsed() < Duration::from_secs(long(10)));
    }

    #[tokio::test]
    async fn a_command_past_its_time_limit_is_stopped() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "echo started; sleep 30",
            "'started'; Start-Sleep -Seconds 60",
        );
        let limit = long(1);
        let started = Instant::now();
        let r = go(&shell, dir.path(), cmd, &limits(limit)).await;
        let took = started.elapsed();
        assert!(r.timed_out);
        assert!(!r.stopped);
        assert_eq!(r.exit_code, None);
        assert!(
            took < Duration::from_secs(limit) + Duration::from_secs(2),
            "took {took:?}"
        );
        assert!(took >= Duration::from_secs(limit));
        let text = for_model(&r, &limits(limit));
        assert!(text.starts_with(&format!("stopped after {limit} s time limit\n")));
        assert!(text.ends_with(NOTICE_LINE));
    }

    #[tokio::test]
    async fn cancel_stops_a_sleeping_command_within_two_seconds() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(&shell, "sleep 30", "Start-Sleep -Seconds 60");
        let lim = limits(120);
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let wait = if slow() { 4 } else { 1 };
        let fired: Mutex<Option<Instant>> = Mutex::new(None);
        let (r, ()) = tokio::join!(
            run(&shell, dir.path(), cmd, &lim, &|_| {}, &cancel),
            async {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                *fired.lock().unwrap() = Some(Instant::now());
                trigger.cancel();
            }
        );
        let after = fired.lock().unwrap().unwrap().elapsed();
        assert!(r.stopped);
        assert!(!r.timed_out);
        assert_eq!(r.exit_code, None);
        assert!(
            after < Duration::from_secs(2),
            "stopped {after:?} after cancel"
        );
        assert_eq!(
            for_model(&r, &limits(120)),
            "stopped by the user\noutput:\n(no output)"
        );
    }

    #[tokio::test]
    async fn cancel_also_stops_a_grandchild() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid.txt");
        let cmd = match shell.family {
            ShellFamily::Posix => {
                std::fs::write(
                    dir.path().join("child.sh"),
                    "echo $$ > pid.txt\nexec sleep 30\n",
                )
                .unwrap();
                "sh ./child.sh & wait"
            }
            ShellFamily::PowerShell => {
                "$p = Start-Process -NoNewWindow -PassThru powershell -ArgumentList '-NoProfile','-Command','Set-Content -Path pid.txt -Value $PID; Start-Sleep 60'; $p.WaitForExit()"
            }
        };
        let lim = limits(120);
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let file = pid_file.clone();
        let (r, pid) = tokio::join!(
            run(&shell, dir.path(), cmd, &lim, &|_| {}, &cancel),
            async move {
                let pid = wait_for_pid_file(&file).await;
                trigger.cancel();
                pid
            }
        );
        assert!(r.stopped);
        assert!(wait_gone(pid).await, "grandchild {pid} is still running");
    }

    #[tokio::test]
    async fn a_background_child_does_not_outlive_the_command() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "sleep 30 & echo $! > pid.txt; echo started",
            "$p = Start-Process -NoNewWindow -PassThru powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 60'; Set-Content -Path pid.txt -Value $p.Id; 'started'",
        );
        let started = Instant::now();
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        assert!(
            started.elapsed() < Duration::from_secs(long(8)),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(r.exit_code, Some(0));
        assert!(!r.timed_out && !r.stopped);
        assert!(r.kept.contains("started"), "{}", shown(&r.kept));
        let pid = wait_for_pid_file(&dir.path().join("pid.txt")).await;
        assert!(
            wait_gone(pid).await,
            "background child {pid} is still running"
        );
    }

    #[tokio::test]
    async fn a_million_characters_are_kept_bounded_with_head_and_tail() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "awk 'BEGIN { for (i = 0; i < 100000; i++) printf \"line%05d\\n\", i }'",
            "0..99999 | ForEach-Object { 'line{0:D5}' -f $_ }",
        );
        let r = go(&shell, dir.path(), cmd, &limits(long(60))).await;
        let k = shown(&r.kept);
        assert_eq!(r.exit_code, Some(0), "{k}");
        assert!(r.kept.chars().count() <= 200_000, "{k}");
        assert!(r.kept.starts_with("line00000"), "{k}");
        assert!(r.kept.trim_end().ends_with("line99999"), "{k}");
        assert!(r.kept.contains("characters left out"), "{k}");
        let text = for_model(&r, &limits(60));
        let t = shown(&text);
        assert_eq!(r.total_chars, 1_000_000, "{k}");
        assert!(text.contains("[… 988000 characters left out …]"), "{t}");
        assert!(
            text.chars().count() <= "exit code 0\noutput:\n".len() + 12_000 + 64 + 40,
            "{t}"
        );
        assert!(text.contains("line00000"), "{t}");
        assert!(
            text.contains("line99999\n(end of output; exit code 0)"),
            "{t}"
        );
    }

    #[tokio::test]
    async fn bytes_that_are_not_utf8_become_replacement_characters() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "printf 'a\\377\\376b\\n'",
            "$o = [Console]::OpenStandardOutput(); $o.Write([byte[]](97,255,254,98,10), 0, 5); $o.Flush()",
        );
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.kept.trim(), "a\u{FFFD}\u{FFFD}b", "{}", shown(&r.kept));
    }

    #[tokio::test]
    async fn endless_output_is_stopped_at_the_limit_with_bounded_memory() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "yes xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            "while ($true) { 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' }",
        );
        let limit = long(1);
        let started = Instant::now();
        let r = go(&shell, dir.path(), cmd, &limits(limit)).await;
        assert!(r.timed_out);
        assert!(started.elapsed() < Duration::from_secs(limit) + Duration::from_secs(2));
        assert!(r.kept.chars().count() <= 200_000, "{}", shown(&r.kept));
        assert!(r.kept.contains("xxxxxxxxxx"), "{}", shown(&r.kept));
    }

    #[tokio::test]
    async fn the_fixed_environment_reaches_the_command() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "echo \"term=[$TERM] nocolor=[$NO_COLOR] pager=[$PAGER]\"",
            "\"term=[$env:TERM] nocolor=[$env:NO_COLOR] pager=[$env:PAGER]\"",
        );
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        assert_eq!(
            r.kept.trim(),
            "term=[dumb] nocolor=[1] pager=[cat]",
            "{}",
            shown(&r.kept)
        );
    }

    #[tokio::test]
    async fn quotes_backslashes_percent_and_non_ascii_arrive_unchanged() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            r#"printf '%s\n' 'héllo ✓ "q" 100% C:\dir\'"#,
            r#"Write-Output 'héllo ✓ "q" 100% C:\dir\'"#,
        );
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        assert_eq!(r.exit_code, Some(0), "{}", shown(&r.kept));
        assert_eq!(
            r.kept.trim(),
            r#"héllo ✓ "q" 100% C:\dir\"#,
            "{}",
            shown(&r.kept)
        );
    }

    #[tokio::test]
    async fn on_output_receives_chunks_before_the_command_ends() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "echo first; sleep 1; echo second",
            "'first'; Start-Sleep -Seconds 3; 'second'",
        );
        let seen: Mutex<Vec<(Instant, String)>> = Mutex::new(Vec::new());
        let r = run(
            &shell,
            dir.path(),
            cmd,
            &limits(long(20)),
            &|chunk| {
                seen.lock()
                    .unwrap()
                    .push((Instant::now(), chunk.to_string()))
            },
            &CancellationToken::new(),
        )
        .await;
        let finished = Instant::now();
        let seen = seen.into_inner().unwrap();
        assert_eq!(r.exit_code, Some(0));
        let joined: String = seen.iter().map(|(_, s)| s.as_str()).collect();
        assert!(
            joined.contains("first") && joined.contains("second"),
            "{joined:?}"
        );
        let (at, _) = seen.iter().find(|(_, s)| s.contains("first")).unwrap();
        let margin = if slow() { 1 } else { 0 };
        assert!(
            finished.duration_since(*at) >= Duration::from_millis(500 + 500 * margin),
            "the first chunk came only {:?} before the end",
            finished.duration_since(*at)
        );
        assert!(seen.iter().all(|(_, s)| s.chars().count() <= 4_000));
    }

    #[tokio::test]
    async fn a_burst_reaches_on_output_in_calls_of_at_most_4000_characters() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "awk 'BEGIN { for (i = 0; i < 2000; i++) printf \"line%05d\\n\", i }'",
            "0..1999 | ForEach-Object { 'line{0:D5}' -f $_ }",
        );
        let calls: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let r = run(
            &shell,
            dir.path(),
            cmd,
            &limits(long(30)),
            &|chunk| calls.lock().unwrap().push(chunk.to_string()),
            &CancellationToken::new(),
        )
        .await;
        let calls = calls.into_inner().unwrap();
        assert_eq!(r.exit_code, Some(0), "{}", shown(&r.kept));
        assert!(calls.iter().all(|c| c.chars().count() <= 4_000));
        assert!(calls.len() < 200, "{} calls", calls.len());
        let last = calls.last().unwrap();
        assert!(last.ends_with("line01999\n"), "{}", shown(last));
    }

    #[tokio::test]
    async fn endless_output_is_passed_on_at_a_bounded_rate() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "yes xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            "while ($true) { 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' }",
        );
        let limit = if slow() { 5 } else { 2 };
        let calls: Mutex<Vec<(Instant, usize, bool)>> = Mutex::new(Vec::new());
        let r = run(
            &shell,
            dir.path(),
            cmd,
            &limits(limit),
            &|chunk| {
                calls.lock().unwrap().push((
                    Instant::now(),
                    chunk.chars().count(),
                    chunk.starts_with("[… "),
                ))
            },
            &CancellationToken::new(),
        )
        .await;
        let calls = calls.into_inner().unwrap();
        assert!(r.timed_out);
        let max_calls = limit as usize * 10 + 5;
        assert!(calls.len() <= max_calls, "{} calls", calls.len());
        assert!(calls.iter().all(|(_, n, _)| *n <= 4_000));
        let total: usize = calls.iter().map(|(_, n, _)| n).sum();
        assert!(total <= max_calls * 4_000, "{total} characters");
        // At most one call per 100 ms (the last one may come early).
        for pair in calls[..calls.len().saturating_sub(1)].windows(2) {
            assert!(
                pair[1].0.duration_since(pair[0].0) >= Duration::from_millis(90),
                "calls too close together"
            );
        }
        if !slow() {
            assert!(
                calls.iter().any(|(_, _, skipped)| *skipped),
                "no skipped notice"
            );
        }
    }

    #[tokio::test]
    async fn the_skipped_notice_names_the_count_and_fits_the_cap() {
        let mut relay = Relay::new();
        let calls: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let sink = |c: &str| calls.lock().unwrap().push(c.to_string());
        relay.add(&"a".repeat(3_000));
        relay.add(&"b".repeat(3_000));
        relay.flush(&sink);
        let calls = calls.into_inner().unwrap();
        assert_eq!(calls.len(), 1);
        let text = &calls[0];
        assert!(text.starts_with("[… "), "{}", &text[..30]);
        let (notice, rest) = text.split_once('\n').unwrap();
        assert!(notice.ends_with(" characters skipped …]"));
        assert!(text.chars().count() <= 4_000);
        assert!(rest.ends_with('b') && rest.starts_with('a'));
        let skipped: usize = notice.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert_eq!(skipped + rest.chars().count(), 6_000);
    }

    #[tokio::test]
    async fn a_token_cancelled_before_the_start_runs_nothing() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(&shell, "echo hi > marker", "'hi' | Set-Content marker");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let r = run(&shell, dir.path(), cmd, &limits(20), &|_| {}, &cancel).await;
        assert!(r.stopped && !r.timed_out);
        assert_eq!(r.exit_code, None);
        assert!(!dir.path().join("marker").exists());
        assert!(r.seconds < 1.0);
    }

    #[tokio::test]
    async fn dropping_the_run_mid_way_still_ends_the_whole_tree() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = match shell.family {
            ShellFamily::Posix => {
                std::fs::write(
                    dir.path().join("child.sh"),
                    "echo $$ > pid.txt\nexec sleep 30\n",
                )
                .unwrap();
                "sh ./child.sh & wait"
            }
            ShellFamily::PowerShell => {
                "$p = Start-Process -NoNewWindow -PassThru powershell -ArgumentList '-NoProfile','-Command','Set-Content -Path pid.txt -Value $PID; Start-Sleep 60'; $p.WaitForExit()"
            }
        };
        let lim = limits(120);
        let cancel = CancellationToken::new();
        let file = dir.path().join("pid.txt");
        let pid = {
            let running = run(&shell, dir.path(), cmd, &lim, &|_| {}, &cancel);
            tokio::select! {
                _ = running => panic!("the command should still be running"),
                pid = wait_for_pid_file(&file) => pid,
            }
        };
        assert!(wait_gone(pid).await, "grandchild {pid} is still running");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_the_run_during_the_stop_grace_still_kills_what_ignores_sigterm() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = "sh -c 'trap \"\" TERM; echo $$ > a.pid; exec sleep 74' & \
                   trap \"\" TERM; echo $$ > b.pid; sleep 73 & echo $! > c.pid; wait";
        let lim = limits(120);
        let cancel = CancellationToken::new();
        let pids = {
            let running = run(&shell, dir.path(), cmd, &lim, &|_| {}, &cancel);
            let stop_then_give_up = async {
                let a = wait_for_pid_file(&dir.path().join("a.pid")).await;
                let b = wait_for_pid_file(&dir.path().join("b.pid")).await;
                let c = wait_for_pid_file(&dir.path().join("c.pid")).await;
                cancel.cancel();
                // Inside the 500 ms SIGTERM grace; nothing has been killed yet.
                tokio::time::sleep(Duration::from_millis(150)).await;
                [a, b, c]
            };
            tokio::select! {
                _ = running => panic!("the command should still be running"),
                pids = stop_then_give_up => pids,
            }
        };
        for pid in pids {
            assert!(wait_gone(pid).await, "process {pid} is still running");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_leader_can_be_seen_to_have_ended_without_being_reaped() {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", "exit 3"]);
        let mut child: Box<dyn ChildWrapper> = Box::new(cmd.spawn().unwrap());
        let mut status = None;
        for _ in 0..200 {
            if leader_ended(&mut *child, &mut status) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(leader_ended(&mut *child, &mut status), "never ended");
        // Still waitable afterwards, with its real status.
        let st = child.wait().await.unwrap();
        assert_eq!(st.code(), Some(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_has_no_controlling_terminal() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let r = go(&shell, dir.path(), "read x < /dev/tty", &limits(20)).await;
        assert!(!r.timed_out);
        assert!(
            matches!(r.exit_code, Some(code) if code != 0),
            "{:?}",
            r.exit_code
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn death_by_signal_has_no_exit_code_and_says_so() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let r = go(&shell, dir.path(), "echo before; kill -9 $$", &limits(20)).await;
        assert_eq!(r.exit_code, None);
        assert!(!r.timed_out && !r.stopped);
        assert_eq!(r.kept, "before\nkilled by signal 9\n");
        assert!(for_model(&r, &limits(20)).starts_with("ended without an exit code\n"));
    }

    #[tokio::test]
    async fn a_missing_project_root_runs_nothing() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("not-there");
        let marker = dir.path().join("marker");
        let cmd = pick(
            &shell,
            "echo hi > ../marker",
            "'hi' | Set-Content ../marker",
        );
        let r = go(&shell, &gone, cmd, &limits(20)).await;
        assert_eq!(r.exit_code, None);
        assert!(!r.timed_out && !r.stopped);
        assert!(r.kept.contains("folder"), "{}", shown(&r.kept));
        assert!(!marker.exists());
        assert!(
            dir.path().join("not-there").metadata().is_err(),
            "nothing was created"
        );
    }
}
