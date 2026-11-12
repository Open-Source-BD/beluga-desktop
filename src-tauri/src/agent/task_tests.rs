//! The task loop against the scripted engine, the real app state with an
//! in-memory store, a real temp project folder and the real dispatcher.
//! Cards are answered the way the interface answers them: after
//! `ApprovalNeeded` arrives on the event stream.

use super::*;
use crate::agent::shell::detect;
use crate::agent::test_support::{long, pick};
use crate::agent::{Answer, ToolCall};
use crate::agent::{prompt, schema};
use crate::commands::chat_state::{self, SKILL_NOTE};
use crate::commands::turn::{self, TurnResult, TurnStatus};
use crate::commands::{chats, compact, folder};
use crate::engine::scripted::{ScriptedEngine, Step};
use crate::engine::{EngineEvent, StepError, StepOutcome, StepRequest, UiError};
use crate::openrouter::error::ApiError;
use crate::openrouter::key_info::{FreeDaily, KeyStatus};
use crate::openrouter::models::{ModelCache, ModelInfo};
use crate::openrouter::stream::{Part, Role, WireMessage};
use crate::secrets::{KeyStore, SessionStore};
use crate::state::{AppState, EventSink, HttpClients, HttpTimeouts};
use crate::store::{Store, StoredMessage};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOOLS_MODEL: &str = "m/tools";
const PLAIN_MODEL: &str = "m/plain";
const SMALL_MODEL: &str = "m/small";
const PAID_TOOLS: &str = "m/paid-tools";
const PAID_PLAIN: &str = "m/paid-plain";

/// The key's status and the clock as the guards see them: statuses are
/// handed out in order, then `fallback` for every later fetch; a wait of
/// `n` seconds takes `n × ms_per_second` milliseconds.
#[derive(Default)]
struct ScriptedOutside {
    statuses: Mutex<std::collections::VecDeque<Option<KeyStatus>>>,
    fallback: Mutex<Option<KeyStatus>>,
    fetches: std::sync::atomic::AtomicUsize,
    waits: Mutex<Vec<u64>>,
    ms_per_second: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl crate::agent::guards::Outside for ScriptedOutside {
    async fn key_status(&self, _: &AppState) -> Option<KeyStatus> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let next = self.statuses.lock().unwrap().pop_front();
        next.unwrap_or_else(|| self.fallback.lock().unwrap().clone())
    }

    async fn wait(&self, seconds: u64) {
        self.waits.lock().unwrap().push(seconds);
        let ms = self.ms_per_second.load(std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(seconds * ms)).await;
    }
}

fn free_left(remaining: u32) -> Option<KeyStatus> {
    Some(KeyStatus {
        is_free_tier: true,
        free_daily: Some(FreeDaily {
            used: 50 - remaining.min(50),
            limit: 50,
            remaining,
        }),
        usage_usd: 0.0,
        limit_remaining_usd: None,
    })
}

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: &str, _: Value) {}
}

struct NoDialogs;
impl crate::dialogs::Dialogs for NoDialogs {
    fn pick_folder(&self) -> Option<PathBuf> {
        None
    }
    fn save_markdown(&self, _: &str) -> Option<PathBuf> {
        None
    }
    fn pick_zip(&self) -> Option<PathBuf> {
        None
    }
}

type Events = Arc<Mutex<Vec<EngineEvent>>>;
type Hook = Arc<Mutex<Option<Box<dyn Fn(&EngineEvent) + Send + Sync>>>>;

struct Fx {
    state: AppState,
    engine: Arc<ScriptedEngine>,
    chat: String,
    project: tempfile::TempDir,
    _data: tempfile::TempDir,
    events: Events,
    hook: Hook,
    /// Cards answered so far by `answer_cards`.
    answered: std::sync::atomic::AtomicUsize,
    outside: Arc<ScriptedOutside>,
}

fn paid(id: &str, tools: bool) -> ModelInfo {
    ModelInfo {
        prompt_usd_per_mtok: 1.0,
        completion_usd_per_mtok: 2.0,
        is_free: false,
        ..model(id, tools, 100_000)
    }
}

fn model(id: &str, tools: bool, context_length: u64) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: id.into(),
        prompt_usd_per_mtok: 0.0,
        completion_usd_per_mtok: 0.0,
        is_free: true,
        supports_tools: tools,
        accepts_images: true,
        context_length,
    }
}

/// A built-in pack `p` with one skill `a`, on by default.
fn write_skills(root: &std::path::Path) {
    let pack = root.join("p");
    let dir = pack.join("skills").join("a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: a\ndescription: d\n---\nDo a.",
    )
    .unwrap();
    let manifest = json!({
        "id": "p", "title": "P", "repo": "o/p", "commit": "abc", "license": "MIT",
        "license_files": [], "skipped": [], "checksum": "x",
        "skills": [{
            "id": "p:a", "pack": "p", "name": "a", "description": "d",
            "dir": "skills/a", "acts_on_computer": true, "body_chars": 5,
            "files": [], "license": null, "broken": null,
        }],
    });
    std::fs::write(pack.join("pack.json"), manifest.to_string()).unwrap();
    std::fs::write(
        root.join("default-skills.json"),
        json!({"on": ["p:a"]}).to_string(),
    )
    .unwrap();
}

fn real(p: &std::path::Path) -> String {
    let s = std::fs::canonicalize(p)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

/// The folder string the app stores when the user picks `p` in the folder
/// dialog (`commands/folder.rs` `real_folder`): the canonical path as is,
/// which on Windows keeps the `\\?\` prefix and the long names.
fn picked(p: &std::path::Path) -> String {
    std::fs::canonicalize(p)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

fn fx(steps: Vec<Step>) -> Fx {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.txt"), "hello\n").unwrap();
    let builtin = data.path().join("builtin");
    write_skills(&builtin);
    let engine = Arc::new(ScriptedEngine::with_steps(steps));
    let keys = SessionStore::default();
    keys.set("sk-or-v1-test").unwrap();
    let http = HttpClients::from_builder(
        || reqwest::Client::builder().no_proxy(),
        HttpTimeouts::PRODUCTION,
    )
    .unwrap();
    let mut state = AppState::with_engine(
        http,
        "http://127.0.0.1:1".into(),
        Arc::new(keys),
        Store::in_memory().unwrap(),
        data.path().join("app"),
        builtin,
        Arc::new(NullSink),
        Arc::new(NoDialogs),
        engine.clone(),
    );
    std::fs::create_dir_all(data.path().join("app")).unwrap();
    ModelCache::save(
        &state.model_cache_file(),
        &[
            model(TOOLS_MODEL, true, 100_000),
            model(PLAIN_MODEL, false, 100_000),
            model(SMALL_MODEL, true, 10_000),
            paid(PAID_TOOLS, true),
            paid(PAID_PLAIN, false),
        ],
    )
    .unwrap();
    let outside = Arc::new(ScriptedOutside {
        ms_per_second: 1.into(),
        ..ScriptedOutside::default()
    });
    state.outside = outside.clone();
    let chat = state.store.create_chat(TOOLS_MODEL).unwrap().id;
    let root = real(project.path());
    state.store.approve_folder(&root).unwrap();
    state.store.set_folder(&chat, Some(&root)).unwrap();
    Fx {
        state,
        engine,
        chat,
        project,
        _data: data,
        events: Arc::default(),
        hook: Arc::default(),
        answered: Default::default(),
        outside,
    }
}

impl Fx {
    fn emitter(&self) -> impl Fn(EngineEvent) + Send + Sync + 'static {
        let (events, hook) = (self.events.clone(), self.hook.clone());
        move |e: EngineEvent| {
            if let Some(f) = hook.lock().unwrap().as_ref() {
                f(&e);
            }
            events.lock().unwrap().push(e);
        }
    }

    async fn send(&self, text: &str) -> TurnResult {
        self.send_on(text, TOOLS_MODEL).await
    }

    async fn send_on(&self, text: &str, model: &str) -> TurnResult {
        let emit = self.emitter();
        tokio::time::timeout(
            Duration::from_secs(long(60)),
            turn::send_message(&self.state, &self.chat, text, vec![], model, &emit),
        )
        .await
        .expect("the send hung")
        .expect("the send was refused")
    }

    /// Runs `f` on every event as it is emitted.
    fn on_event(&self, f: impl Fn(&EngineEvent) + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }

    fn events(&self) -> Vec<EngineEvent> {
        self.events.lock().unwrap().clone()
    }

    fn cards(&self) -> usize {
        self.events()
            .iter()
            .filter(|e| matches!(e, EngineEvent::ApprovalNeeded { .. }))
            .count()
    }

    /// Waits until `pred` holds for some event.
    async fn wait_for(&self, pred: impl Fn(&EngineEvent) -> bool) {
        tokio::time::timeout(Duration::from_secs(long(30)), async {
            while !self.events().iter().any(&pred) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the awaited event never came");
    }

    /// Answers the cards not answered yet by this helper, in order, each
    /// after it is shown.
    async fn answer_cards(&self, answers: Vec<Answer>) {
        use std::sync::atomic::Ordering;
        let start = self.answered.fetch_add(answers.len(), Ordering::SeqCst);
        for (i, answer) in answers.into_iter().enumerate() {
            let id = tokio::time::timeout(Duration::from_secs(long(30)), async {
                loop {
                    let shown: Vec<String> = self
                        .events()
                        .into_iter()
                        .filter_map(|e| match e {
                            EngineEvent::ApprovalNeeded { approval } => {
                                Some(approval.id().to_string())
                            }
                            _ => None,
                        })
                        .collect();
                    if let Some(id) = shown.get(start + i) {
                        return id.clone();
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("no card was shown: {:#?}", self.events()));
            self.state
                .approvals
                .answer(&self.chat, &id, answer)
                .expect("the answer was refused");
        }
    }

    fn rows(&self) -> Vec<StoredMessage> {
        chats::get_messages(&self.state, &self.chat).unwrap()
    }

    /// Each row as `role`, `assistant[call names]`, `tool:status` or `note`.
    fn shape(&self) -> Vec<String> {
        self.rows().iter().map(shape_of).collect()
    }

    fn requests(&self) -> Vec<StepRequest> {
        self.engine.requests()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.project.path().join(name)
    }
}

fn shape_of(m: &StoredMessage) -> String {
    match m.kind.as_str() {
        "note" => "note".to_string(),
        "tool" => format!("tool:{}", m.tool_meta.as_ref().unwrap().status),
        _ if m.role == "assistant" && !m.tool_calls.is_empty() => {
            let names: Vec<&str> = m.tool_calls.iter().map(|c| c.name.as_str()).collect();
            format!("assistant[{}]", names.join(","))
        }
        _ => m.role.clone(),
    }
}

fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args.to_string(),
    }
}

fn calls(text: &str, cs: Vec<ToolCall>) -> Step {
    Step::Reply(StepOutcome {
        text: text.into(),
        tool_calls: cs,
        cost_usd: Some(0.01),
        ..StepOutcome::default()
    })
}

fn say(text: &str) -> Step {
    Step::Reply(StepOutcome {
        text: text.into(),
        cost_usd: Some(0.01),
        ..StepOutcome::default()
    })
}

fn network_error(partial: &str) -> Step {
    Step::Reply(StepOutcome {
        text: partial.into(),
        error: Some(StepError {
            ui: UiError::from(ApiError::Network("gone".into())),
            retry_after_secs: None,
        }),
        ..StepOutcome::default()
    })
}

fn overflowed(text: &str) -> Step {
    Step::Reply(StepOutcome {
        text: text.into(),
        overflowed: true,
        cost_usd: Some(0.01),
        ..StepOutcome::default()
    })
}

fn text_of(m: &WireMessage) -> String {
    m.parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            Part::ImageDataUrl(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every assistant `tool_calls` entry is followed by exactly one tool
/// message with its id before any other role, and no tool message lacks
/// its call.
fn assert_well_formed(req: &StepRequest) {
    let msgs = &req.messages;
    let mut i = 0;
    while i < msgs.len() {
        let m = &msgs[i];
        assert_ne!(
            m.role,
            Role::Tool,
            "tool message {i} has no call: {msgs:#?}"
        );
        i += 1;
        if m.role == Role::Assistant && !m.tool_calls.is_empty() {
            let mut want: Vec<&str> = m.tool_calls.iter().map(|c| c.id.as_str()).collect();
            while i < msgs.len() && msgs[i].role == Role::Tool {
                let id = msgs[i]
                    .tool_call_id
                    .as_deref()
                    .expect("a tool message without an id");
                let at = want
                    .iter()
                    .position(|w| *w == id)
                    .unwrap_or_else(|| panic!("result {id} answers no open call: {msgs:#?}"));
                want.remove(at);
                i += 1;
            }
            assert!(want.is_empty(), "calls without results {want:?}: {msgs:#?}");
        }
    }
}

/// A plain (one-request) request: no tools, no tool roles, no calls.
fn assert_plain(req: &StepRequest) {
    assert!(req.tools.is_none());
    assert!(req.session_id.is_empty());
    for m in &req.messages {
        assert_ne!(m.role, Role::Tool);
        assert!(m.tool_calls.is_empty());
    }
}

fn not_run_texts(f: &Fx) -> Vec<String> {
    f.rows()
        .into_iter()
        .filter(|m| m.tool_meta.as_ref().is_some_and(|t| t.status == "not_run"))
        .map(|m| m.text)
        .collect()
}

fn sleeper() -> &'static str {
    pick(
        &detect(),
        "echo started; sleep 30",
        "Write-Output started; Start-Sleep -Seconds 30",
    )
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_three_step_task_stores_assistant_and_tool_rows_in_order_and_returns_done() {
    let f = fx(vec![
        calls(
            "Looking.",
            vec![call("c1", "read_file", json!({"path": "a.txt"}))],
        ),
        calls(
            "",
            vec![call(
                "c2",
                "edit_file",
                json!({"path": "a.txt", "old_text": "hello", "new_text": "bye"}),
            )],
        ),
        say("Done."),
    ]);
    let (r, ()) = tokio::join!(f.send("change it"), f.answer_cards(vec![Answer::Once]));
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[read_file]",
            "tool:ok",
            "assistant[edit_file]",
            "tool:ok",
            "assistant"
        ]
    );
    let rows = f.rows();
    assert_eq!(rows[1].text, "Looking.");
    assert_eq!(rows[2].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(rows[4].tool_call_id.as_deref(), Some("c2"));
    assert_eq!(rows[5].text, "Done.");
    assert_eq!(std::fs::read_to_string(f.path("a.txt")).unwrap(), "bye\n");
    assert_eq!(f.events().last(), Some(&EngineEvent::Done));
}

#[tokio::test]
async fn the_second_request_carries_the_first_replys_calls_and_the_matching_result() {
    let f = fx(vec![
        calls("", vec![call("c1", "read_file", json!({"path": "a.txt"}))]),
        say("ok"),
    ]);
    assert_eq!(f.send("read it").await.status, TurnStatus::Done);
    let reqs = f.requests();
    assert_eq!(reqs.len(), 2);
    let second = &reqs[1].messages;
    let n = second.len();
    assert_eq!(second[n - 2].role, Role::Assistant);
    assert_eq!(
        second[n - 2].tool_calls,
        vec![call("c1", "read_file", json!({"path": "a.txt"}))]
    );
    assert_eq!(second[n - 1].role, Role::Tool);
    assert_eq!(second[n - 1].tool_call_id.as_deref(), Some("c1"));
    assert!(
        text_of(&second[n - 1]).contains("hello"),
        "{:?}",
        second[n - 1]
    );
    for r in &reqs {
        assert_well_formed(r);
    }
}

#[tokio::test]
async fn the_system_message_and_tools_are_byte_identical_across_the_tasks_requests() {
    let f = fx(vec![
        calls("", vec![call("c1", "list_dir", json!({}))]),
        calls("", vec![call("c2", "read_file", json!({"path": "a.txt"}))]),
        say("ok"),
        say("again"),
    ]);
    f.send("look").await;
    f.send("and again").await;
    let reqs = f.requests();
    assert_eq!(reqs.len(), 4);
    let first = &reqs[0];
    assert_eq!(first.messages[0].role, Role::System);
    let name = std::path::Path::new(&real(f.project.path()))
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(
        text_of(&first.messages[0]),
        prompt::system(os_name(), detect().name, &name)
    );
    assert_eq!(first.tools, Some(schema::tools()));
    for r in &reqs {
        assert_eq!(r.messages[0], first.messages[0]);
        assert_eq!(
            serde_json::to_string(&r.tools).unwrap(),
            serde_json::to_string(&first.tools).unwrap()
        );
        assert_eq!(r.session_id, f.chat);
        assert_eq!(r.model_id, TOOLS_MODEL);
    }
}

#[tokio::test]
async fn two_calls_in_one_reply_both_run_in_order_each_with_its_own_result() {
    let f = fx(vec![
        calls(
            "",
            vec![
                call("c1", "read_file", json!({"path": "a.txt"})),
                call("c2", "list_dir", json!({})),
            ],
        ),
        say("ok"),
    ]);
    f.send("go").await;
    let rows = f.rows();
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[read_file,list_dir]",
            "tool:ok",
            "tool:ok",
            "assistant"
        ]
    );
    assert_eq!(rows[2].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(rows[3].tool_call_id.as_deref(), Some("c2"));
    let started: Vec<String> = f
        .events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::ToolStarted { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect();
    assert_eq!(started, ["c1", "c2"]);
}

#[tokio::test]
async fn a_broken_arguments_call_and_an_unknown_tool_each_get_an_error_and_the_task_continues() {
    let f = fx(vec![
        calls(
            "",
            vec![
                ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\": ".into(),
                },
                call("c2", "make_coffee", json!({})),
            ],
        ),
        say("sorry"),
    ]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Done);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[read_file,make_coffee]",
            "tool:error",
            "tool:error",
            "assistant"
        ]
    );
    assert_eq!(f.requests().len(), 2);
    assert_well_formed(&f.requests()[1]);
}

#[tokio::test]
async fn stop_during_a_running_command_keeps_the_commands_result_and_the_rows_valid() {
    let f = fx(vec![
        calls(
            "",
            vec![call("c1", "run_command", json!({"command": sleeper()}))],
        ),
        say("after"),
    ]);
    let stopper = async {
        f.answer_cards(vec![Answer::Once]).await;
        f.wait_for(|e| matches!(e, EngineEvent::ToolOutput { .. }))
            .await;
        turn::stop(&f.state, &f.chat);
    };
    let started = std::time::Instant::now();
    let (r, ()) = tokio::join!(f.send("run it"), stopper);
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("stopped"));
    assert!(started.elapsed() < Duration::from_secs(long(20)));
    assert_eq!(f.shape(), ["user", "assistant[run_command]", "tool:error"]);
    let result = &f.rows()[2];
    assert_eq!(result.tool_call_id.as_deref(), Some("c1"));
    assert!(
        result
            .tool_meta
            .as_ref()
            .unwrap()
            .summary
            .contains("stopped")
    );
    assert_eq!(
        f.events().last(),
        Some(&EngineEvent::Interrupted {
            reason: "stopped".into()
        })
    );
    // The next send goes out well-formed.
    f.send("go on").await;
    assert_well_formed(f.requests().last().unwrap());
}

#[tokio::test]
async fn stop_while_an_approval_waits_records_not_run_and_changes_no_file() {
    let f = fx(vec![calls(
        "",
        vec![
            call(
                "c1",
                "write_file",
                json!({"path": "b.txt", "content": "new"}),
            ),
            call("c2", "list_dir", json!({})),
        ],
    )]);
    let stopper = async {
        f.wait_for(|e| matches!(e, EngineEvent::ApprovalNeeded { .. }))
            .await;
        turn::stop(&f.state, &f.chat);
    };
    let (r, ()) = tokio::join!(f.send("write"), stopper);
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("stopped"));
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,list_dir]",
            "tool:not_run",
            "tool:not_run"
        ]
    );
    assert_eq!(not_run_texts(&f), [STOPPED, STOPPED]);
    assert!(!f.path("b.txt").exists());
}

#[tokio::test]
async fn after_an_interrupted_task_the_next_send_first_stores_not_run_results() {
    let f = fx(vec![calls(
        "",
        vec![
            call(
                "c1",
                "write_file",
                json!({"path": "b.txt", "content": "new"}),
            ),
            call("c2", "list_dir", json!({})),
        ],
    )]);
    // The app quits while the card waits: the task's future is dropped.
    tokio::select! {
        _ = f.send("write") => panic!("the task should be waiting on the card"),
        () = f.wait_for(|e| matches!(e, EngineEvent::ApprovalNeeded { .. })) => {}
    }
    assert_eq!(f.shape(), ["user", "assistant[write_file,list_dir]"]);
    assert!(f.state.approvals.waiting(&f.chat).is_none());
    f.engine.push(vec![say("ok")]);
    assert_eq!(f.send("continue").await.status, TurnStatus::Done);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,list_dir]",
            "tool:not_run",
            "tool:not_run",
            "user",
            "assistant"
        ]
    );
    assert_eq!(not_run_texts(&f), [INTERRUPTED, INTERRUPTED]);
    // Before the crash: the user row and the assistant row. After: two
    // repaired results, the user row and the reply.
    let stored = f
        .events()
        .iter()
        .filter(|e| **e == EngineEvent::MessageStored)
        .count();
    assert_eq!(stored, 2 + 4);
    let last = f.requests().pop().unwrap();
    assert_well_formed(&last);
    assert_eq!(text_of(last.messages.last().unwrap()), "continue");
    assert!(!f.path("b.txt").exists());
}

#[tokio::test]
async fn a_message_answer_stores_the_decline_then_the_user_row_and_marks_later_calls_not_run() {
    let f = fx(vec![
        calls(
            "",
            vec![
                call(
                    "c1",
                    "write_file",
                    json!({"path": "b.txt", "content": "new"}),
                ),
                call("c2", "read_file", json!({"path": "a.txt"})),
            ],
        ),
        say("ok, c.txt then"),
    ]);
    let (r, ()) = tokio::join!(
        f.send("write"),
        f.answer_cards(vec![Answer::Message {
            text: "use c.txt instead".into()
        }])
    );
    assert_eq!(r.status, TurnStatus::Done);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,read_file]",
            "tool:declined",
            "tool:not_run",
            "user",
            "assistant"
        ]
    );
    let rows = f.rows();
    assert_eq!(rows[3].text, USER_SENT);
    assert_eq!(rows[4].text, "use c.txt instead");
    assert_eq!(rows[4].kind, "message");
    let second = &f.requests()[1];
    assert_well_formed(second);
    let last = second.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(text_of(last), "use c.txt instead");
    assert!(!f.path("b.txt").exists());
    let stored = f
        .events()
        .iter()
        .filter(|e| **e == EngineEvent::MessageStored)
        .count();
    // the first user row, assistant, two results, the user row, the final reply
    assert_eq!(stored, 6);
}

#[tokio::test]
async fn a_network_error_on_step_three_returns_interrupted_and_keeps_steps_one_and_two() {
    let f = fx(vec![
        calls("", vec![call("c1", "read_file", json!({"path": "a.txt"}))]),
        calls("", vec![call("c2", "list_dir", json!({}))]),
        network_error(""),
    ]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("network"));
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[read_file]",
            "tool:ok",
            "assistant[list_dir]",
            "tool:ok"
        ]
    );
    f.engine.push(vec![say("ok")]);
    f.send("continue").await;
    assert_well_formed(f.requests().last().unwrap());
}

#[tokio::test]
async fn a_step_error_before_any_work_fails_as_today_and_retry_runs_the_task() {
    let f = fx(vec![network_error(""), say("ok")]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Failed);
    assert_eq!(r.error.unwrap().kind, "network");
    assert_eq!(f.shape(), ["user"]);
    let emit = f.emitter();
    let r = turn::retry_turn(&f.state, &f.chat, TOOLS_MODEL, false, &emit)
        .await
        .unwrap();
    assert_eq!(r.status, TurnStatus::Done);
    assert_eq!(f.shape(), ["user", "assistant"]);
    assert!(f.requests()[1].tools.is_some());
}

#[tokio::test]
async fn a_partial_reply_cut_by_an_error_is_kept_as_interrupted() {
    let f = fx(vec![network_error("Half a")]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Interrupted);
    let rows = f.rows();
    assert_eq!(f.shape(), ["user", "assistant"]);
    assert_eq!(rows[1].text, "Half a");
    assert!(rows[1].interrupted);
}

#[tokio::test]
async fn stop_while_the_model_streams_keeps_the_partial_text_and_returns_stopped() {
    let f = fx(vec![
        calls("", vec![call("c1", "list_dir", json!({}))]),
        Step::UntilCancelled("Thinking about".into()),
    ]);
    let stopper = async {
        f.wait_for(|e| matches!(e, EngineEvent::TextDelta { .. }))
            .await;
        turn::stop(&f.state, &f.chat);
    };
    let (r, ()) = tokio::join!(f.send("go"), stopper);
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("stopped"));
    assert_eq!(
        f.shape(),
        ["user", "assistant[list_dir]", "tool:ok", "assistant"]
    );
    assert!(f.rows()[3].interrupted);
}

#[tokio::test]
async fn a_chat_without_a_folder_or_a_model_without_tools_uses_the_one_request_path() {
    let f = fx(vec![say("plain one"), say("plain two")]);
    // A folder, but a model without tools.
    assert_eq!(f.send_on("hi", PLAIN_MODEL).await.status, TurnStatus::Done);
    // No folder, a model with tools.
    folder::set_chat_folder(&f.state, &f.chat, None)
        .await
        .unwrap();
    assert_eq!(f.send("again").await.status, TurnStatus::Done);
    let reqs = f.requests();
    assert_eq!(reqs.len(), 2);
    for r in &reqs {
        assert_plain(r);
        assert_eq!(r.messages[0].role, Role::User);
    }
    assert_eq!(f.shape(), ["user", "assistant", "user", "assistant"]);
}

#[tokio::test]
async fn the_one_request_body_has_no_tools_provider_or_session_id() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n",
                ),
        )
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let keys = SessionStore::default();
    keys.set("sk-or-v1-test").unwrap();
    let state = AppState::new(
        HttpClients::from_builder(
            || reqwest::Client::builder().no_proxy(),
            HttpTimeouts::PRODUCTION,
        )
        .unwrap(),
        server.uri(),
        Arc::new(keys),
        Store::in_memory().unwrap(),
        data.path().to_path_buf(),
        data.path().join("none"),
        Arc::new(NullSink),
        Arc::new(NoDialogs),
    );
    ModelCache::save(
        &state.model_cache_file(),
        &[
            model(TOOLS_MODEL, true, 100_000),
            model(PLAIN_MODEL, false, 100_000),
        ],
    )
    .unwrap();
    let plain_chat = state.store.create_chat(TOOLS_MODEL).unwrap().id;
    let folder_chat = state.store.create_chat(PLAIN_MODEL).unwrap().id;
    state
        .store
        .set_folder(&folder_chat, Some(&real(project.path())))
        .unwrap();
    for (chat, model) in [(&plain_chat, TOOLS_MODEL), (&folder_chat, PLAIN_MODEL)] {
        let r = turn::send_message(&state, chat, "Hi", vec![], model, &|_| {})
            .await
            .unwrap();
        assert_eq!(r.status, TurnStatus::Done);
        let reqs = server.received_requests().await.unwrap();
        let body: Value = reqs.last().unwrap().body_json().unwrap();
        let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["messages", "model", "stream", "usage"], "{body}");
    }
}

#[tokio::test]
async fn history_with_tool_rows_is_sent_as_plain_chat_on_the_one_request_path() {
    let f = fx(vec![
        calls(
            "Reading.",
            vec![call("c1", "read_file", json!({"path": "a.txt"}))],
        ),
        calls("", vec![call("c2", "list_dir", json!({}))]),
        say("Done."),
        say("plain"),
    ]);
    f.send("look").await;
    let summaries: Vec<String> = f
        .rows()
        .iter()
        .filter_map(|m| m.tool_meta.as_ref().map(|t| t.summary.clone()))
        .collect();
    folder::set_chat_folder(&f.state, &f.chat, None)
        .await
        .unwrap();
    f.send("and now?").await;
    let last = f.requests().pop().unwrap();
    assert_plain(&last);
    let roles: Vec<Role> = last.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [
            Role::User,
            Role::Assistant,
            Role::Assistant,
            Role::Assistant,
            Role::User
        ]
    );
    assert_eq!(
        text_of(&last.messages[1]),
        format!("Reading.\n[{}]", summaries[0])
    );
    assert_eq!(text_of(&last.messages[2]), format!("[{}]", summaries[1]));
    assert_eq!(text_of(&last.messages[3]), "Done.");
}

#[tokio::test]
async fn the_folder_removed_mid_task_skips_the_rest_ends_the_task_and_messages_still_load() {
    let f = fx(vec![calls(
        "",
        vec![
            call("c1", "read_file", json!({"path": "a.txt"})),
            call("c2", "list_dir", json!({})),
        ],
    )]);
    let root = f.project.path().to_path_buf();
    f.on_event(move |e| {
        if matches!(e, EngineEvent::ToolFinished { call_id, .. } if call_id == "c1") {
            let _ = std::fs::remove_dir_all(&root);
        }
    });
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("folder_missing"));
    assert_eq!(f.requests().len(), 1);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[read_file,list_dir]",
            "tool:ok",
            "tool:not_run",
            "note"
        ]
    );
    assert_eq!(f.rows()[3].text, FOLDER_CHANGED);
    assert_eq!(f.rows()[4].text, FOLDER_GONE);
}

#[tokio::test]
async fn the_skills_note_is_the_project_text_in_a_project_chat_and_the_0_2_0_text_otherwise() {
    assert_eq!(prompt::skills_note(false), SKILL_NOTE);
    let f = fx(vec![say("one"), say("two")]);
    chat_state::activate_skill(&f.state, &f.chat, "p:a", TOOLS_MODEL).unwrap();
    f.send("hi").await;
    folder::set_chat_folder(&f.state, &f.chat, None)
        .await
        .unwrap();
    f.send("again").await;
    let reqs = f.requests();
    assert_eq!(text_of(&reqs[0].messages[1]), prompt::skills_note(true));
    assert!(text_of(&reqs[0].messages[2]).starts_with("Skill: p:a"));
    assert_eq!(text_of(&reqs[1].messages[0]), SKILL_NOTE);
    assert!(text_of(&reqs[1].messages[1]).starts_with("Skill: p:a"));
}

#[tokio::test]
async fn compaction_in_a_chat_with_tool_rows_keeps_every_remaining_call_paired() {
    let f = fx(vec![
        calls("", vec![call("c1", "read_file", json!({"path": "a.txt"}))]),
        say("Read it."),
        say("SUMMARY"),
        calls("", vec![call("c2", "list_dir", json!({}))]),
        say("Listed."),
    ]);
    f.send("read").await;
    let r = compact::compact_chat(&f.state, &f.chat, TOOLS_MODEL, None)
        .await
        .unwrap();
    assert_eq!(r.status, TurnStatus::Done);
    let summary_req = &f.requests()[2];
    assert_plain(summary_req);
    f.send("list").await;
    for r in f.requests() {
        assert_well_formed(&r);
    }
    let last = f.requests().pop().unwrap();
    assert!(last.messages.iter().any(|m| text_of(m).contains("SUMMARY")));
    assert!(f.state.store.unanswered_calls(&f.chat).unwrap().is_empty());
}

#[tokio::test]
async fn compaction_never_ends_between_a_call_and_its_missing_results() {
    let f = fx(vec![say("SUMMARY"), say("ok")]);
    let s = &f.state.store;
    s.append_message(&f.chat, "user", "one", &[], false)
        .unwrap();
    s.append_message(&f.chat, "assistant", "two", &[], false)
        .unwrap();
    s.append_message(&f.chat, "user", "three", &[], false)
        .unwrap();
    s.append_message(&f.chat, "assistant", "four", &[], false)
        .unwrap();
    s.append_message(&f.chat, "user", "five", &[], false)
        .unwrap();
    // A task that ended without storing its result (a crash).
    let open = crate::store::StoredToolCall {
        id: "c9".into(),
        name: "list_dir".into(),
        arguments: "{}".into(),
    };
    s.append_assistant(&f.chat, "", &[open], false).unwrap();
    compact::compact_chat(&f.state, &f.chat, TOOLS_MODEL, None)
        .await
        .unwrap();
    // The boundary is before the user row that started the open task: the
    // conversation after the summary starts with that user row.
    let sendable = s.sendable_messages(&f.chat).unwrap();
    assert_eq!(sendable.len(), 2);
    assert_eq!(sendable[0].text, "five");
    assert_eq!(sendable[1].tool_calls.len(), 1);
    f.send("continue").await;
    let last = f.requests().pop().unwrap();
    assert_well_formed(&last);
    assert!(
        last.messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("c9"))
    );
}

#[tokio::test]
async fn usage_is_recorded_per_request_with_running_totals() {
    let f = fx(vec![
        calls("", vec![call("c1", "list_dir", json!({}))]),
        calls("", vec![call("c2", "list_dir", json!({}))]),
        say("ok"),
    ]);
    f.send("go").await;
    let usage: Vec<(u32, f64)> = f
        .events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::Usage { requests, cost_usd } => Some((requests, cost_usd)),
            _ => None,
        })
        .collect();
    assert_eq!(usage.len(), 3);
    for (i, (requests, cost)) in usage.iter().enumerate() {
        assert_eq!(*requests, i as u32 + 1);
        assert!((cost - 0.01 * (i as f64 + 1.0)).abs() < 1e-9, "{usage:?}");
    }
    let (requests, cost) = f.state.store.chat_usage(&f.chat).unwrap();
    assert_eq!(requests, 3);
    assert!((cost - 0.03).abs() < 1e-9);
}

#[tokio::test]
async fn a_reply_with_neither_text_nor_calls_ends_done_with_a_note() {
    let f = fx(vec![say("")]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Done);
    assert_eq!(f.shape(), ["user", "note"]);
    assert_eq!(f.rows()[1].text, NOTHING);
}

#[tokio::test]
async fn an_overflowed_reply_is_noted_and_the_model_is_told_once() {
    let f = fx(vec![overflowed("Doing lots"), say("ok")]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Done);
    assert_eq!(f.shape(), ["user", "assistant", "note", "assistant"]);
    assert_eq!(f.rows()[2].text, OVERFLOW_NOTE);
    let second = &f.requests()[1];
    let last = second.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(text_of(last), OVERFLOW_ASK);
}

#[tokio::test]
async fn two_overflowed_replies_in_a_row_pause_the_task_as_going_in_circles() {
    let f = fx(vec![overflowed(""), overflowed("")]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Paused);
    assert_eq!(r.reason.as_deref(), Some("going_in_circles"));
    assert_eq!(f.requests().len(), 2);
    assert_eq!(f.shape(), ["user", "note", "note", "note"]);
    assert_eq!(
        notes(&f),
        [OVERFLOW_NOTE, OVERFLOW_NOTE, guards::STRUGGLING]
    );
    assert_eq!(
        f.events().last(),
        Some(&EngineEvent::Paused {
            reason: "going_in_circles".into()
        })
    );
}

#[tokio::test]
async fn an_assistant_row_that_cannot_be_stored_fails_the_task_and_runs_no_tool() {
    let f = fx(vec![calls(
        "",
        vec![call(
            "c1",
            "write_file",
            json!({"path": "b.txt", "content": "x"}),
        )],
    )]);
    f.state
        .store
        .exec_for_test(
            "CREATE TRIGGER no_assistant BEFORE INSERT ON messages WHEN NEW.role = 'assistant'
             BEGIN SELECT RAISE(ABORT, 'full'); END;",
        )
        .unwrap();
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Failed);
    assert_eq!(f.cards(), 0);
    assert!(!f.path("b.txt").exists());
    assert_eq!(f.shape(), ["user"]);
}

#[tokio::test]
async fn a_result_that_cannot_be_stored_fails_the_task_and_the_next_send_repairs() {
    let f = fx(vec![calls(
        "",
        vec![call("c1", "read_file", json!({"path": "a.txt"}))],
    )]);
    f.state
        .store
        .exec_for_test(
            "CREATE TRIGGER no_tool BEFORE INSERT ON messages WHEN NEW.kind = 'tool'
             BEGIN SELECT RAISE(ABORT, 'full'); END;",
        )
        .unwrap();
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Failed);
    assert_eq!(f.shape(), ["user", "assistant[read_file]", "note"]);
    assert_eq!(f.rows()[2].text, RESULT_NOT_SAVED);
    f.state
        .store
        .exec_for_test("DROP TRIGGER no_tool;")
        .unwrap();
    f.engine.push(vec![say("ok")]);
    f.send("continue").await;
    assert!(not_run_texts(&f).is_empty());
    let lost = &f.rows()[3];
    assert_eq!(lost.tool_call_id.as_deref(), Some("c1"));
    assert_eq!(lost.tool_meta.as_ref().unwrap().status, "error");
    assert_eq!(lost.text, LOST);
    assert_well_formed(f.requests().last().unwrap());
}

#[tokio::test]
async fn fullness_counts_the_system_message_tools_and_tool_rows() {
    let f = fx(vec![]);
    let empty = chat_state::chat_fullness(&f.state, &f.chat, SMALL_MODEL, 0).unwrap();
    let fixed = prompt::system(os_name(), detect().name, "x")
        .chars()
        .count()
        + schema::tools().to_string().chars().count();
    assert!(
        empty.percent as usize >= fixed / 4 * 100 / 10_000 - 1,
        "{empty:?}"
    );
    let big = crate::store::StoredToolCall {
        id: "c1".into(),
        name: "write_file".into(),
        arguments: json!({"path": "b.txt", "content": "y".repeat(2_000)}).to_string(),
    };
    let s = &f.state.store;
    s.append_assistant(&f.chat, "", &[big], false).unwrap();
    let meta = crate::store::ToolMeta {
        name: "write_file".into(),
        status: "ok".into(),
        summary: "Created b.txt".into(),
        detail: None,
    };
    s.append_tool_result(&f.chat, "c1", &meta, &"z".repeat(2_000))
        .unwrap();
    let after = chat_state::chat_fullness(&f.state, &f.chat, SMALL_MODEL, 0).unwrap();
    // 4,000+ characters at four a token of a 10,000-token context: +10%.
    let grew = after.percent - empty.percent;
    assert!((10..=11).contains(&grew), "{empty:?} -> {after:?}");
}

// ---------------------------------------------------------------------------
// The invariant: interrupted anywhere, the next send is well-formed
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum How {
    Stop,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum At {
    /// While the first reply streams, before anything is stored.
    FirstStep,
    /// The assistant row is stored; no call has started.
    BeforeCalls,
    /// The first call is running (no card).
    FirstCall,
    /// Between the first call's result and the second call.
    BetweenCalls,
    /// The edit card waits.
    CardWaits,
    /// The command runs.
    CommandRuns,
    /// Every call is answered; the next reply streams.
    NextStep,
}

fn stage_reached(at: At, events: &[EngineEvent]) -> bool {
    let started = |id: &str| {
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::ToolStarted { call_id, .. } if call_id == id))
    };
    let finished = |id: &str| {
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::ToolFinished { call_id, .. } if call_id == id))
    };
    let deltas = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::TextDelta { .. }))
        .count();
    match at {
        At::FirstStep => deltas >= 1,
        At::BeforeCalls => events.contains(&EngineEvent::MessageStored),
        At::FirstCall => started("c1"),
        At::BetweenCalls => finished("c1"),
        At::CardWaits => events
            .iter()
            .any(|e| matches!(e, EngineEvent::ApprovalNeeded { .. })),
        At::CommandRuns => events
            .iter()
            .any(|e| matches!(e, EngineEvent::ToolOutput { call_id, .. } if call_id == "c3")),
        At::NextStep => deltas >= 2,
    }
}

async fn interrupt_and_continue(at: At, how: How) {
    // Every stage but the last stops while the command still runs.
    let command = if at == At::NextStep {
        "echo started"
    } else {
        sleeper()
    };
    let first = Step::Stream(
        vec!["Working".into()],
        StepOutcome {
            text: "Working".into(),
            tool_calls: vec![
                call("c1", "read_file", json!({"path": "a.txt"})),
                call(
                    "c2",
                    "edit_file",
                    json!({"path": "a.txt", "old_text": "hello", "new_text": "bye"}),
                ),
                call("c3", "run_command", json!({"command": command})),
            ],
            ..StepOutcome::default()
        },
    );
    let first = if at == At::FirstStep {
        Step::UntilCancelled("Working".into())
    } else {
        first
    };
    let mut steps = vec![first];
    if at == At::NextStep {
        steps.push(Step::UntilCancelled("Next".into()));
    }
    let f = fx(steps);
    // Answers every card with Once, except the one this run stops on.
    let answerer = async {
        let mut answered = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(long(30));
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "{at:?}: never reached"
            );
            let ids: Vec<String> = f
                .events()
                .into_iter()
                .filter_map(|e| match e {
                    EngineEvent::ApprovalNeeded { approval } => Some(approval.id().to_string()),
                    _ => None,
                })
                .collect();
            if at != At::CardWaits
                && let Some(id) = ids.get(answered)
            {
                let _ = f.state.approvals.answer(&f.chat, id, Answer::Once);
                answered += 1;
            }
            if stage_reached(at, &f.events()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    match how {
        How::Drop => {
            tokio::select! {
                r = f.send("start") => panic!("{at:?}: the task ended before the interrupt: {r:?}"),
                () = answerer => {}
            }
        }
        How::Stop => {
            let stopper = async {
                answerer.await;
                turn::stop(&f.state, &f.chat);
            };
            let (r, ()) = tokio::join!(f.send("start"), stopper);
            assert_eq!(r.status, TurnStatus::Interrupted, "{at:?}");
            assert_eq!(r.reason.as_deref(), Some("stopped"), "{at:?}");
        }
    }
    // Whatever is stored now, the next send repairs it and sends it.
    f.engine.push(vec![say("continuing")]);
    let r = tokio::time::timeout(Duration::from_secs(long(60)), f.send("continue"))
        .await
        .unwrap_or_else(|_| panic!("{at:?} {how:?}: the next send hung: {:?}", f.shape()));
    assert_eq!(r.status, TurnStatus::Done, "{at:?} {how:?}");
    let last = f.requests().pop().unwrap();
    assert_well_formed(&last);
    assert_eq!(text_of(&last.messages[last.messages.len() - 1]), "continue");
    assert!(
        f.state.store.unanswered_calls(&f.chat).unwrap().is_empty(),
        "{at:?} {how:?}: {:?}",
        f.shape()
    );
    // Every call stored got exactly one result.
    let rows = f.rows();
    for m in rows.iter().filter(|m| !m.tool_calls.is_empty()) {
        for c in &m.tool_calls {
            let n = rows
                .iter()
                .filter(|r| r.tool_call_id.as_deref() == Some(c.id.as_str()))
                .count();
            assert_eq!(n, 1, "{at:?} {how:?}: call {} has {n} results", c.id);
        }
    }
    if at == At::CardWaits {
        assert_eq!(std::fs::read_to_string(f.path("a.txt")).unwrap(), "hello\n");
    }
}

#[tokio::test]
async fn interrupted_at_any_point_by_stop_the_next_send_is_well_formed() {
    for at in [
        At::FirstStep,
        At::BeforeCalls,
        At::FirstCall,
        At::BetweenCalls,
        At::CardWaits,
        At::CommandRuns,
        At::NextStep,
    ] {
        interrupt_and_continue(at, How::Stop).await;
    }
}

#[tokio::test]
async fn interrupted_at_any_point_by_dropping_the_task_the_next_send_is_well_formed() {
    for at in [
        At::FirstStep,
        At::BeforeCalls,
        At::FirstCall,
        At::BetweenCalls,
        At::CardWaits,
        At::CommandRuns,
        At::NextStep,
    ] {
        interrupt_and_continue(at, How::Drop).await;
    }
}

#[tokio::test]
async fn a_new_send_replaces_a_running_task_and_keeps_the_conversation_valid() {
    let f = fx(vec![
        calls(
            "",
            vec![
                call("c1", "write_file", json!({"path": "b.txt", "content": "x"})),
                call("c2", "list_dir", json!({})),
            ],
        ),
        say("second"),
    ]);
    let second = async {
        f.wait_for(|e| matches!(e, EngineEvent::ApprovalNeeded { .. }))
            .await;
        f.send("never mind").await
    };
    let (first, second) = tokio::join!(f.send("write"), second);
    assert_eq!(first.status, TurnStatus::Interrupted);
    assert_eq!(second.status, TurnStatus::Done);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,list_dir]",
            "tool:not_run",
            "tool:not_run",
            "user",
            "assistant"
        ]
    );
    assert_well_formed(f.requests().last().unwrap());
}

#[test]
fn the_interrupted_text_is_exact() {
    assert_eq!(INTERRUPTED, "Not run: the task was interrupted.");
    assert_eq!(
        FOLDER_CHANGED,
        "Not run: the project folder was closed or changed."
    );
    assert_eq!(
        LOST,
        "The result of this call was lost; it may have run. Check before repeating."
    );
    assert_eq!(STOPPED, "Not run: the task was stopped.");
    assert_eq!(USER_SENT, "Not run: the user sent a new message.");
    assert_eq!(NOTHING, "The model returned nothing.");
}

#[test]
fn history_adds_a_not_run_result_for_a_call_that_has_none() {
    let rows = vec![StoredMessage {
        id: "a".into(),
        role: "assistant".into(),
        text: String::new(),
        attachments: vec![],
        interrupted: false,
        created_at: 0,
        kind: "message".into(),
        seq: 1,
        tool_calls: vec![crate::store::StoredToolCall {
            id: "c1".into(),
            name: "list_dir".into(),
            arguments: "{}".into(),
        }],
        tool_call_id: None,
        tool_meta: None,
    }];
    let wire = history(&rows, true);
    let req = StepRequest {
        model_id: "m".into(),
        messages: wire.clone(),
        tools: None,
        session_id: String::new(),
    };
    assert_well_formed(&req);
    assert_eq!(text_of(&wire[1]), LOST);
}

// ---------------------------------------------------------------------------
// Fix round 1
// ---------------------------------------------------------------------------

/// The reply most fix tests start from: a card first, then a read and a list.
fn card_then_reads() -> Step {
    calls(
        "",
        vec![
            call(
                "c1",
                "write_file",
                json!({"path": "b.txt", "content": "new"}),
            ),
            call("c2", "read_file", json!({"path": "a.txt"})),
            call("c3", "list_dir", json!({})),
        ],
    )
}

fn card_waits(e: &EngineEvent) -> bool {
    matches!(e, EngineEvent::ApprovalNeeded { .. })
}

/// Every call of every assistant row is answered by exactly one result,
/// stored right after it (before any other non-tool row).
fn assert_rows_paired(f: &Fx) {
    let rows = f.rows();
    let mut i = 0;
    while i < rows.len() {
        let m = &rows[i];
        i += 1;
        if m.kind == "tool" {
            panic!("an orphan result: {:?}", f.shape());
        }
        let mut want: Vec<&str> = m.tool_calls.iter().map(|c| c.id.as_str()).collect();
        while i < rows.len() && (rows[i].kind == "tool" || rows[i].kind == "note") {
            if rows[i].kind == "tool" {
                let id = rows[i].tool_call_id.as_deref().unwrap();
                let at = want
                    .iter()
                    .position(|w| *w == id)
                    .unwrap_or_else(|| panic!("result {id} out of place: {:?}", f.shape()));
                want.remove(at);
            }
            i += 1;
        }
        assert!(want.is_empty(), "unanswered {want:?}: {:?}", f.shape());
    }
    assert!(f.state.store.unanswered_calls(&f.chat).unwrap().is_empty());
}

#[tokio::test]
async fn a_plain_send_during_a_project_task_waits_for_it_and_keeps_every_call_paired() {
    let f = fx(vec![card_then_reads(), say("plain reply")]);
    let plain = async {
        f.wait_for(card_waits).await;
        f.send_on("hi", PLAIN_MODEL).await
    };
    let (first, second) = tokio::join!(f.send("write"), plain);
    assert_eq!(first.status, TurnStatus::Interrupted);
    assert_eq!(second.status, TurnStatus::Done);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,read_file,list_dir]",
            "tool:not_run",
            "tool:not_run",
            "tool:not_run",
            "user",
            "assistant"
        ]
    );
    assert_rows_paired(&f);
    assert!(!f.path("b.txt").exists());
}

#[tokio::test]
async fn a_send_after_the_folder_is_gone_during_a_task_keeps_every_call_paired() {
    let f = fx(vec![card_then_reads(), say("plain reply")]);
    let later = async {
        f.wait_for(card_waits).await;
        std::fs::remove_dir_all(f.project.path()).unwrap();
        f.send("hi").await
    };
    let (_, second) = tokio::join!(f.send("write"), later);
    assert_eq!(second.status, TurnStatus::Done);
    assert_eq!(f.shape().last().map(String::as_str), Some("assistant"));
    assert_rows_paired(&f);
}

#[tokio::test]
async fn compaction_and_take_back_during_a_project_task_change_nothing() {
    let f = fx(vec![card_then_reads()]);
    let during = async {
        f.wait_for(card_waits).await;
        let before = f.shape();
        let compacted = compact::compact_chat(&f.state, &f.chat, TOOLS_MODEL, None).await;
        assert!(compacted.is_err(), "{compacted:?}");
        let user = f.rows()[0].id.clone();
        let taken = chats::take_user_message(&f.state, &f.chat, &user);
        assert!(taken.is_err(), "{taken:?}");
        assert_eq!(f.shape(), before);
        assert!(f.state.approvals.waiting(&f.chat).is_some());
        turn::stop(&f.state, &f.chat);
    };
    let (r, ()) = tokio::join!(f.send("write"), during);
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_rows_paired(&f);
    assert_eq!(f.requests().len(), 1);
}

#[tokio::test]
async fn a_retry_during_a_running_task_waits_for_it_and_is_refused_once_it_stored_tool_rows() {
    let f = fx(vec![card_then_reads(), say("continued")]);
    let retry = async {
        f.wait_for(card_waits).await;
        let emit = f.emitter();
        turn::retry_turn(&f.state, &f.chat, TOOLS_MODEL, false, &emit).await
    };
    let (_, second) = tokio::join!(f.send("write"), retry);
    // The running task stored its not-run results while it was taken over,
    // so the retry would replay tool work: it is refused.
    assert_eq!(
        second.unwrap_err().message,
        "A task that changed things cannot be retried. Send a message to continue."
    );
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,read_file,list_dir]",
            "tool:not_run",
            "tool:not_run",
            "tool:not_run"
        ]
    );
    assert_rows_paired(&f);
    // A message continues it.
    assert_eq!(f.send("continue").await.status, TurnStatus::Done);
    assert_rows_paired(&f);
}

/// Closing or changing the folder while a card waits: nothing more is read
/// in the old folder, and every call left says why it did not run.
async fn folder_changed_while_a_card_waits(to: Option<String>) {
    let f = fx(vec![card_then_reads()]);
    if let Some(other) = &to {
        f.state.store.approve_folder(other).unwrap();
    }
    let change = async {
        f.wait_for(card_waits).await;
        folder::set_chat_folder(&f.state, &f.chat, to.clone())
            .await
            .unwrap();
    };
    let (r, ()) = tokio::join!(f.send("write"), change);
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("folder_missing"));
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,read_file,list_dir]",
            "tool:not_run",
            "tool:not_run",
            "tool:not_run",
            "note"
        ]
    );
    assert_eq!(
        not_run_texts(&f),
        [FOLDER_CHANGED, FOLDER_CHANGED, FOLDER_CHANGED]
    );
    assert!(!f.rows().iter().any(|m| m.text.contains("hello")));
    assert!(!f.path("b.txt").exists());
    assert_eq!(f.requests().len(), 1);
}

#[tokio::test]
async fn closing_the_folder_while_a_card_waits_reads_nothing_more() {
    folder_changed_while_a_card_waits(None).await;
}

#[tokio::test]
async fn changing_the_folder_while_a_card_waits_reads_nothing_in_either_folder() {
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("a.txt"), "hello there\n").unwrap();
    folder_changed_while_a_card_waits(Some(picked(other.path()))).await;
}

#[tokio::test]
async fn a_folder_changed_in_the_store_between_calls_skips_the_rest_without_reading() {
    // No cancel and no dropped card: only the checks before acting see it.
    let f = fx(vec![card_then_reads()]);
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("a.txt"), "hello there\n").unwrap();
    let change = async {
        f.wait_for(card_waits).await;
        f.state
            .store
            .set_folder(&f.chat, Some(&real(other.path())))
            .unwrap();
        f.answer_cards(vec![Answer::Once]).await;
    };
    let (r, ()) = tokio::join!(f.send("go"), change);
    assert_eq!(r.reason.as_deref(), Some("folder_missing"));
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,read_file,list_dir]",
            "tool:error",
            "tool:not_run",
            "tool:not_run",
            "note"
        ]
    );
    assert_eq!(not_run_texts(&f), [FOLDER_CHANGED, FOLDER_CHANGED]);
    assert!(!f.rows().iter().any(|m| m.text.contains("hello")));
    assert!(!f.path("b.txt").exists());
    assert!(!other.path().join("b.txt").exists());
}

#[tokio::test]
async fn a_crash_after_a_command_started_records_its_result_as_lost() {
    let f = fx(vec![calls(
        "",
        vec![call("c1", "run_command", json!({"command": sleeper()}))],
    )]);
    let started = async {
        f.answer_cards(vec![Answer::Once]).await;
        f.wait_for(|e| matches!(e, EngineEvent::ToolOutput { .. }))
            .await;
    };
    tokio::select! {
        _ = f.send("run") => panic!("the command should still run"),
        () = started => {}
    }
    f.engine.push(vec![say("ok")]);
    f.send("continue").await;
    let result = &f.rows()[2];
    assert_eq!(result.tool_call_id.as_deref(), Some("c1"));
    assert_eq!(result.tool_meta.as_ref().unwrap().status, "error");
    assert_eq!(result.text, LOST);
    assert_well_formed(f.requests().last().unwrap());
}

#[tokio::test]
async fn a_crash_before_a_call_started_records_it_as_not_run() {
    // The card never got an answer, so nothing started.
    let f = fx(vec![calls(
        "",
        vec![call("c1", "run_command", json!({"command": "echo hi"}))],
    )]);
    tokio::select! {
        _ = f.send("run") => panic!("the card should wait"),
        () = f.wait_for(card_waits) => {}
    }
    f.engine.push(vec![say("ok")]);
    f.send("continue").await;
    assert_eq!(not_run_texts(&f), [INTERRUPTED]);
}

#[tokio::test]
async fn a_request_that_was_never_sent_is_not_counted() {
    let f = fx(vec![
        calls("", vec![call("c1", "list_dir", json!({}))]),
        Step::Unsent(StepOutcome {
            error: Some(StepError {
                ui: UiError::from(ApiError::Unauthorized),
                retry_after_secs: None,
            }),
            ..StepOutcome::default()
        }),
    ]);
    f.send("go").await;
    let usage: Vec<u32> = f
        .events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::Usage { requests, .. } => Some(requests),
            _ => None,
        })
        .collect();
    assert_eq!(usage, [1]);
    assert_eq!(f.state.store.chat_usage(&f.chat).unwrap().0, 1);
}

#[tokio::test]
async fn deleting_chats_forgets_their_task_locks() {
    let f = fx(vec![]);
    for _ in 0..20 {
        let id = f.state.store.create_chat(TOOLS_MODEL).unwrap().id;
        f.state
            .store
            .set_folder(&id, Some(&real(f.project.path())))
            .unwrap();
        f.engine.push(vec![say("x")]);
        let r = turn::send_message(&f.state, &id, "hi", vec![], TOOLS_MODEL, &|_| {})
            .await
            .unwrap();
        assert_eq!(r.status, TurnStatus::Done);
        chats::delete_chat(&f.state, &id).await.unwrap();
    }
    assert_eq!(f.state.task_locks.chats(), 0);
}

#[tokio::test]
async fn compaction_refuses_when_no_user_row_starts_the_open_task() {
    let f = fx(vec![say("SUMMARY")]);
    let s = &f.state.store;
    let first = s
        .append_message(&f.chat, "assistant", "a", &[], false)
        .unwrap();
    for t in ["b", "c", "d", "e"] {
        s.append_message(&f.chat, "assistant", t, &[], false)
            .unwrap();
    }
    let open = crate::store::StoredToolCall {
        id: "c9".into(),
        name: "list_dir".into(),
        arguments: "{}".into(),
    };
    s.append_assistant(&f.chat, "", &[open], false).unwrap();
    s.set_summary(&f.chat, "S", first.seq).unwrap();
    let r = compact::compact_chat(&f.state, &f.chat, TOOLS_MODEL, None).await;
    assert!(r.is_err(), "{r:?}");
    assert!(f.requests().is_empty());
    assert_eq!(
        s.summary(&f.chat).unwrap(),
        Some(("S".to_string(), first.seq))
    );
}

// ---------------------------------------------------------------------------
// Guards (Task 12)
// ---------------------------------------------------------------------------

use crate::agent::guards::{self as g, set_spend_limit};

const LIMIT_REACHED: &str =
    "You have reached today's spending limit of $1.00. You can change it in Settings.";

fn costing(cost: f64, step: Step) -> Step {
    match step {
        Step::Reply(out) => Step::Reply(StepOutcome {
            cost_usd: Some(cost),
            ..out
        }),
        other => other,
    }
}

fn rate_limited(retry_after_secs: Option<u64>) -> Step {
    Step::Reply(StepOutcome {
        error: Some(StepError {
            ui: UiError::from(ApiError::RateLimited { retry_after_secs }),
            retry_after_secs,
        }),
        ..StepOutcome::default()
    })
}

fn list(id: &str) -> Step {
    calls("", vec![call(id, "list_dir", json!({}))])
}

/// `n` replies, each one `list_dir` call with its own id.
fn lists(n: usize) -> Vec<Step> {
    (0..n).map(|i| list(&format!("l{i}"))).collect()
}

fn keep_going_cards(f: &Fx) -> Vec<(u32, Option<u32>, Option<f64>)> {
    f.events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::ApprovalNeeded {
                approval:
                    crate::agent::Approval::KeepGoing {
                        requests,
                        free_left,
                        spent_today_usd,
                        ..
                    },
            } => Some((requests, free_left, spent_today_usd)),
            _ => None,
        })
        .collect()
}

fn notes(f: &Fx) -> Vec<String> {
    f.rows()
        .into_iter()
        .filter(|m| m.kind == "note")
        .map(|m| m.text)
        .collect()
}

impl Fx {
    async fn try_send_on(&self, text: &str, model: &str) -> Result<TurnResult, UiError> {
        let emit = self.emitter();
        turn::send_message(&self.state, &self.chat, text, vec![], model, &emit).await
    }

    /// Sends "continue" after a pause and checks the task goes on: the
    /// request is well-formed, ends with "continue", and the task is done.
    async fn continues(&self, model: &str) {
        self.engine.push(vec![say("going on")]);
        let before = self.requests().len();
        let r = self.send_on("continue", model).await;
        assert_eq!(r.status, TurnStatus::Done, "{r:?}: {:?}", self.shape());
        let reqs = self.requests();
        assert_eq!(reqs.len(), before + 1);
        let last = reqs.last().unwrap();
        assert_well_formed(last);
        assert!(last.tools.is_some());
        assert_eq!(text_of(last.messages.last().unwrap()), "continue");
        assert_rows_paired(self);
    }
}

fn spend(f: &Fx, usd: f64) {
    let other = f.state.store.create_chat(TOOLS_MODEL).unwrap().id;
    f.state.store.record_usage(&other, 1, usd).unwrap();
}

#[tokio::test]
async fn sends_and_retries_on_a_paid_model_over_the_limit_are_refused_before_any_request() {
    let f = fx(vec![]);
    spend(&f, 1.00);
    // A plain send (a model without tools) and a project send.
    for model in [PAID_PLAIN, PAID_TOOLS] {
        let e = f.try_send_on("hi", model).await.unwrap_err();
        assert_eq!(e.message, LIMIT_REACHED);
        assert_eq!(e.kind, "daily_limit");
    }
    assert!(f.requests().is_empty());
    assert!(f.shape().is_empty(), "nothing stored: {:?}", f.shape());
    // A retry of a stored question.
    f.state
        .store
        .append_message(&f.chat, "user", "q", &[], false)
        .unwrap();
    for model in [PAID_PLAIN, PAID_TOOLS] {
        let emit = f.emitter();
        let e = turn::retry_turn(&f.state, &f.chat, model, false, &emit)
            .await
            .unwrap_err();
        assert_eq!(e.message, LIMIT_REACHED);
    }
    assert!(f.requests().is_empty());
    assert_eq!(f.shape(), ["user"]);
    // A free model is never blocked.
    f.engine.push(vec![say("free")]);
    assert_eq!(f.send("hi").await.status, TurnStatus::Done);
    // No limit lets the paid send through.
    set_spend_limit(&f.state.store, None).unwrap();
    f.engine.push(vec![say("paid")]);
    assert_eq!(
        f.send_on("again", PAID_PLAIN).await.status,
        TurnStatus::Done
    );
}

#[tokio::test]
async fn compaction_on_a_paid_model_over_the_limit_is_refused_and_leaves_the_chat_free() {
    let f = fx(vec![]);
    for (role, t) in [
        ("user", "a"),
        ("assistant", "b"),
        ("user", "c"),
        ("assistant", "d"),
    ] {
        f.state
            .store
            .append_message(&f.chat, role, t, &[], false)
            .unwrap();
    }
    spend(&f, 1.50);
    let e = compact::compact_chat(&f.state, &f.chat, PAID_PLAIN, None)
        .await
        .unwrap_err();
    assert_eq!(e.message, LIMIT_REACHED);
    assert!(f.requests().is_empty());
    assert_eq!(f.state.store.summary(&f.chat).unwrap(), None);
    assert!(!f.state.turns().contains_key(&f.chat));
    // The chat is not left busy: a free compaction runs.
    f.engine.push(vec![say("SUMMARY")]);
    let r = compact::compact_chat(&f.state, &f.chat, TOOLS_MODEL, None)
        .await
        .unwrap();
    assert_eq!(r.status, TurnStatus::Done);
}

#[tokio::test]
async fn a_paid_task_crossing_the_limit_mid_task_pauses_with_daily_limit_and_keeps_its_rows() {
    let f = fx(vec![costing(0.6, list("c1")), costing(0.6, list("c2"))]);
    let r = f.send_on("go", PAID_TOOLS).await;
    assert_eq!(r.status, TurnStatus::Paused);
    assert_eq!(r.reason.as_deref(), Some("daily_limit"));
    assert_eq!(f.requests().len(), 2);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[list_dir]",
            "tool:ok",
            "assistant[list_dir]",
            "tool:ok",
            "note"
        ]
    );
    assert_eq!(
        notes(&f),
        ["Paused: today's spending limit of $1.00 is reached. You can change it in Settings."]
    );
    assert_eq!(
        f.events().last(),
        Some(&EngineEvent::Paused {
            reason: "daily_limit".into()
        })
    );
    // A send while over the limit is refused and stores nothing.
    let before = f.shape();
    assert!(f.try_send_on("continue", PAID_TOOLS).await.is_err());
    assert_eq!(f.shape(), before);
    // Raised, the next send continues.
    set_spend_limit(&f.state.store, Some(5.0)).unwrap();
    f.continues(PAID_TOOLS).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_25_requests_the_task_asks_to_keep_going_and_stop_here_pauses() {
    let f = fx(lists(50));
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(30).unwrap());
    let (r, ()) = tokio::join!(
        f.send("go"),
        f.answer_cards(vec![Answer::KeepGoing, Answer::StopHere])
    );
    assert_eq!(r.status, TurnStatus::Paused, "{r:?}");
    assert_eq!(r.reason.as_deref(), Some("keep_going_declined"));
    assert_eq!(f.requests().len(), 50);
    assert_eq!(
        keep_going_cards(&f),
        [(25, Some(30), None), (50, Some(30), None)]
    );
    // No note: the user chose to stop.
    assert!(notes(&f).is_empty());
    assert_eq!(f.shape().last().map(String::as_str), Some("tool:ok"));
    f.continues(TOOLS_MODEL).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_keep_going_card_on_a_paid_model_shows_todays_spend() {
    let mut steps = lists(25);
    steps.push(say("done"));
    let f = fx(steps);
    spend(&f, 0.5);
    let (r, ()) = tokio::join!(
        f.send_on("go", PAID_TOOLS),
        f.answer_cards(vec![Answer::KeepGoing])
    );
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    let cards = keep_going_cards(&f);
    assert_eq!(cards.len(), 1);
    let (requests, free, spent) = cards[0];
    assert_eq!((requests, free), (25, None));
    // $0.50 before, and 25 requests at $0.01.
    assert!((spent.unwrap() - 0.75).abs() < 1e-9, "{spent:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_answer_to_keep_going_stores_the_user_row_and_continues() {
    let mut steps = lists(25);
    steps.push(say("ok, doing that"));
    let f = fx(steps);
    let (r, ()) = tokio::join!(
        f.send("go"),
        f.answer_cards(vec![Answer::Message {
            text: "now tidy up".into()
        }])
    );
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    let shape = f.shape();
    let n = shape.len();
    assert_eq!(shape[n - 3..], ["tool:ok", "user", "assistant"]);
    assert_eq!(f.rows()[n - 2].text, "now tidy up");
    let last = f.requests().pop().unwrap();
    assert_well_formed(&last);
    assert_eq!(text_of(last.messages.last().unwrap()), "now tidy up");
    assert_eq!(f.requests().len(), 26);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_while_the_keep_going_card_waits_ends_the_task_as_stopped() {
    let f = fx(lists(25));
    let stopper = async {
        f.wait_for(card_waits).await;
        turn::stop(&f.state, &f.chat);
    };
    let (r, ()) = tokio::join!(f.send("go"), stopper);
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("stopped"));
    assert_eq!(f.requests().len(), 25);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_free_model_with_no_free_requests_left_pauses_before_any_request() {
    let f = fx(vec![]);
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(0).unwrap());
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Paused);
    assert_eq!(r.reason.as_deref(), Some("no_free_requests"));
    assert!(f.requests().is_empty());
    assert_eq!(f.shape(), ["user", "note"]);
    assert_eq!(
        notes(&f),
        ["Paused: no free requests are left today. They reset at midnight UTC."]
    );
    // Read once, just now: a fresh 0 is not read again.
    assert_eq!(
        f.outside.fetches.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(5).unwrap());
    f.continues(TOOLS_MODEL).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_free_allowance_reaching_zero_mid_task_pauses_at_the_next_check() {
    let f = fx(lists(12));
    f.outside.statuses.lock().unwrap().push_back(free_left(10));
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(0).unwrap());
    let r = f.send("go").await;
    assert_eq!(r.reason.as_deref(), Some("no_free_requests"), "{r:?}");
    // Checked before the first request and again after ten.
    assert_eq!(f.requests().len(), 10);
    assert_eq!(f.shape().last().map(String::as_str), Some("note"));
    assert_rows_paired(&f);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_status_that_cannot_be_read_never_blocks_a_task() {
    // The fallback is None: every fetch fails.
    let f = fx(vec![list("c1"), say("ok")]);
    assert_eq!(f.send("go").await.status, TurnStatus::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rate_limit_is_waited_out_and_the_same_request_sent_again() {
    let f = fx(vec![list("c1"), rate_limited(Some(2)), say("ok")]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert!(f.events().contains(&EngineEvent::Waiting { seconds: 2 }));
    assert_eq!(*f.outside.waits.lock().unwrap(), [2]);
    let reqs = f.requests();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[1], reqs[2], "the retry is byte-identical");
    // The rate-limited request was sent and counts.
    assert_eq!(f.state.store.chat_usage(&f.chat).unwrap().0, 3);
    assert_eq!(
        f.shape(),
        ["user", "assistant[list_dir]", "tool:ok", "assistant"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fourth_rate_limit_in_a_task_pauses_and_the_waits_are_capped() {
    let f = fx(vec![
        list("c1"),
        rate_limited(None),
        rate_limited(Some(120)),
        rate_limited(Some(1)),
        rate_limited(Some(1)),
    ]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Paused);
    assert_eq!(r.reason.as_deref(), Some("rate_limited"));
    assert_eq!(*f.outside.waits.lock().unwrap(), [10, 60, 1]);
    let waiting: Vec<u64> = f
        .events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::Waiting { seconds } => Some(seconds),
            _ => None,
        })
        .collect();
    assert_eq!(waiting, [10, 60, 1]);
    assert_eq!(f.requests().len(), 5);
    assert_eq!(
        notes(&f),
        ["Paused: the model is limiting requests. Try again in a minute."]
    );
    f.continues(TOOLS_MODEL).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_during_a_rate_limit_wait_ends_the_task_at_once() {
    let f = fx(vec![list("c1"), rate_limited(Some(60))]);
    // A real minute, unless the stop cuts it short.
    f.outside
        .ms_per_second
        .store(1_000, std::sync::atomic::Ordering::SeqCst);
    let stopper = async {
        f.wait_for(|e| matches!(e, EngineEvent::Waiting { .. }))
            .await;
        turn::stop(&f.state, &f.chat);
    };
    let started = std::time::Instant::now();
    let (r, ()) = tokio::join!(f.send("go"), stopper);
    assert!(started.elapsed() < Duration::from_secs(long(10)));
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("stopped"));
    assert_eq!(f.requests().len(), 2);
    assert_eq!(f.shape(), ["user", "assistant[list_dir]", "tool:ok"]);
}

fn read_missing(id: &str) -> ToolCall {
    call(id, "read_file", json!({"path": "missing.txt"}))
}

#[tokio::test]
async fn three_identical_failing_calls_pause_the_task_with_the_struggling_note() {
    let f = fx(vec![
        calls("", vec![read_missing("c1")]),
        calls("", vec![read_missing("c2")]),
        calls(
            "",
            vec![read_missing("c3"), call("c4", "list_dir", json!({}))],
        ),
    ]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Paused);
    assert_eq!(r.reason.as_deref(), Some("going_in_circles"));
    assert_eq!(f.requests().len(), 3);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[read_file]",
            "tool:error",
            "assistant[read_file]",
            "tool:error",
            "assistant[read_file,list_dir]",
            "tool:error",
            "tool:not_run",
            "note"
        ]
    );
    assert_eq!(not_run_texts(&f), [PAUSED]);
    assert_eq!(
        notes(&f),
        ["This model is struggling with this task. A stronger model usually does better."]
    );
    f.continues(TOOLS_MODEL).await;
}

#[tokio::test]
async fn a_success_between_failing_calls_resets_the_count() {
    let f = fx(vec![
        calls("", vec![read_missing("c1"), read_missing("c2")]),
        calls("", vec![call("c3", "list_dir", json!({}))]),
        calls("", vec![read_missing("c4"), read_missing("c5")]),
        say("I give up on that file."),
    ]);
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert!(notes(&f).is_empty());
}

#[tokio::test]
async fn five_unreadable_calls_in_a_row_pause_the_task_with_the_struggling_note() {
    let broken = |id: &str| ToolCall {
        id: id.into(),
        name: "read_file".into(),
        arguments: "{\"path\": ".into(),
    };
    let f = fx(vec![
        calls(
            "",
            vec![
                call("c1", "make_coffee", json!({})),
                broken("c2"),
                call("c3", "make_tea", json!({})),
            ],
        ),
        calls(
            "",
            vec![
                broken("c4"),
                broken("c5"),
                call("c6", "list_dir", json!({})),
            ],
        ),
    ]);
    let r = f.send("go").await;
    assert_eq!(r.reason.as_deref(), Some("going_in_circles"), "{r:?}");
    assert_eq!(f.requests().len(), 2);
    assert_eq!(
        f.shape()[5..],
        [
            "assistant[read_file,read_file,list_dir]",
            "tool:error",
            "tool:error",
            "tool:not_run",
            "note"
        ]
    );
    assert_eq!(
        notes(&f),
        ["This model is struggling with this task. A stronger model usually does better."]
    );
    f.continues(TOOLS_MODEL).await;
}

/// A file whose `read_file` result is about `chars` characters.
fn big_file(f: &Fx, name: &str, chars: usize) {
    let line = format!("{}\n", "y".repeat(59));
    std::fs::write(f.path(name), line.repeat(chars / 60)).unwrap();
}

fn read_big(id: &str) -> ToolCall {
    call(id, "read_file", json!({"path": "big.txt"}))
}

/// The fixed part of a project request on the small model, in characters.
fn fixed_chars(f: &Fx) -> usize {
    let name = std::path::Path::new(&real(f.project.path()))
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    prompt::system(os_name(), detect().name, &name)
        .chars()
        .count()
        + schema::tools().to_string().chars().count()
}

#[tokio::test]
async fn over_seventy_percent_the_oldest_results_are_trimmed_in_the_request_only() {
    let mut steps: Vec<Step> = (0..6)
        .map(|i| calls("", vec![read_big(&format!("c{i}"))]))
        .collect();
    steps.push(say("done"));
    let f = fx(steps);
    // 70% of the small model's 10,000 tokens is 28,000 characters: the
    // fixed part and four results fit, six do not.
    let room = 28_000 - fixed_chars(&f);
    let result = room / 5;
    assert!(result <= 16_000 - 1_000, "a read returns at most 16,000");
    big_file(&f, "big.txt", result);
    let r = f.send_on("go", SMALL_MODEL).await;
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    let reqs = f.requests();
    assert_eq!(reqs.len(), 7);
    let tool_texts = |req: &StepRequest| -> Vec<String> {
        req.messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(text_of)
            .collect()
    };
    let last = tool_texts(&reqs[6]);
    assert_eq!(last.len(), 6);
    let trimmed = last.iter().filter(|t| *t == g::TRIMMED).count();
    assert!(trimmed >= 1, "{trimmed}");
    // The oldest first, never the last four.
    assert!(last[..trimmed].iter().all(|t| t == g::TRIMMED));
    assert!(last[trimmed..].iter().all(|t| t != g::TRIMMED));
    assert!(trimmed <= 2);
    // Once trimmed, trimmed in every later request: the start of each
    // request is the start of the next.
    for pair in reqs.windows(2) {
        let (a, b) = (tool_texts(&pair[0]), tool_texts(&pair[1]));
        for (x, y) in a.iter().zip(&b) {
            if x == g::TRIMMED {
                assert_eq!(y, g::TRIMMED);
            }
        }
    }
    for r in &reqs {
        assert_well_formed(r);
    }
    // What is stored and shown is never trimmed.
    let stored: Vec<StoredMessage> = f.rows().into_iter().filter(|m| m.kind == "tool").collect();
    assert_eq!(stored.len(), 6);
    assert!(stored.iter().all(|m| m.text.chars().count() > result / 2));
}

#[tokio::test]
async fn a_chat_trimming_cannot_fit_pauses_with_the_chat_full_note() {
    let mut steps: Vec<Step> = (0..4)
        .map(|i| calls("", vec![read_big(&format!("c{i}"))]))
        .collect();
    steps.push(say("never sent"));
    let f = fx(steps);
    // Four results alone, which are never trimmed, are over 70%.
    let room = 28_000 - fixed_chars(&f);
    big_file(&f, "big.txt", room / 4 + 1_000);
    let r = f.send_on("go", SMALL_MODEL).await;
    assert_eq!(r.status, TurnStatus::Paused, "{r:?}");
    assert_eq!(r.reason.as_deref(), Some("chat_full"));
    assert_eq!(f.requests().len(), 4);
    assert_eq!(
        notes(&f),
        ["This chat is too full to continue. Use /compact to shorten it, then say continue."]
    );
    // /compact, then "continue".
    f.engine.clear();
    f.engine.push(vec![say("SUMMARY")]);
    let c = compact::compact_chat(&f.state, &f.chat, SMALL_MODEL, None)
        .await
        .unwrap();
    assert_eq!(c.status, TurnStatus::Done);
    f.continues(SMALL_MODEL).await;
}

// ---------------------------------------------------------------------------
// Carried over from Task 11
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_the_folder_with_a_background_command_running_ends_the_task_with_the_folder_reason()
{
    let f = fx(vec![
        calls(
            "",
            vec![call(
                "c1",
                "run_command",
                json!({"command": sleeper(), "background": true}),
            )],
        ),
        Step::UntilCancelled("Waiting for the server".into()),
    ]);
    let close = async {
        f.answer_cards(vec![Answer::Once]).await;
        f.wait_for(|e| matches!(e, EngineEvent::TextDelta { .. }))
            .await;
        folder::set_chat_folder(&f.state, &f.chat, None)
            .await
            .unwrap();
    };
    let (r, ()) = tokio::join!(f.send("start the server"), close);
    assert_eq!(r.status, TurnStatus::Interrupted);
    assert_eq!(r.reason.as_deref(), Some("folder_missing"));
    assert_eq!(notes(&f), [FOLDER_GONE]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_retry_with_nothing_to_retry_leaves_a_running_task_alone() {
    let f = fx(vec![card_then_reads(), say("finished")]);
    let retry = async {
        f.wait_for(card_waits).await;
        let emit = f.emitter();
        // The model has no tools: the one-request path. The last row is a
        // reply with calls, so there is nothing to retry.
        let r = turn::retry_turn(&f.state, &f.chat, PLAIN_MODEL, false, &emit).await;
        assert_eq!(
            r.unwrap_err().message,
            "There is nothing to retry in this chat."
        );
        assert!(
            f.state.approvals.waiting(&f.chat).is_some(),
            "the card still waits"
        );
        f.answer_cards(vec![Answer::Once]).await;
    };
    let (r, ()) = tokio::join!(f.send("write"), retry);
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert!(f.path("b.txt").exists());
}

// ---------------------------------------------------------------------------
// Money guards carried from the review of the request guards
// ---------------------------------------------------------------------------

/// A reply whose provider reported no cost.
fn unpriced(text: &str, prompt_tokens: Option<u32>) -> Step {
    Step::Reply(StepOutcome {
        text: text.into(),
        cost_usd: None,
        prompt_tokens,
        ..StepOutcome::default()
    })
}

fn recorded_cost(f: &Fx) -> f64 {
    f.state.store.chat_usage(&f.chat).unwrap().1
}

#[tokio::test]
async fn a_paid_project_reply_with_no_reported_cost_records_an_estimate() {
    // 1,000 prompt tokens at $1 a million, and 8 characters (2 tokens) at $2.
    let f = fx(vec![unpriced("abcdefgh", Some(1000))]);
    assert_eq!(f.send_on("go", PAID_TOOLS).await.status, TurnStatus::Done);
    assert!((recorded_cost(&f) - 0.001_004).abs() < 1e-12);
    let shown: Vec<f64> = f
        .events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::Usage { cost_usd, .. } => Some(cost_usd),
            _ => None,
        })
        .collect();
    assert_eq!(shown.len(), 1);
    assert!((shown[0] - 0.001_004).abs() < 1e-12);
}

#[tokio::test]
async fn without_reported_prompt_tokens_the_estimate_uses_the_request_characters() {
    let f = fx(vec![unpriced("abcdefgh", None)]);
    f.send_on("go", PAID_TOOLS).await;
    let request = &f.requests()[0];
    let chars = request
        .messages
        .iter()
        .map(chat_state::wire_chars)
        .sum::<usize>()
        + schema::tools().to_string().chars().count();
    let expected = (chars as f64 / 4.0) * 1.0 / 1e6 + 2.0 * 2.0 / 1e6;
    assert!(
        (recorded_cost(&f) - expected).abs() < 1e-9,
        "{} vs {expected}",
        recorded_cost(&f)
    );
}

#[tokio::test]
async fn a_free_model_or_a_reported_cost_gets_no_estimate() {
    let f = fx(vec![unpriced("abcdefgh", Some(1000))]);
    f.send("go").await;
    assert_eq!(recorded_cost(&f), 0.0);
    let f = fx(vec![costing(0.5, say("abcdefgh"))]);
    f.send_on("go", PAID_TOOLS).await;
    assert!((recorded_cost(&f) - 0.5).abs() < 1e-12);
}

#[tokio::test]
async fn a_paid_plain_reply_with_no_reported_cost_records_an_estimate() {
    let f = fx(vec![unpriced("abcd", Some(2000))]);
    assert_eq!(f.send_on("go", PAID_PLAIN).await.status, TurnStatus::Done);
    // 2,000 tokens at $1 a million and 1 token at $2.
    assert!((recorded_cost(&f) - 0.002_002).abs() < 1e-12);
}

#[tokio::test]
async fn a_model_missing_from_the_model_list_records_no_estimate() {
    let f = fx(vec![unpriced("abcd", Some(2000))]);
    assert_eq!(f.send_on("go", "m/unknown").await.status, TurnStatus::Done);
    assert_eq!(recorded_cost(&f), 0.0);
    assert_eq!(f.state.store.chat_usage(&f.chat).unwrap().0, 1);
}

#[tokio::test]
async fn a_paid_request_is_refused_when_todays_spend_cannot_be_read() {
    let f = fx(vec![]);
    f.state
        .store
        .exec_for_test("DROP TABLE daily_usage;")
        .unwrap();
    for model in [PAID_PLAIN, PAID_TOOLS] {
        let e = f.try_send_on("hi", model).await.unwrap_err();
        assert_eq!(e.kind, "daily_limit", "{model}");
        assert_eq!(e.message, LIMIT_REACHED);
    }
    assert!(f.requests().is_empty());
    // A free model is not held back by it, and neither is "no limit".
    f.engine.push(vec![say("free")]);
    assert_eq!(f.send("hi").await.status, TurnStatus::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_free_rate_limit_with_no_free_requests_left_pauses_at_once_without_waiting() {
    let f = fx(vec![list("c1"), rate_limited(Some(30))]);
    // Plenty at the start, none when the rate limit arrives.
    f.outside.statuses.lock().unwrap().push_back(free_left(10));
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(0).unwrap());
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Paused, "{r:?}");
    assert_eq!(r.reason.as_deref(), Some("no_free_requests"));
    assert!(f.outside.waits.lock().unwrap().is_empty());
    assert!(
        !f.events()
            .iter()
            .any(|e| matches!(e, EngineEvent::Waiting { .. }))
    );
    assert_eq!(
        notes(&f),
        ["Paused: no free requests are left today. They reset at midnight UTC."]
    );
    assert_rows_paired(&f);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_free_rate_limit_with_requests_left_is_still_waited_out() {
    let f = fx(vec![list("c1"), rate_limited(Some(2)), say("ok")]);
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(5).unwrap());
    let r = f.send("go").await;
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert_eq!(*f.outside.waits.lock().unwrap(), [2]);
}

#[path = "interface_tests.rs"]
mod interface;
