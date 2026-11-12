//! Background commands: a dev server or watcher that is started, keeps running
//! between the model's steps, can have its new output read, and is stopped on
//! request, when its chat is deleted, when the chat's folder is closed or
//! changed, and when the app quits.
//!
//! A background command differs from a foreground one (`run::run`) in that it
//! has no time limit and no caller awaiting it: a supervisor task owns the
//! process and collects its output continuously into a ring of the newest
//! `KEEP` characters (when full, the oldest text is dropped, because a
//! server's recent output is what matters). `output` returns what arrived
//! since the previous `output` call for that id, at most `READ_MAX`
//! characters (the newest when more arrived, after a first line saying how
//! many were skipped). The text kept for the user is the whole ring.
//!
//! Process handling is `run`'s: the shared shell, scrubbed environment,
//! output cleaner and spawn; on macOS and Linux its own session with SIGTERM
//! to the group, SIGKILL after 500 ms, the leader reaped only after the group
//! was signalled; on Windows a Job Object. When the command's own process
//! exits, whatever it left running in its group (or job) is stopped too, so a
//! command that starts a server in the background of its shell and returns
//! ends that server with it.
//!
//! Stopping is deliberate on every path: `stop`, `stop_chat` and `stop_all`
//! each return within about two seconds; the app calls `stop_all` when it
//! exits (see `lib.rs`); dropping the registry kills every remaining group as
//! a last resort, and so does the end of the runtime (each supervisor holds a
//! `GroupGuard`). On Windows the job also ends when the app's handle closes
//! (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), so even a killed app leaves nothing
//! running. On macOS and Linux a hard kill of the app (SIGKILL, power loss)
//! runs no code at all and leaves background commands running; a process
//! that moves itself to another session (`setsid`, daemonizing) is not
//! reached by a stop, as for foreground commands.

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use process_wrap::tokio::ChildWrapper;
use serde::Serialize;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use super::run::{DRAIN_AFTER_STOP, EXIT_GRACE};
#[cfg(unix)]
use super::run::{GroupGuard, GroupSlot};
use super::shell::Shell;
use crate::state::EventSink;

/// Commands running at once in one chat.
const MAX_RUNNING: usize = 4;
/// Characters of output kept per command.
const KEEP: usize = 200_000;
/// Characters returned by one `output` call, notice included.
const READ_MAX: usize = 12_000;
/// A stop that has not finished by now is forced.
const STOP_WITHIN: Duration = Duration::from_secs(2);
/// The event the interface refreshes its list on; the payload is the chat id.
pub const CHANGED_EVENT: &str = "background-changed";

pub const FIFTH: &str =
    "Four background commands are already running in this chat. Stop one first.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BgInfo {
    pub id: String,
    /// The command text as given.
    pub command: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    /// Unix time, seconds.
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BgRead {
    pub new_text: String,
    pub running: bool,
    pub exit_code: Option<i32>,
}

/// The newest `keep` characters of a stream and a read position in it.
pub(crate) struct Ring {
    buf: String,
    chars: usize,
    keep: usize,
    /// Characters ever pushed.
    total: u64,
    /// `total` at the last `take_new`.
    read_to: u64,
}

impl Ring {
    pub(crate) fn new(keep: usize) -> Self {
        Self {
            buf: String::new(),
            chars: 0,
            keep,
            total: 0,
            read_to: 0,
        }
    }

    pub(crate) fn push(&mut self, text: &str) {
        let n = text.chars().count();
        self.total += n as u64;
        self.buf.push_str(text);
        self.chars += n;
        if self.chars > self.keep {
            let at = self.byte_of(self.chars - self.keep);
            self.buf.drain(..at);
            self.chars = self.keep;
        }
    }

    /// Byte offset of the `n`th character of the buffer.
    fn byte_of(&self, n: usize) -> usize {
        self.buf
            .char_indices()
            .nth(n)
            .map_or(self.buf.len(), |(i, _)| i)
    }

    /// Everything kept.
    pub(crate) fn all(&self) -> String {
        self.buf.clone()
    }

    /// What arrived since the last call, at most `max` characters: the newest
    /// when more arrived, after a first line saying how many were skipped
    /// (cut here, or already dropped from the ring).
    pub(crate) fn take_new(&mut self, max: usize) -> String {
        let oldest = self.total - self.chars as u64;
        let from = self.read_to.max(oldest);
        let dropped = from - self.read_to;
        let new_chars = (self.total - from) as usize;
        self.read_to = self.total;
        let mut take = new_chars.min(max);
        let mut notice = String::new();
        // The notice is part of the budget, and its length depends on the count.
        while dropped + (new_chars - take) as u64 > 0 {
            let skipped = dropped + (new_chars - take) as u64;
            notice = format!("[… {skipped} characters skipped …]\n");
            let room = max.saturating_sub(notice.chars().count());
            if take <= room {
                break;
            }
            take = room.min(new_chars);
        }
        let start = self.byte_of(self.chars - take);
        format!("{notice}{}", &self.buf[start..])
    }
}

struct State {
    ring: Ring,
    running: bool,
    exit_code: Option<i32>,
}

struct Shared {
    state: Mutex<State>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What stopping a command needs, cloneable so the registry lock is never
/// held while waiting for a stop.
#[derive(Clone)]
struct Ctl {
    shared: Arc<Shared>,
    stop: CancellationToken,
    /// `true` once the supervisor has stopped the tree and finished.
    done: watch::Receiver<bool>,
    abort: AbortHandle,
    /// Lets the registry send the last-resort SIGKILL itself.
    #[cfg(unix)]
    slot: GroupSlot,
}

impl Ctl {
    /// The last resort: kill the group (unix) and end the supervisor, whose
    /// dropped child closes the Job Object (Windows).
    fn kill_now(&self) {
        #[cfg(unix)]
        self.slot.kill();
        self.abort.abort();
        self.shared.state().running = false;
    }

    /// Asks the supervisor to stop the tree and waits for it, forcing the end
    /// if it takes longer than `STOP_WITHIN`. Returns whether it was running.
    async fn stop(&self) -> bool {
        let was_running = self.shared.state().running;
        self.stop.cancel();
        let mut done = self.done.clone();
        if tokio::time::timeout(STOP_WITHIN, done.wait_for(|d| *d))
            .await
            .is_err()
        {
            self.kill_now();
        }
        was_running
    }
}

struct Entry {
    id: String,
    command: String,
    started_at: i64,
    ctl: Ctl,
}

impl Entry {
    fn running(&self) -> bool {
        self.ctl.shared.state().running
    }

    fn info(&self) -> BgInfo {
        let st = self.ctl.shared.state();
        BgInfo {
            id: self.id.clone(),
            command: self.command.clone(),
            running: st.running,
            exit_code: st.exit_code,
            started_at: self.started_at,
        }
    }
}

#[derive(Default)]
struct Chat {
    /// Ids handed out so far; never goes back, so an id is not reused.
    issued: u32,
    commands: Vec<Entry>,
}

pub struct Background {
    chats: Mutex<HashMap<String, Chat>>,
    events: Arc<dyn EventSink>,
}

impl Background {
    pub fn new(events: Arc<dyn EventSink>) -> Self {
        Self {
            chats: Mutex::new(HashMap::new()),
            events,
        }
    }

    fn chats(&self) -> MutexGuard<'_, HashMap<String, Chat>> {
        self.chats.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self, chat_id: &str) {
        self.events.emit(CHANGED_EVENT, json!(chat_id));
    }

    /// Starts `command` in `root` and returns at once. Refused (nothing
    /// started) when the chat already runs four or the folder is missing.
    pub async fn start(
        &self,
        chat_id: &str,
        shell: &Shell,
        root: &Path,
        command: &str,
    ) -> Result<BgInfo, String> {
        if !root.is_dir() {
            return Err(format!(
                "The project folder {} does not exist, so the command was not run.",
                root.display()
            ));
        }
        let info = {
            let mut chats = self.chats();
            let chat = chats.entry(chat_id.to_string()).or_default();
            if chat.commands.iter().filter(|c| c.running()).count() >= MAX_RUNNING {
                return Err(FIFTH.to_string());
            }
            let child = super::run::spawn(shell, root, command)?;
            chat.issued += 1;
            let id = format!("bg{}", chat.issued);
            let shared = Arc::new(Shared {
                state: Mutex::new(State {
                    ring: Ring::new(KEEP),
                    running: true,
                    exit_code: None,
                }),
            });
            let stop = CancellationToken::new();
            let (done_tx, done_rx) = watch::channel(false);
            #[cfg(unix)]
            let group = GroupGuard::new(child.id().and_then(|pid| i32::try_from(pid).ok()));
            #[cfg(unix)]
            let slot = group.slot();
            let task = tokio::spawn(supervise(Supervised {
                child,
                #[cfg(unix)]
                group,
                shared: shared.clone(),
                stop: stop.clone(),
                done: done_tx,
                events: self.events.clone(),
                chat_id: chat_id.to_string(),
            }));
            let entry = Entry {
                id,
                command: command.to_string(),
                started_at: now_secs(),
                ctl: Ctl {
                    shared,
                    stop,
                    done: done_rx,
                    abort: task.abort_handle(),
                    #[cfg(unix)]
                    slot,
                },
            };
            let info = entry.info();
            chat.commands.push(entry);
            info
        };
        self.changed(chat_id);
        Ok(info)
    }

    fn unknown(id: &str) -> String {
        format!("There is no background command {id} in this chat.")
    }

    /// What the command wrote since the last call. A command that has ended is
    /// dropped from the list by the read that reports it (its text is the last
    /// the model sees).
    pub fn output(&self, chat_id: &str, id: &str) -> Result<BgRead, String> {
        let mut chats = self.chats();
        let chat = chats.get_mut(chat_id).ok_or_else(|| Self::unknown(id))?;
        let at = chat
            .commands
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| Self::unknown(id))?;
        let read = {
            let mut st = chat.commands[at].ctl.shared.state();
            BgRead {
                new_text: st.ring.take_new(READ_MAX),
                running: st.running,
                exit_code: st.exit_code,
            }
        };
        if !read.running {
            chat.commands.remove(at);
        }
        Ok(read)
    }

    /// All the output kept for the user, whether read or not.
    pub fn kept(&self, chat_id: &str, id: &str) -> Result<String, String> {
        let chats = self.chats();
        let entry = chats
            .get(chat_id)
            .and_then(|c| c.commands.iter().find(|c| c.id == id))
            .ok_or_else(|| Self::unknown(id))?;
        Ok(entry.ctl.shared.state().ring.all())
    }

    pub fn list(&self, chat_id: &str) -> Vec<BgInfo> {
        self.chats()
            .get(chat_id)
            .map(|c| c.commands.iter().map(Entry::info).collect())
            .unwrap_or_default()
    }

    /// Stops one command (the whole process tree). It stays listed, ended,
    /// until read once.
    pub async fn stop(&self, chat_id: &str, id: &str) -> Result<(), String> {
        let ctl = self
            .chats()
            .get(chat_id)
            .and_then(|c| c.commands.iter().find(|c| c.id == id))
            .map(|e| e.ctl.clone())
            .ok_or_else(|| Self::unknown(id))?;
        if ctl.stop().await {
            self.changed(chat_id);
        }
        Ok(())
    }

    pub async fn stop_chat(&self, chat_id: &str) {
        let taken = self
            .chats()
            .get_mut(chat_id)
            .map(|c| std::mem::take(&mut c.commands))
            .unwrap_or_default();
        if taken.is_empty() {
            return;
        }
        stop_entries(taken).await;
        self.changed(chat_id);
    }

    pub async fn stop_all(&self) {
        let taken: Vec<(String, Vec<Entry>)> = self
            .chats()
            .iter_mut()
            .map(|(chat, c)| (chat.clone(), std::mem::take(&mut c.commands)))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let chats: Vec<String> = taken.iter().map(|(c, _)| c.clone()).collect();
        stop_entries(taken.into_iter().flat_map(|(_, v)| v).collect()).await;
        for chat in chats {
            self.changed(&chat);
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        let chats = self.chats.get_mut().unwrap_or_else(|e| e.into_inner());
        for entry in chats.values().flat_map(|c| c.commands.iter()) {
            entry.ctl.kill_now();
        }
    }
}

/// Stops all of them at the same time.
async fn stop_entries(entries: Vec<Entry>) {
    futures_util::future::join_all(entries.iter().map(|e| e.ctl.stop())).await;
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

struct Supervised {
    child: Box<dyn ChildWrapper>,
    #[cfg(unix)]
    group: GroupGuard,
    shared: Arc<Shared>,
    stop: CancellationToken,
    done: watch::Sender<bool>,
    events: Arc<dyn EventSink>,
    chat_id: String,
}

enum End {
    Exited,
    Stopped,
}

/// Owns the process: collects its output until it exits or is stopped, then
/// ends its whole tree and records how it ended.
async fn supervise(mut s: Supervised) {
    let (tx, mut rx) = mpsc::channel::<String>(32);
    let mut readers = Vec::new();
    if let Some(pipe) = s.child.stdout().take() {
        readers.push(tokio::spawn(super::run::pump(pipe, tx.clone())));
    }
    if let Some(pipe) = s.child.stderr().take() {
        readers.push(tokio::spawn(super::run::pump(pipe, tx.clone())));
    }
    drop(tx);

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
        tokio::select! {
            biased;
            _ = s.stop.cancelled() => break End::Stopped,
            _ = tick.tick() => {
                if exited_at.is_none() && super::run::leader_ended(&mut *s.child, &mut status) {
                    exited_at = Some(Instant::now());
                }
            }
            msg = rx.recv(), if !closed => match msg {
                Some(text) => s.shared.state().ring.push(&text),
                None => closed = true,
            },
        }
    };

    let natural = matches!(end, End::Exited);
    #[cfg(unix)]
    let reaped = super::run::stop_tree(&mut *s.child, &mut s.group, !natural).await;
    #[cfg(windows)]
    let reaped = super::run::stop_tree(&mut *s.child, !natural).await;
    status = status.or(reaped);

    if !closed {
        let until = tokio::time::Instant::now() + DRAIN_AFTER_STOP;
        while let Ok(Some(text)) = tokio::time::timeout_at(until, rx.recv()).await {
            s.shared.state().ring.push(&text);
        }
    }
    for reader in &readers {
        reader.abort();
    }

    let mut exit_code = None;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut note: Option<String> = None;
    if natural && let Some(st) = status {
        exit_code = st.code();
        #[cfg(unix)]
        if exit_code.is_none() {
            use std::os::unix::process::ExitStatusExt;
            if let Some(sig) = st.signal() {
                note = Some(format!("killed by signal {sig}\n"));
            }
        }
    }
    {
        let mut st = s.shared.state();
        if let Some(note) = note {
            if !st.ring.all().ends_with('\n') && st.ring.total > 0 {
                st.ring.push("\n");
            }
            st.ring.push(&note);
        }
        st.exit_code = exit_code;
        st.running = false;
    }
    let _ = s.done.send(true);
    if natural {
        s.events.emit(CHANGED_EVENT, json!(s.chat_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::rules::ShellFamily;
    use crate::agent::shell::detect;
    use crate::agent::test_support::{alive, long, pick, wait_for_pid_file, wait_gone};
    use serde_json::{Value, json};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Events(Mutex<Vec<(String, Value)>>);
    impl EventSink for Events {
        fn emit(&self, event: &str, payload: Value) {
            self.0.lock().unwrap().push((event.to_string(), payload));
        }
    }
    impl Events {
        fn count(&self, chat: &str) -> usize {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, p)| n == "background-changed" && *p == json!(chat))
                .count()
        }
    }

    fn setup() -> (Background, Arc<Events>, tempfile::TempDir) {
        let events = Arc::new(Events::default());
        (
            Background::new(events.clone()),
            events,
            tempfile::tempdir().unwrap(),
        )
    }

    /// Prints a line every 0.2 s until stopped.
    fn ticker(shell: &Shell) -> &'static str {
        pick(
            shell,
            "while true; do echo tick; sleep 0.2; done",
            "while ($true) { 'tick'; Start-Sleep -Milliseconds 200 }",
        )
    }

    /// Reads until `done` holds for the accumulated text, or fails.
    async fn read_until(
        bg: &Background,
        chat: &str,
        id: &str,
        done: impl Fn(&str, &BgRead) -> bool,
    ) -> (String, BgRead) {
        let mut text = String::new();
        let deadline = Instant::now() + Duration::from_secs(long(20));
        loop {
            let r = bg.output(chat, id).unwrap();
            text.push_str(&r.new_text);
            if done(&text, &r) {
                return (text, r);
            }
            assert!(Instant::now() < deadline, "gave up; got {text:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// A command whose grandchild writes its pid, then sleeps.
    fn with_grandchild(shell: &Shell, dir: &Path) -> &'static str {
        match shell.family {
            ShellFamily::Posix => {
                std::fs::write(dir.join("child.sh"), "echo $$ > pid.txt\nexec sleep 30\n").unwrap();
                "sh ./child.sh & wait"
            }
            ShellFamily::PowerShell => {
                "$p = Start-Process -NoNewWindow -PassThru powershell -ArgumentList '-NoProfile','-Command','Set-Content -Path pid.txt -Value $PID; Start-Sleep 60'; $p.WaitForExit()"
            }
        }
    }

    // ---- the ring ----------------------------------------------------------

    #[test]
    fn ring_returns_only_what_is_new_and_keeps_the_newest() {
        let mut r = Ring::new(10);
        r.push("abc");
        assert_eq!(r.take_new(100), "abc");
        assert_eq!(r.take_new(100), "");
        r.push("defgh");
        assert_eq!(r.take_new(100), "defgh");
        assert_eq!(r.all(), "abcdefgh");
        r.push("ijklmn");
        assert_eq!(r.all(), "efghijklmn", "oldest text is dropped");
    }

    #[test]
    fn ring_says_how_many_characters_were_skipped() {
        let mut r = Ring::new(10);
        r.push("0123456789ABCDEF"); // 6 fell off the ring before any read
        let text = r.take_new(1_000);
        assert_eq!(text, "[… 6 characters skipped …]\n6789ABCDEF");
        // More arrived than the read may return: the newest, with the notice.
        let mut r = Ring::new(1_000);
        r.push(&"x".repeat(50));
        r.push("END");
        let text = r.take_new(40);
        assert!(text.chars().count() <= 40, "{text:?}");
        assert!(text.starts_with("[… ") && text.contains(" characters skipped …]\n"));
        assert!(text.ends_with("xxEND"), "{text:?}");
        assert_eq!(r.take_new(40), "");
    }

    #[test]
    fn ring_counts_characters_not_bytes() {
        let mut r = Ring::new(4);
        r.push("é✓é✓é");
        assert_eq!(r.all(), "✓é✓é");
    }

    // ---- start, read, exit -------------------------------------------------

    #[tokio::test]
    async fn start_returns_bg1_at_once_while_it_runs_and_output_has_its_text() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        let t = Instant::now();
        let info = bg
            .start("c1", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap();
        assert!(t.elapsed() < Duration::from_secs(long(5)));
        assert_eq!(info.id, "bg1");
        assert_eq!(info.command, ticker(&shell));
        assert!(info.running && info.exit_code.is_none());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((now - info.started_at).abs() < 30);
        let (text, r) = read_until(&bg, "c1", "bg1", |t, _| t.contains("tick")).await;
        assert!(text.contains("tick"));
        assert!(r.running && r.exit_code.is_none());
        bg.stop_all().await;
    }

    #[tokio::test]
    async fn a_second_read_returns_only_new_text() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        let cmd = pick(
            &shell,
            "echo first; sleep 1.5; echo second; sleep 30",
            "'first'; Start-Sleep -Milliseconds 1500; 'second'; Start-Sleep 60",
        );
        bg.start("c1", &shell, dir.path(), cmd).await.unwrap();
        let (t1, _) = read_until(&bg, "c1", "bg1", |t, _| t.contains("first")).await;
        assert!(!t1.contains("second"));
        let (t2, _) = read_until(&bg, "c1", "bg1", |t, _| t.contains("second")).await;
        assert!(!t2.contains("first"), "{t2:?}");
        bg.stop_all().await;
    }

    #[tokio::test]
    async fn a_command_that_exits_shows_running_false_and_its_code() {
        let (bg, ev, dir) = setup();
        let shell = detect();
        bg.start("c1", &shell, dir.path(), "echo done; exit 3")
            .await
            .unwrap();
        let (text, r) = read_until(&bg, "c1", "bg1", |_, r| !r.running).await;
        assert!(text.contains("done"), "{text:?}");
        assert_eq!(r.exit_code, Some(3));
        // start and exit each told the interface once.
        let t = Instant::now();
        while ev.count("c1") < 2 && t.elapsed() < Duration::from_secs(3) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(ev.count("c1"), 2);
    }

    #[tokio::test]
    async fn a_command_that_cannot_run_still_gets_an_id_and_its_error_is_read() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        let info = bg
            .start("c1", &shell, dir.path(), "no-such-command-zzq-beluga")
            .await
            .unwrap();
        assert_eq!(info.id, "bg1");
        let (text, r) = read_until(&bg, "c1", "bg1", |_, r| !r.running).await;
        assert!(!text.trim().is_empty(), "the error text is shown");
        assert!(
            matches!(r.exit_code, Some(c) if c != 0),
            "{:?}",
            r.exit_code
        );
    }

    #[tokio::test]
    async fn a_finished_command_is_listed_until_read_once_and_its_id_is_not_reused() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        bg.start("c1", &shell, dir.path(), "echo hi").await.unwrap();
        // Wait for the end without reading.
        let t = Instant::now();
        while bg.list("c1")[0].running {
            assert!(t.elapsed() < Duration::from_secs(long(20)));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let listed = bg.list("c1");
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].running && listed[0].exit_code == Some(0));
        let r = bg.output("c1", "bg1").unwrap();
        assert!(!r.running && r.new_text.contains("hi"));
        assert!(bg.list("c1").is_empty(), "read once after finishing");
        assert!(bg.output("c1", "bg1").is_err());
        let again = bg
            .start("c1", &shell, dir.path(), "echo again")
            .await
            .unwrap();
        assert_eq!(again.id, "bg2", "ids are never reused");
        let other = bg.start("c2", &shell, dir.path(), "echo x").await.unwrap();
        assert_eq!(other.id, "bg1", "ids are per chat");
    }

    #[tokio::test]
    async fn output_for_an_unknown_id_is_a_plain_error() {
        let (bg, _ev, _dir) = setup();
        let e = bg.output("c1", "bg9").unwrap_err();
        assert!(e.contains("bg9"), "{e}");
        assert!(bg.kept("c1", "bg9").is_err());
        assert!(bg.stop("c1", "bg9").await.is_err());
        assert!(bg.list("c1").is_empty());
    }

    #[tokio::test]
    async fn a_missing_project_root_starts_nothing() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        let gone = dir.path().join("not-there");
        let e = bg.start("c1", &shell, &gone, "echo hi").await.unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
        assert!(bg.list("c1").is_empty());
        assert!(!gone.exists());
    }

    // ---- limits on output --------------------------------------------------

    #[tokio::test]
    async fn long_output_is_cut_to_the_newest_twelve_thousand_and_kept_up_to_two_hundred_thousand()
    {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        let cmd = pick(
            &shell,
            "i=1; while [ $i -le 40000 ]; do echo line$i; i=$((i+1)); done",
            "1..40000 | ForEach-Object { \"line$_\" }",
        );
        bg.start("c1", &shell, dir.path(), cmd).await.unwrap();
        let t = Instant::now();
        while bg.list("c1")[0].running {
            assert!(t.elapsed() < Duration::from_secs(long(30)));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let kept = bg.kept("c1", "bg1").unwrap();
        assert!(kept.chars().count() <= 200_000, "{}", kept.chars().count());
        assert!(kept.chars().count() > 190_000);
        assert!(kept.ends_with("line40000\n"));
        assert!(!kept.starts_with("line1\n"), "the oldest text was dropped");
        let r = bg.output("c1", "bg1").unwrap();
        assert!(r.new_text.chars().count() <= 12_000);
        assert!(r.new_text.starts_with("[… "), "{:?}", &r.new_text[..40]);
        assert!(
            r.new_text
                .lines()
                .next()
                .unwrap()
                .ends_with("characters skipped …]")
        );
        assert!(r.new_text.ends_with("line40000\n"));
        assert!(!r.running && r.exit_code == Some(0));
    }

    // ---- limits on count ---------------------------------------------------

    #[tokio::test]
    async fn the_fifth_command_in_a_chat_is_refused_with_the_exact_message() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        for n in 1..=4 {
            let info = bg
                .start("c1", &shell, dir.path(), ticker(&shell))
                .await
                .unwrap();
            assert_eq!(info.id, format!("bg{n}"));
        }
        let e = bg
            .start("c1", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap_err();
        assert_eq!(
            e,
            "Four background commands are already running in this chat. Stop one first."
        );
        assert_eq!(bg.list("c1").len(), 4);
        // Another chat is unaffected.
        assert_eq!(
            bg.start("c2", &shell, dir.path(), ticker(&shell))
                .await
                .unwrap()
                .id,
            "bg1"
        );
        // Stopping one makes room.
        bg.stop("c1", "bg2").await.unwrap();
        let fifth = bg
            .start("c1", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap();
        assert_eq!(fifth.id, "bg5");
        bg.stop_all().await;
    }

    #[tokio::test]
    async fn finished_commands_do_not_count_toward_the_four() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        for _ in 0..4 {
            bg.start("c1", &shell, dir.path(), "echo hi").await.unwrap();
        }
        let t = Instant::now();
        while bg.list("c1").iter().any(|i| i.running) {
            assert!(t.elapsed() < Duration::from_secs(long(20)));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(bg.start("c1", &shell, dir.path(), "echo hi").await.is_ok());
    }

    // ---- stopping ----------------------------------------------------------

    #[tokio::test]
    async fn stop_ends_the_command_and_its_children_within_two_seconds() {
        let (bg, ev, dir) = setup();
        let shell = detect();
        let cmd = with_grandchild(&shell, dir.path());
        bg.start("c1", &shell, dir.path(), cmd).await.unwrap();
        let pid = wait_for_pid_file(&dir.path().join("pid.txt")).await;
        let before = ev.count("c1");
        let t = Instant::now();
        bg.stop("c1", "bg1").await.unwrap();
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert!(wait_gone(pid).await, "grandchild {pid} is still running");
        let info = &bg.list("c1")[0];
        assert!(!info.running && info.exit_code.is_none());
        assert_eq!(
            ev.count("c1"),
            before + 1,
            "stopping tells the interface once"
        );
        // Stopping what already stopped is harmless, and silent.
        assert!(bg.stop("c1", "bg1").await.is_ok());
        assert_eq!(ev.count("c1"), before + 1);
        let r = bg.output("c1", "bg1").unwrap();
        assert!(!r.running && r.exit_code.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_kills_what_ignores_sigterm_within_two_seconds() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        let cmd = "sh -c 'trap \"\" TERM; echo $$ > a.pid; exec sleep 74' & \
                   trap \"\" TERM; echo $$ > b.pid; sleep 73 & echo $! > c.pid; wait";
        bg.start("c1", &shell, dir.path(), cmd).await.unwrap();
        let a = wait_for_pid_file(&dir.path().join("a.pid")).await;
        let b = wait_for_pid_file(&dir.path().join("b.pid")).await;
        let c = wait_for_pid_file(&dir.path().join("c.pid")).await;
        let t = Instant::now();
        bg.stop("c1", "bg1").await.unwrap();
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        for pid in [a, b, c] {
            assert!(wait_gone(pid).await, "process {pid} is still running");
        }
    }

    #[tokio::test]
    async fn stop_chat_stops_only_that_chats_commands_and_clears_them() {
        let (bg, ev, dir) = setup();
        let shell = detect();
        let c1 = with_grandchild(&shell, dir.path());
        bg.start("c1", &shell, dir.path(), c1).await.unwrap();
        let pid = wait_for_pid_file(&dir.path().join("pid.txt")).await;
        bg.start("c1", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap();
        bg.start("c2", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap();
        let before = ev.count("c1");
        let t = Instant::now();
        bg.stop_chat("c1").await;
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert!(wait_gone(pid).await);
        assert!(bg.list("c1").is_empty(), "the chat's set is cleared");
        assert_eq!(ev.count("c1"), before + 1, "one event for the whole chat");
        let other = bg.list("c2");
        assert_eq!(other.len(), 1);
        assert!(other[0].running);
        let (text, _) = read_until(&bg, "c2", "bg1", |t, _| t.contains("tick")).await;
        assert!(text.contains("tick"));
        // A new command in the cleared chat does not reuse an id.
        let again = bg
            .start("c1", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap();
        assert_eq!(again.id, "bg3");
        bg.stop_all().await;
    }

    #[tokio::test]
    async fn stop_all_stops_every_chat() {
        let (bg, _ev, dir) = setup();
        let shell = detect();
        let c = with_grandchild(&shell, dir.path());
        bg.start("c1", &shell, dir.path(), c).await.unwrap();
        let pid = wait_for_pid_file(&dir.path().join("pid.txt")).await;
        bg.start("c2", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap();
        bg.start("c3", &shell, dir.path(), ticker(&shell))
            .await
            .unwrap();
        let t = Instant::now();
        bg.stop_all().await;
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert!(wait_gone(pid).await);
        for chat in ["c1", "c2", "c3"] {
            assert!(bg.list(chat).iter().all(|i| !i.running), "{chat}");
        }
    }

    #[tokio::test]
    async fn dropping_the_registry_kills_every_remaining_command() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let bg = Background::new(Arc::new(Events::default()));
        let c = with_grandchild(&shell, dir.path());
        bg.start("c1", &shell, dir.path(), c).await.unwrap();
        let pid = wait_for_pid_file(&dir.path().join("pid.txt")).await;
        assert!(alive(pid));
        drop(bg);
        assert!(
            wait_gone(pid).await,
            "grandchild {pid} survived the registry"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_the_registry_kills_what_ignores_sigterm_at_once() {
        let shell = detect();
        let dir = tempfile::tempdir().unwrap();
        let bg = Background::new(Arc::new(Events::default()));
        let cmd = "trap \"\" TERM; echo $$ > b.pid; sleep 73 & echo $! > c.pid; wait";
        bg.start("c1", &shell, dir.path(), cmd).await.unwrap();
        let b = wait_for_pid_file(&dir.path().join("b.pid")).await;
        let c = wait_for_pid_file(&dir.path().join("c.pid")).await;
        drop(bg);
        for pid in [b, c] {
            assert!(wait_gone(pid).await, "process {pid} survived");
        }
    }
}
