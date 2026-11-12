//! Run one shell command in the project folder: capture its output with
//! bounded memory, enforce a time limit, and stop the whole process tree on
//! timeout or cancel.
//!
//! Output is read from both pipes at once and merged in arrival order (the
//! order between the two streams is not exact). Only the first quarter and the
//! last part of what fits in `Limits::kept` is held; the pipes are still read
//! to the end so the command never blocks on a full pipe.

use std::path::Path;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
#[cfg(windows)]
use process_wrap::tokio::{CreationFlags, JobObject};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::shell::Shell;

/// Longest a command may run when the model gives no time limit.
const DEFAULT_TIMEOUT_SECS: i64 = 120;
const MAX_TIMEOUT_SECS: i64 = 600;
/// Room kept free in `kept` for the "left out" notice.
const NOTICE_ROOM: usize = 64;
/// Output is passed on at least this often...
const FLUSH_EVERY: Duration = Duration::from_millis(100);
/// ...and in pieces of at most this many characters.
const FLUSH_CHARS: usize = 4_000;
/// How long output is still collected after the child exited, waiting for the
/// pipes to close, before the rest of its tree is stopped.
const EXIT_GRACE: Duration = Duration::from_secs(1);
/// How long output is still collected after the tree was stopped.
const DRAIN_AFTER_STOP: Duration = Duration::from_millis(300);
/// On unix, how long the tree gets to end after SIGTERM before SIGKILL.
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_millis(500);
#[cfg(unix)]
const SIGTERM: i32 = 15;
/// Longest escape sequence given up on as a sequence (so a lone ESC cannot
/// swallow the rest of the output).
const MAX_SEQUENCE: usize = 2_048;

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
    /// Inside `ESC ] ...`.
    Osc,
    /// Inside an OSC, after ESC (the start of its terminator).
    OscTerminator,
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
                    ']' => Esc::Osc,
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
            Esc::Osc => {
                self.in_sequence += 1;
                if c == '\u{7}' || self.in_sequence > MAX_SEQUENCE {
                    self.esc = Esc::Text;
                } else if c == '\u{1b}' {
                    self.esc = Esc::OscTerminator;
                }
                return;
            }
            Esc::OscTerminator => {
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
            '\r' => {
                out.push('\n');
                self.after_cr = true;
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// `"\n[… {n} characters left out …]\n"`
fn left_out(n: usize) -> String {
    format!("\n[… {n} characters left out …]\n")
}

/// The first part and the last part of an unbounded stream of text.
struct Collector {
    head: String,
    head_chars: usize,
    head_cap: usize,
    tail: String,
    tail_chars: usize,
    tail_cap: usize,
    total: usize,
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
            tail_cap: kept.saturating_sub(head_cap + NOTICE_ROOM),
            total: 0,
        }
    }

    fn push(&mut self, text: &str) {
        let n = text.chars().count();
        self.total += n;
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
        if self.tail_chars > self.tail_cap {
            let excess = self.tail_chars - self.tail_cap;
            let at = self
                .tail
                .char_indices()
                .nth(excess)
                .map_or(self.tail.len(), |(i, _)| i);
            self.tail.drain(..at);
            self.tail_chars = self.tail_cap;
        }
    }

    /// Adds `line` on a line of its own.
    fn push_line(&mut self, line: &str) {
        let ends_clean = match self
            .tail
            .chars()
            .last()
            .or_else(|| self.head.chars().last())
        {
            None => true,
            Some(c) => c == '\n',
        };
        if !ends_clean {
            self.push("\n");
        }
        self.push(line);
        self.push("\n");
    }

    fn finish(self) -> String {
        let kept = self.head_chars + self.tail_chars;
        let mut out = self.head;
        if self.total > kept {
            out.push_str(&left_out(self.total - kept));
        }
        out.push_str(&self.tail);
        out
    }
}

/// Hands text to `on_output` in coalesced pieces.
struct Relay {
    pending: String,
    chars: usize,
    since: Option<Instant>,
}

impl Relay {
    fn new() -> Self {
        Self {
            pending: String::new(),
            chars: 0,
            since: None,
        }
    }

    fn due(&self) -> Option<Instant> {
        self.since.map(|t| t + FLUSH_EVERY)
    }

    fn add(&mut self, text: &str, on_output: &(dyn Fn(&str) + Send + Sync)) {
        if text.is_empty() {
            return;
        }
        self.pending.push_str(text);
        self.chars += text.chars().count();
        self.since.get_or_insert_with(Instant::now);
        while self.chars >= FLUSH_CHARS {
            let at = self
                .pending
                .char_indices()
                .nth(FLUSH_CHARS)
                .map_or(self.pending.len(), |(i, _)| i);
            on_output(&self.pending[..at]);
            self.pending.drain(..at);
            self.chars -= FLUSH_CHARS;
        }
        if self.pending.is_empty() {
            self.since = None;
        }
    }

    fn flush(&mut self, on_output: &(dyn Fn(&str) + Send + Sync)) {
        if !self.pending.is_empty() {
            on_output(&self.pending);
        }
        self.pending.clear();
        self.chars = 0;
        self.since = None;
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

fn not_run(started: Instant, message: String) -> Ran {
    Ran {
        exit_code: None,
        timed_out: false,
        stopped: false,
        kept: message,
        seconds: started.elapsed().as_secs_f32(),
    }
}

/// Ends everything the command started. With `graceful` (unix) the group is
/// asked to end first and gets 500 ms; then it is killed outright. On Windows
/// the Job Object is terminated.
async fn stop_tree(child: &mut dyn ChildWrapper, graceful: bool) {
    #[cfg(unix)]
    if graceful {
        let _ = child.signal(SIGTERM);
        let until = Instant::now() + TERM_GRACE;
        while Instant::now() < until && !matches!(child.try_wait(), Ok(Some(_))) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    #[cfg(not(unix))]
    let _ = graceful;
    let _ = child.start_kill();
}

enum End {
    Exited,
    TimedOut,
    Cancelled,
}

/// Runs `command` in `project_root` with the given shell. Returns when it
/// ends, when `limits.timeout` passes, or when `cancel` fires; in the last two
/// cases the whole process tree is stopped first. `on_output` gets the text as
/// it arrives and is never called after this returns.
pub async fn run(
    shell: &Shell,
    project_root: &Path,
    command: &str,
    limits: &Limits,
    on_output: &(dyn Fn(&str) + Send + Sync),
    cancel: &CancellationToken,
) -> Ran {
    let started = Instant::now();
    if !project_root.is_dir() {
        return not_run(
            started,
            format!(
                "The project folder {} does not exist, so the command was not run.",
                project_root.display()
            ),
        );
    }

    let mut cmd = shell.command(command);
    cmd.current_dir(project_root);
    let mut wrap = CommandWrap::from(cmd);
    wrap.wrap(KillOnDrop);
    #[cfg(unix)]
    wrap.wrap(ProcessGroup::leader());
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
            );
        }
    };

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
                    match child.try_wait() {
                        Ok(Some(st)) => {
                            status = Some(st);
                            exited_at = Some(Instant::now());
                        }
                        Ok(None) => {}
                        Err(_) => exited_at = Some(Instant::now()),
                    }
                }
            }
            _ = flush => relay.flush(on_output),
            msg = rx.recv(), if !closed => match msg {
                Some(text) => {
                    collector.push(&text);
                    relay.add(&text, on_output);
                }
                None => closed = true,
            },
        }
    };

    let natural = matches!(end, End::Exited);
    stop_tree(&mut *child, !natural).await;

    // Whatever the stopped tree still wrote, but not for long: a grandchild
    // that holds the pipes open must not hold this up.
    if !closed {
        let until = tokio::time::Instant::now() + DRAIN_AFTER_STOP;
        while let Ok(Some(text)) = tokio::time::timeout_at(until, rx.recv()).await {
            collector.push(&text);
            relay.add(&text, on_output);
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

    Ran {
        exit_code,
        timed_out,
        stopped,
        kept: collector.finish(),
        seconds: started.elapsed().as_secs_f32(),
    }
}

/// What the model is told: how the command ended, then its output, cut in the
/// middle when it is longer than `limits.to_model`.
pub fn for_model(ran: &Ran, limits: &Limits) -> String {
    let status = if ran.timed_out {
        format!("stopped after {} s time limit", limits.timeout.as_secs())
    } else if ran.stopped {
        "stopped by the user".to_string()
    } else if let Some(code) = ran.exit_code {
        format!("exit code {code}")
    } else {
        "ended without an exit code".to_string()
    };
    let body = if ran.kept.is_empty() {
        "(no output)".to_string()
    } else {
        cut_middle(&ran.kept, limits.to_model)
    };
    let mut text = format!("{status}\n{body}");
    if ran.timed_out {
        text.push_str("\nCommands get no keyboard input. Use non-interactive flags.");
    }
    text
}

/// All of `text` when it fits in `max` characters, else its first sixth and
/// the rest of the budget from its end, around a notice.
fn cut_middle(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let head = max / 6;
    let tail = max - head;
    let head_end = text.char_indices().nth(head).map_or(text.len(), |(i, _)| i);
    let tail_start = text
        .char_indices()
        .nth(total - tail)
        .map_or(text.len(), |(i, _)| i);
    format!(
        "{}{}{}",
        &text[..head_end],
        left_out(total - max),
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
            seconds: 0.5,
        }
    }

    #[test]
    fn for_model_reports_status_and_output() {
        let l = limits(120);
        assert_eq!(
            for_model(&ran(Some(0), "hello\n"), &l),
            "exit code 0\nhello\n"
        );
        assert_eq!(for_model(&ran(Some(2), ""), &l), "exit code 2\n(no output)");
        let mut t = ran(None, "partial\n");
        t.timed_out = true;
        assert_eq!(
            for_model(&t, &limits(5)),
            format!("stopped after 5 s time limit\npartial\n\n{NOTICE_LINE}")
        );
        let mut s = ran(None, "");
        s.stopped = true;
        let text = for_model(&s, &l);
        assert_eq!(text, "stopped by the user\n(no output)");
        assert!(
            !text.contains(NOTICE_LINE),
            "the line is only for a time-out"
        );
        assert!(!for_model(&ran(Some(1), "no input here, not a tty\n"), &l).contains(NOTICE_LINE));
    }

    #[test]
    fn for_model_cuts_the_middle_around_the_notice() {
        let l = limits(120);
        let kept: String = (0..30_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let text = for_model(&ran(Some(0), &kept), &l);
        let body = text.strip_prefix("exit code 0\n").unwrap();
        let notice = "\n[… 18000 characters left out …]\n";
        assert_eq!(body.chars().count(), 12_000 + notice.chars().count());
        assert!(body.starts_with(&kept[..2_000]));
        assert!(body.ends_with(&kept[kept.len() - 10_000..]));
        assert_eq!(&body[2_000..2_000 + notice.len()], notice);
        // exactly at the limit is not cut
        let exact: String = "x".repeat(12_000);
        assert_eq!(
            for_model(&ran(Some(0), &exact), &l),
            format!("exit code 0\n{exact}")
        );
    }

    // ---- real processes -----------------------------------------------------

    #[tokio::test]
    async fn echo_prints_and_exits_zero() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let r = go(&shell, dir.path(), "echo hello", &limits(long(20))).await;
        assert_eq!(r.exit_code, Some(0));
        assert!(!r.timed_out && !r.stopped);
        assert_eq!(r.kept.trim(), "hello");
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
        assert!(r.kept.contains("done:["), "{:?}", r.kept);
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
            "stopped by the user\n(no output)"
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
        assert!(r.kept.contains("started"));
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
            "1..100000 | ForEach-Object { 'line{0:D5}' -f $_ }",
        );
        let r = go(&shell, dir.path(), cmd, &limits(long(60))).await;
        assert_eq!(r.exit_code, Some(0));
        assert!(
            r.kept.chars().count() <= 200_000,
            "{}",
            r.kept.chars().count()
        );
        assert!(r.kept.starts_with("line00000"));
        assert!(r.kept.trim_end().ends_with("line99999"));
        assert!(r.kept.contains("characters left out"));
        let text = for_model(&r, &limits(60));
        let notice_room = 60;
        assert!(text.chars().count() <= "exit code 0\n".len() + 12_000 + notice_room);
        assert!(text.contains("line00000"));
        assert!(text.trim_end().ends_with("line99999"));
        assert!(text.contains("characters left out"));
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
        assert_eq!(r.kept.trim(), "a\u{FFFD}\u{FFFD}b");
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
        assert!(r.kept.chars().count() <= 200_000);
        assert!(r.kept.contains("xxxxxxxxxx"));
    }

    #[tokio::test]
    async fn the_api_key_is_not_in_the_commands_environment() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: no other test reads or writes this variable.
        unsafe { std::env::set_var("OPENROUTER_API_KEY", "sk-test-not-a-real-key") };
        let cmd = pick(
            &shell,
            "echo \"key=[$OPENROUTER_API_KEY] term=[$TERM] nocolor=[$NO_COLOR]\"",
            "\"key=[$env:OPENROUTER_API_KEY] term=[$env:TERM] nocolor=[$env:NO_COLOR]\"",
        );
        let r = go(&shell, dir.path(), cmd, &limits(long(20))).await;
        unsafe { std::env::remove_var("OPENROUTER_API_KEY") };
        assert_eq!(r.kept.trim(), "key=[] term=[dumb] nocolor=[1]");
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
        assert_eq!(r.exit_code, Some(0), "{:?}", r.kept);
        assert_eq!(r.kept.trim(), r#"héllo ✓ "q" 100% C:\dir\"#);
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
    async fn on_output_coalesces_a_burst_into_few_calls_of_at_most_4000_characters() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let cmd = pick(
            &shell,
            "awk 'BEGIN { for (i = 0; i < 2000; i++) printf \"line%05d\\n\", i }'",
            "1..2000 | ForEach-Object { 'line{0:D5}' -f $_ }",
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
        assert_eq!(r.exit_code, Some(0));
        assert!(calls.iter().all(|c| c.chars().count() <= 4_000));
        assert!(calls.len() < 200, "{} calls", calls.len());
        assert_eq!(calls.concat(), r.kept);
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
        assert!(r.kept.contains("folder"), "{:?}", r.kept);
        assert!(!marker.exists());
        assert!(
            dir.path().join("not-there").metadata().is_err(),
            "nothing was created"
        );
    }
}
