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
    let state = AppState::with_engine(
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
        ],
    )
    .unwrap();
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
    assert_eq!(f.shape(), ["user", "note", "note"]);
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
async fn the_guards_task_12_fills_in_let_the_task_continue_by_default() {
    let mut g = Guards::default();
    let mut messages = Vec::new();
    assert_eq!(g.before_request(1, &mut messages), Next::Continue);
    assert_eq!(g.keep_going(25).await, Next::Continue);
    let outcome = crate::agent::ToolOutcome {
        status: crate::agent::ToolStatus::Error,
        text: "x".into(),
        summary: "x".into(),
        detail: None,
    };
    assert_eq!(
        g.after_call(&call("c", "read_file", json!({})), &outcome),
        Next::Continue
    );
    let e = StepError {
        ui: UiError::from(ApiError::RateLimited {
            retry_after_secs: Some(1),
        }),
        retry_after_secs: Some(1),
    };
    assert!(!g.retry_after(&e).await);
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
async fn a_retry_during_a_running_task_waits_for_it_and_continues() {
    let f = fx(vec![card_then_reads(), say("continued")]);
    let retry = async {
        f.wait_for(card_waits).await;
        let emit = f.emitter();
        turn::retry_turn(&f.state, &f.chat, TOOLS_MODEL, false, &emit)
            .await
            .unwrap()
    };
    let (_, second) = tokio::join!(f.send("write"), retry);
    assert_eq!(second.status, TurnStatus::Done);
    assert_eq!(
        f.shape(),
        [
            "user",
            "assistant[write_file,read_file,list_dir]",
            "tool:not_run",
            "tool:not_run",
            "tool:not_run",
            "assistant"
        ]
    );
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
    folder_changed_while_a_card_waits(Some(real(other.path()))).await;
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
