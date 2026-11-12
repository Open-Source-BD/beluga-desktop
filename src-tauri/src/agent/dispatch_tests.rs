//! The dispatcher against real tools, a temp project, an in-memory store and
//! answers given through `Approvals::answer` the way the interface gives them.

use super::*;
use crate::agent::approval::NOT_WAITING;
use crate::agent::shell::detect;
use crate::agent::test_support::{long, pick};
use crate::agent::{Answer, Approval, ToolStatus};
use crate::state::EventSink;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: &str, _: Value) {}
}

type Events = Arc<Mutex<Vec<EngineEvent>>>;

struct Fx {
    dir: tempfile::TempDir,
    project: Project,
    shell: Shell,
    store: Store,
    library: Arc<Library>,
    approvals: Approvals,
    background: Background,
    read_log: Arc<Mutex<ReadLog>>,
    events: Events,
    emit: Box<dyn Fn(EngineEvent) + Send + Sync>,
    cancel: CancellationToken,
    chat: String,
    next: AtomicU32,
    hook: Hook,
}

type Hook = Arc<Mutex<Option<Box<dyn Fn(&EngineEvent) + Send + Sync>>>>;

fn real_str(p: &std::path::Path) -> String {
    std::fs::canonicalize(p)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

impl Fx {
    fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = real_str(dir.path());
        let project = Project::open(&root).unwrap();
        let store = Store::in_memory().unwrap();
        let chat = store.create_chat("m/a").unwrap().id;
        store.set_folder(&chat, Some(&root)).unwrap();
        let none = dir.path().join("no-skills");
        let events: Events = Arc::default();
        let sink = events.clone();
        let hook: Hook = Arc::default();
        let on_event = hook.clone();
        Fx {
            project,
            shell: detect(),
            store,
            library: Arc::new(Library::load(&none, &none)),
            approvals: Approvals::new(),
            background: Background::new(Arc::new(NullSink)),
            read_log: Arc::default(),
            events,
            emit: Box::new(move |e| {
                if let Some(f) = on_event.lock().unwrap().as_ref() {
                    f(&e);
                }
                sink.lock().unwrap().push(e);
            }),
            cancel: CancellationToken::new(),
            chat,
            next: AtomicU32::new(1),
            hook,
            dir,
        }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            chat_id: &self.chat,
            project: &self.project,
            shell: &self.shell,
            store: &self.store,
            library: &self.library,
            approvals: &self.approvals,
            background: &self.background,
            read_log: &self.read_log,
            emit: &*self.emit,
            cancel: &self.cancel,
        }
    }

    fn folder(&self) -> String {
        let s = self.project.root.to_str().unwrap().to_string();
        s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }

    fn write(&self, name: &str, text: &str) {
        let p = self.path(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap()
    }

    /// Runs `f` on every event as it is emitted, before it is recorded.
    fn on_event(&self, f: impl Fn(&EngineEvent) + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }

    fn events(&self) -> Vec<EngineEvent> {
        self.events.lock().unwrap().clone()
    }

    fn cards(&self) -> Vec<Approval> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                EngineEvent::ApprovalNeeded { approval } => Some(approval),
                _ => None,
            })
            .collect()
    }

    fn tool_call(&self, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{}", self.next.fetch_add(1, Ordering::SeqCst)),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    /// Runs a call that must not show a card.
    async fn call(&self, name: &str, args: Value) -> Dispatched {
        self.call_raw(name, &args.to_string()).await
    }

    async fn call_raw(&self, name: &str, args: &str) -> Dispatched {
        let before = self.cards().len();
        let call = self.tool_call(name, args);
        let d = tokio::time::timeout(Duration::from_secs(long(30)), run_call(&self.ctx(), &call))
            .await
            .expect("the call waited (a card was shown?)");
        assert_eq!(self.cards().len(), before, "{name} showed a card");
        d
    }

    /// Waits for the card after the first `before` and returns it.
    async fn next_card(&self, before: usize) -> Approval {
        tokio::time::timeout(Duration::from_secs(long(10)), async {
            loop {
                if let Some(c) = self.cards().get(before) {
                    return c.clone();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("no card was shown")
    }

    /// Runs a call that shows a card and answers it with `answer`.
    async fn answered(&self, name: &str, args: Value, answer: Answer) -> (Dispatched, Approval) {
        let before = self.cards().len();
        let call = self.tool_call(name, &args.to_string());
        let ctx = self.ctx();
        let (d, card) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(long(30)), run_call(&ctx, &call)),
            async {
                let card = self.next_card(before).await;
                self.approvals
                    .answer(&self.chat, card.id(), answer)
                    .expect("the answer was refused");
                card
            }
        );
        (d.expect("the call did not finish"), card)
    }

    /// Reads the file through the dispatcher, as the model would first.
    async fn model_reads(&self, name: &str) {
        let d = self.call("read_file", json!({"path": name})).await;
        assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    }
}

fn declined(d: &Dispatched) {
    assert_eq!(d.outcome.status, ToolStatus::Declined, "{}", d.outcome.text);
}

fn edit_args(path: &str, old: &str, new: &str) -> Value {
    json!({"path": path, "old_text": old, "new_text": new})
}

// ---------------------------------------------------------------------------
// Read tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_tools_run_with_no_approval_and_emit_started_then_finished() {
    let fx = Fx::new();
    fx.write("notes.txt", "alpha\nbeta\n");
    let calls = [
        ("read_file", json!({"path": "notes.txt"})),
        ("list_dir", json!({})),
        ("find_files", json!({"pattern": "*.txt"})),
        ("search", json!({"pattern": "alpha"})),
        (
            "update_plan",
            json!({"items": [{"text": "Look", "state": "in_progress"}]}),
        ),
    ];
    for (name, args) in calls {
        let before = fx.events().len();
        let d = fx.call(name, args).await;
        assert_eq!(
            d.outcome.status,
            ToolStatus::Ok,
            "{name}: {}",
            d.outcome.text
        );
        assert_eq!(d.user_message, None);
        let ev = fx.events()[before..].to_vec();
        let EngineEvent::ToolStarted {
            call_id, name: n, ..
        } = &ev[0]
        else {
            panic!("{name}: {ev:?}");
        };
        assert_eq!(n, name);
        let finished = ev.last().unwrap();
        assert_eq!(
            finished,
            &EngineEvent::ToolFinished {
                call_id: call_id.clone(),
                status: ToolStatus::Ok,
                summary: d.outcome.summary.clone(),
                detail: d.outcome.detail.clone(),
            },
            "{name}"
        );
    }
    let d = fx.call("read_file", json!({"path": "notes.txt"})).await;
    assert!(d.outcome.text.contains("1\talpha"));
}

#[tokio::test]
async fn update_plan_saves_the_plan_and_emits_it() {
    let fx = Fx::new();
    fx.call(
        "update_plan",
        json!({"items": [{"text": "One", "state": "done"}, {"text": "Two", "state": "pending"}]}),
    )
    .await;
    let saved: Value = serde_json::from_str(&fx.store.plan(&fx.chat).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!([{"text": "One", "state": "done"}, {"text": "Two", "state": "pending"}])
    );
    assert!(fx.events().iter().any(|e| matches!(
        e,
        EngineEvent::PlanUpdated { items } if items.len() == 2
    )));
    // A broken plan keeps the saved one.
    let d = fx.call("update_plan", json!({"items": "x"})).await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    let again: Value = serde_json::from_str(&fx.store.plan(&fx.chat).unwrap()).unwrap();
    assert_eq!(again, saved);
}

#[tokio::test]
async fn a_read_is_recorded_so_an_edit_may_follow_and_a_partial_read_counts() {
    let fx = Fx::new();
    fx.write("a.txt", "one\ntwo\nthree\n");
    // Not read yet: the edit is refused before any card.
    let d = fx.call("edit_file", edit_args("a.txt", "two", "2")).await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert!(
        d.outcome.text.contains("Read a.txt first"),
        "{}",
        d.outcome.text
    );
    let d = fx
        .call(
            "read_file",
            json!({"path": "a.txt", "offset": 1, "limit": 1}),
        )
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Ok);
    let (d, _) = fx
        .answered("edit_file", edit_args("a.txt", "two", "2"), Answer::Once)
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    assert_eq!(fx.read("a.txt"), "one\n2\nthree\n");
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_once_applies_and_the_next_edit_asks_again() {
    let fx = Fx::new();
    fx.write("a.txt", "one\ntwo\n");
    fx.model_reads("a.txt").await;
    let (d, card) = fx
        .answered("edit_file", edit_args("a.txt", "one", "uno"), Answer::Once)
        .await;
    assert!(matches!(card, Approval::Edit { .. }));
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    assert_eq!(fx.read("a.txt"), "uno\ntwo\n");
    assert!(!fx.store.edits_allowed(&fx.chat).unwrap());
    let (d, _) = fx
        .answered("edit_file", edit_args("a.txt", "two", "dos"), Answer::Once)
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Ok);
    assert_eq!(fx.cards().len(), 2);
    assert_eq!(fx.read("a.txt"), "uno\ndos\n");
}

#[tokio::test]
async fn edit_all_edits_applies_sets_the_chat_flag_and_later_changes_do_not_ask() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    let (d, _) = fx
        .answered(
            "edit_file",
            edit_args("a.txt", "one", "uno"),
            Answer::AllEdits,
        )
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Ok);
    assert!(fx.store.edits_allowed(&fx.chat).unwrap());
    let d = fx
        .call("edit_file", edit_args("a.txt", "uno", "eins"))
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    let d = fx
        .call(
            "write_file",
            json!({"path": "new/b.txt", "content": "hi\n"}),
        )
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    assert_eq!(fx.read("a.txt"), "eins\n");
    assert_eq!(fx.read("new/b.txt"), "hi\n");
    assert_eq!(fx.cards().len(), 1);
}

#[tokio::test]
async fn edit_decline_leaves_the_file_and_returns_the_declined_text() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    let (d, _) = fx
        .answered(
            "edit_file",
            edit_args("a.txt", "one", "uno"),
            Answer::Decline { note: None },
        )
        .await;
    declined(&d);
    assert_eq!(d.outcome.text, "The user declined.");
    assert_eq!(d.outcome.summary, "Declined: edit a.txt");
    assert_eq!(d.user_message, None);
    let (d, _) = fx
        .answered(
            "edit_file",
            edit_args("a.txt", "one", "uno"),
            Answer::Decline {
                note: Some("keep it English".into()),
            },
        )
        .await;
    declined(&d);
    assert_eq!(
        d.outcome.text,
        "The user declined and said: keep it English"
    );
    assert_eq!(fx.read("a.txt"), "one\n");
    let (d, _) = fx
        .answered(
            "write_file",
            json!({"path": "c.txt", "content": "x"}),
            Answer::Decline { note: None },
        )
        .await;
    assert_eq!(d.outcome.summary, "Declined: create c.txt");
    assert!(!fx.path("c.txt").exists());
}

#[tokio::test]
async fn the_edit_card_carries_the_real_path_created_flag_and_changed_lines() {
    let fx = Fx::new();
    fx.write("src/app.ts", "let a = 1;\nlet b = 2;\n");
    fx.model_reads("src/app.ts").await;
    let (_, card) = fx
        .answered(
            "edit_file",
            edit_args("./src/./app.ts", "let a = 1;", "let a = 10;\nlet z = 0;"),
            Answer::Decline { note: None },
        )
        .await;
    assert_eq!(
        card,
        Approval::Edit {
            id: card.id().to_string(),
            path: "src/app.ts".into(),
            created: false,
            removed: vec!["let a = 1;".into()],
            added: vec!["let a = 10;".into(), "let z = 0;".into()],
            removed_total: 1,
            added_total: 2,
            total_lines: 3,
        }
    );
    let content: String = (1..=50).map(|n| format!("line {n}\n")).collect();
    let (_, card) = fx
        .answered(
            "write_file",
            json!({"path": "made/new.txt", "content": content}),
            Answer::Decline { note: None },
        )
        .await;
    let Approval::Edit {
        path,
        created,
        removed,
        added,
        added_total,
        total_lines,
        ..
    } = card
    else {
        panic!("{card:?}");
    };
    assert_eq!(path, "made/new.txt");
    assert!(created);
    assert!(removed.is_empty());
    assert_eq!(added.len(), 40);
    assert_eq!((added_total, total_lines), (50, 50));
    assert!(!fx.path("made").exists());
}

#[tokio::test]
async fn a_file_changed_while_the_edit_card_waits_is_not_overwritten() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    let before = fx.cards().len();
    let call = fx.tool_call("edit_file", &edit_args("a.txt", "one", "uno").to_string());
    let ctx = fx.ctx();
    let (d, _) = tokio::join!(run_call(&ctx, &call), async {
        let card = fx.next_card(before).await;
        fx.write("a.txt", "someone else's text\n");
        fx.approvals
            .answer(&fx.chat, card.id(), Answer::Once)
            .unwrap();
    });
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert!(d.outcome.text.contains("changed"), "{}", d.outcome.text);
    assert_eq!(fx.read("a.txt"), "someone else's text\n");
}

#[tokio::test]
async fn secret_files_are_never_changed_and_never_asked_about() {
    let fx = Fx::new();
    fx.write(".env", "KEY=1\n");
    fx.store.set_edits_allowed(&fx.chat, true).unwrap();
    for (name, args) in [
        ("write_file", json!({"path": ".env", "content": "KEY=2\n"})),
        ("edit_file", edit_args(".env", "KEY=1", "KEY=2")),
        (
            "write_file",
            json!({"path": ".git-credentials", "content": "x"}),
        ),
    ] {
        let d = fx.call(name, args).await;
        assert_eq!(d.outcome.status, ToolStatus::Error, "{name}");
    }
    assert_eq!(fx.read(".env"), "KEY=1\n");
    assert!(!fx.path(".git-credentials").exists());
}

#[tokio::test]
async fn a_failed_edits_flag_read_changes_nothing() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    fx.store
        .exec_for_test("ALTER TABLE chats RENAME COLUMN edits_allowed TO gone")
        .unwrap();
    let d = fx.call("edit_file", edit_args("a.txt", "one", "uno")).await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert_eq!(fx.read("a.txt"), "one\n");
}

#[tokio::test]
async fn all_edits_that_cannot_be_saved_applies_once_and_says_so() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    fx.store
        .exec_for_test(
            "CREATE TRIGGER no_flag BEFORE UPDATE OF edits_allowed ON chats \
             BEGIN SELECT RAISE(FAIL, 'no'); END;",
        )
        .unwrap();
    let (d, _) = fx
        .answered(
            "edit_file",
            edit_args("a.txt", "one", "uno"),
            Answer::AllEdits,
        )
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Ok);
    assert!(
        d.outcome.text.contains("could not be saved"),
        "{}",
        d.outcome.text
    );
    assert_eq!(fx.read("a.txt"), "uno\n");
    assert!(!fx.store.edits_allowed(&fx.chat).unwrap());
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tokio::test]
async fn command_once_runs_and_streams_its_output() {
    let fx = Fx::new();
    let (d, card) = fx
        .answered(
            "run_command",
            json!({"command": "echo hello"}),
            Answer::Once,
        )
        .await;
    assert!(matches!(card, Approval::Command { .. }));
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    assert!(
        d.outcome.text.starts_with("exit code 0"),
        "{}",
        d.outcome.text
    );
    assert!(d.outcome.text.contains("hello"));
    assert!(
        d.outcome.summary.starts_with("Ran echo hello (exit 0, "),
        "{}",
        d.outcome.summary
    );
    assert!(d.outcome.summary.ends_with(" s)"));
    assert!(d.outcome.detail.as_deref().unwrap().contains("hello"));
    let streamed: String = fx
        .events()
        .iter()
        .filter_map(|e| match e {
            EngineEvent::ToolOutput { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(streamed.contains("hello"));
}

#[tokio::test]
async fn command_always_saves_the_rule_and_a_matching_command_then_runs_without_asking() {
    let fx = Fx::new();
    fx.write("notes.txt", "alpha\n");
    fx.write("other.txt", "beta\n");
    let (d, card) = fx
        .answered(
            "run_command",
            json!({"command": "cat notes.txt"}),
            Answer::Always,
        )
        .await;
    let Approval::Command { rule, .. } = &card else {
        panic!("{card:?}");
    };
    assert_eq!(rule.as_deref(), Some("cat"));
    assert!(d.outcome.text.contains("alpha"), "{}", d.outcome.text);
    assert_eq!(
        fx.store.rules(&fx.folder()).unwrap(),
        vec!["cat".to_string()]
    );
    let d = fx
        .call("run_command", json!({"command": "cat other.txt"}))
        .await;
    assert!(d.outcome.text.contains("beta"), "{}", d.outcome.text);
    // A form the saved rule does not match asks again, and offers no rule
    // that is already saved.
    let (d, card) = fx
        .answered(
            "run_command",
            json!({"command": "cat -n notes.txt"}),
            Answer::Decline { note: None },
        )
        .await;
    declined(&d);
    let Approval::Command { rule, .. } = card else {
        panic!();
    };
    assert_eq!(rule, None);
}

#[tokio::test]
async fn a_compound_command_offers_no_rule_and_always_is_refused() {
    let fx = Fx::new();
    let before = fx.cards().len();
    let command = pick(&fx.shell, "echo a && echo b", "echo a; echo b").to_string();
    let call = fx.tool_call("run_command", &json!({"command": command}).to_string());
    let ctx = fx.ctx();
    let (d, _) = tokio::join!(run_call(&ctx, &call), async {
        let card = fx.next_card(before).await;
        let Approval::Command { rule, .. } = &card else {
            panic!("{card:?}");
        };
        assert_eq!(*rule, None);
        let e = fx
            .approvals
            .answer(&fx.chat, card.id(), Answer::Always)
            .unwrap_err();
        assert_eq!(e.message, NOT_WAITING);
        let e = fx
            .approvals
            .answer(&fx.chat, card.id(), Answer::AllEdits)
            .unwrap_err();
        assert_eq!(e.message, NOT_WAITING);
        fx.approvals
            .answer(&fx.chat, card.id(), Answer::Decline { note: None })
            .unwrap();
    });
    declined(&d);
    assert!(fx.store.rules(&fx.folder()).unwrap().is_empty());
}

#[tokio::test]
async fn a_saved_rule_never_lets_a_compound_command_through() {
    let fx = Fx::new();
    fx.store.add_rule(&fx.folder(), "npm install").unwrap();
    fx.store.add_rule(&fx.folder(), "cat").unwrap();
    for command in [
        "npm install && rm -rf .",
        "npm install lodash",
        "cat notes.txt; echo x > made.txt",
    ] {
        let (d, card) = fx
            .answered(
                "run_command",
                json!({"command": command}),
                Answer::Decline { note: None },
            )
            .await;
        declined(&d);
        let Approval::Command { command: shown, .. } = card else {
            panic!();
        };
        assert_eq!(shown, command);
    }
    assert!(!fx.path("made.txt").exists());
}

#[tokio::test]
async fn the_command_card_carries_the_whole_command_folder_outside_hint_and_first_line_once() {
    let fx = Fx::new();
    let outside = pick(&fx.shell, "cat /etc/hosts", r"cat C:\Windows\win.ini").to_string();
    let (_, card) = fx
        .answered(
            "run_command",
            json!({"command": outside}),
            Answer::Decline { note: None },
        )
        .await;
    assert_eq!(
        card,
        Approval::Command {
            id: card.id().to_string(),
            command: outside.clone(),
            folder: fx.folder(),
            // The card offers the program; a saved `cat` never matches a
            // command that refers outside the project.
            rule: Some("cat".into()),
            outside: true,
            first_in_project: true,
            background: false,
        }
    );
    // Declining keeps the first-command line; approving ends it.
    let (_, card) = fx
        .answered("run_command", json!({"command": "echo one"}), Answer::Once)
        .await;
    let Approval::Command {
        first_in_project,
        outside,
        ..
    } = card
    else {
        panic!();
    };
    assert!(first_in_project);
    assert!(!outside);
    let (_, card) = fx
        .answered(
            "run_command",
            json!({"command": "echo two"}),
            Answer::Decline { note: None },
        )
        .await;
    let Approval::Command {
        first_in_project, ..
    } = card
    else {
        panic!();
    };
    assert!(!first_in_project);
}

#[tokio::test]
async fn a_long_command_shows_complete_on_the_card() {
    let fx = Fx::new();
    let command = format!("echo {}", "x".repeat(3_900));
    let (_, card) = fx
        .answered(
            "run_command",
            json!({"command": command}),
            Answer::Decline { note: None },
        )
        .await;
    let Approval::Command { command: shown, .. } = card else {
        panic!();
    };
    assert_eq!(shown, command);
}

#[tokio::test]
async fn a_message_on_a_command_card_runs_nothing_and_is_passed_on() {
    let fx = Fx::new();
    let command = pick(
        &fx.shell,
        "echo x > made.txt",
        "Set-Content -Path made.txt -Value x",
    );
    let (d, _) = fx
        .answered(
            "run_command",
            json!({"command": command}),
            Answer::Message {
                text: "use the other script".into(),
            },
        )
        .await;
    declined(&d);
    assert_eq!(
        d.outcome.text,
        "The user declined and said: use the other script"
    );
    assert_eq!(d.user_message.as_deref(), Some("use the other script"));
    assert!(!fx.path("made.txt").exists());
}

#[tokio::test]
async fn a_declined_command_leaves_no_file_and_no_process() {
    let fx = Fx::new();
    let command = pick(
        &fx.shell,
        "echo x > made.txt; exec sleep 30",
        "Set-Content -Path made.txt -Value x; Start-Sleep 60",
    );
    for bg in [false, true] {
        let (d, _) = fx
            .answered(
                "run_command",
                json!({"command": command, "background": bg}),
                Answer::Decline { note: None },
            )
            .await;
        declined(&d);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!fx.path("made.txt").exists());
    assert!(fx.background.list(&fx.chat).is_empty());
}

#[tokio::test]
async fn commands_too_long_or_with_nul_are_refused_without_asking() {
    let fx = Fx::new();
    for command in [
        format!("echo {}", "x".repeat(4_000)),
        "echo a\0b".to_string(),
        vec!["echo a"; 51].join("\n"),
    ] {
        let d = fx.call("run_command", json!({"command": command})).await;
        assert_eq!(d.outcome.status, ToolStatus::Error);
    }
    // The size refusals say what to do instead.
    for command in [
        format!("echo {}", "x".repeat(4_000)),
        vec!["echo a"; 51].join("\n"),
    ] {
        let d = fx.call("run_command", json!({"command": command})).await;
        assert_eq!(
            d.outcome.text,
            "That command is too long to run. Put it in a script file and run the file."
        );
    }
}

#[tokio::test]
async fn a_failed_rules_read_runs_nothing() {
    let fx = Fx::new();
    fx.store.exec_for_test("DROP TABLE command_rules").unwrap();
    let d = fx.call("run_command", json!({"command": "echo hi"})).await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert!(d.outcome.text.contains("not run"), "{}", d.outcome.text);
}

#[tokio::test]
async fn always_that_cannot_be_saved_runs_once_and_says_so() {
    let fx = Fx::new();
    fx.write("notes.txt", "alpha\n");
    fx.store
        .exec_for_test(
            "CREATE TRIGGER no_rule BEFORE INSERT ON command_rules \
             BEGIN SELECT RAISE(FAIL, 'no'); END;",
        )
        .unwrap();
    let (d, _) = fx
        .answered(
            "run_command",
            json!({"command": "cat notes.txt"}),
            Answer::Always,
        )
        .await;
    assert!(d.outcome.text.contains("alpha"), "{}", d.outcome.text);
    assert!(
        d.outcome.text.contains("The rule could not be saved"),
        "{}",
        d.outcome.text
    );
    // Not saved, so the next one asks again.
    let (d, _) = fx
        .answered(
            "run_command",
            json!({"command": "cat notes.txt"}),
            Answer::Decline { note: None },
        )
        .await;
    declined(&d);
}

#[tokio::test]
async fn background_true_starts_after_approval_and_returns_its_id() {
    let fx = Fx::new();
    let command = pick(
        &fx.shell,
        "echo started; exec sleep 30",
        "Write-Output started; Start-Sleep 60",
    );
    let (d, card) = fx
        .answered(
            "run_command",
            json!({"command": command, "background": true}),
            Answer::Once,
        )
        .await;
    let Approval::Command { background, .. } = card else {
        panic!();
    };
    assert!(background);
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    assert!(
        d.outcome.text.starts_with(
            "Started in the background as bg1. Use command_output to read what it prints and stop_command to stop it."
        ),
        "{}",
        d.outcome.text
    );
    assert_eq!(fx.background.list(&fx.chat).len(), 1);
    let d = fx.call("command_output", json!({"id": "bg1"})).await;
    assert_eq!(d.outcome.status, ToolStatus::Ok);
    assert!(
        d.outcome.text.contains("still running"),
        "{}",
        d.outcome.text
    );
    let d = fx.call("stop_command", json!({"id": "bg1"})).await;
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    let d = fx.call("command_output", json!({"id": "bg9"})).await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert_eq!(
        d.outcome.text,
        "There is no background command bg9 in this chat."
    );
    fx.background.stop_all().await;
}

#[tokio::test]
async fn a_background_start_is_refused_when_the_folder_changed_while_asking() {
    let fx = Fx::new();
    let other = tempfile::tempdir().unwrap();
    let command = pick(
        &fx.shell,
        "echo x > made.txt",
        "Set-Content -Path made.txt -Value x",
    );
    for bg in [true, false] {
        let before = fx.cards().len();
        let call = fx.tool_call(
            "run_command",
            &json!({"command": command, "background": bg}).to_string(),
        );
        let ctx = fx.ctx();
        let (d, _) = tokio::join!(run_call(&ctx, &call), async {
            let card = fx.next_card(before).await;
            fx.store
                .set_folder(&fx.chat, Some(&real_str(other.path())))
                .unwrap();
            fx.approvals
                .answer(&fx.chat, card.id(), Answer::Once)
                .unwrap();
        });
        assert_eq!(d.outcome.status, ToolStatus::Error, "{}", d.outcome.text);
        fx.store
            .set_folder(&fx.chat, Some(fx.project.root.to_str().unwrap()))
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(fx.background.list(&fx.chat).is_empty());
    assert!(!fx.path("made.txt").exists());
}

#[tokio::test]
async fn every_tool_checks_the_folder_before_doing_anything() {
    let fx = Fx::new();
    fx.write("a.txt", "secret-ish\n");
    let other = tempfile::tempdir().unwrap();
    fx.store
        .set_folder(&fx.chat, Some(&real_str(other.path())))
        .unwrap();
    for (name, args) in [
        ("read_file", json!({"path": "a.txt"})),
        ("list_dir", json!({})),
        ("find_files", json!({"pattern": "*.txt"})),
        ("search", json!({"pattern": "secret"})),
        (
            "update_plan",
            json!({"items": [{"text": "x", "state": "pending"}]}),
        ),
        ("command_output", json!({"id": "bg1"})),
        ("stop_command", json!({"id": "bg1"})),
    ] {
        let d = fx.call(name, args).await;
        assert_eq!(d.outcome.status, ToolStatus::Error, "{name}");
        assert!(
            d.outcome.text.contains("closed or changed"),
            "{name}: {}",
            d.outcome.text
        );
        assert!(!d.outcome.text.contains("secret-ish"), "{name}");
    }
    assert_eq!(fx.store.plan(&fx.chat).unwrap(), "[]");
}

#[tokio::test]
async fn a_card_ended_by_a_folder_change_says_so() {
    let fx = Fx::new();
    let other = tempfile::tempdir().unwrap();
    let before = fx.cards().len();
    let call = fx.tool_call(
        "write_file",
        &json!({"path": "b.txt", "content": "x"}).to_string(),
    );
    let ctx = fx.ctx();
    let (d, _) = tokio::join!(run_call(&ctx, &call), async {
        fx.next_card(before).await;
        // What closing or changing the folder does.
        fx.store
            .set_folder(&fx.chat, Some(&real_str(other.path())))
            .unwrap();
        fx.approvals.drop_chat(&fx.chat);
    });
    assert_eq!(d.outcome.status, ToolStatus::NotRun, "{}", d.outcome.text);
    assert_eq!(
        d.outcome.text,
        "Not run: the project folder was closed or changed."
    );
    assert!(!fx.path("b.txt").exists());
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_secret_read_asks_and_once_returns_the_contents() {
    let fx = Fx::new();
    fx.write(".env", "KEY=1\n");
    let (d, card) = fx
        .answered("read_file", json!({"path": ".env"}), Answer::Once)
        .await;
    assert_eq!(
        card,
        Approval::Secret {
            id: card.id().to_string(),
            path: ".env".into()
        }
    );
    assert_eq!(d.outcome.status, ToolStatus::Ok, "{}", d.outcome.text);
    assert!(d.outcome.text.contains("KEY=1"));
    let (d, _) = fx
        .answered(
            "read_file",
            json!({"path": ".env"}),
            Answer::Decline { note: None },
        )
        .await;
    declined(&d);
    assert!(!d.outcome.text.contains("KEY=1"));
    assert_eq!(d.outcome.summary, "Declined: read .env");
}

#[cfg(unix)]
#[tokio::test]
async fn an_approved_secret_read_cannot_be_swapped_for_another_secret() {
    let fx = Fx::new();
    fx.write(".env", "KEY=1\n");
    fx.write("deploy/.env.production", "PROD=secret\n");
    let before = fx.cards().len();
    let call = fx.tool_call("read_file", &json!({"path": ".env"}).to_string());
    let ctx = fx.ctx();
    let (d, _) = tokio::join!(run_call(&ctx, &call), async {
        let card = fx.next_card(before).await;
        std::fs::remove_file(fx.path(".env")).unwrap();
        std::os::unix::fs::symlink("deploy/.env.production", fx.path(".env")).unwrap();
        fx.approvals
            .answer(&fx.chat, card.id(), Answer::Once)
            .unwrap();
    });
    assert_eq!(d.outcome.status, ToolStatus::Error, "{}", d.outcome.text);
    assert!(!d.outcome.text.contains("PROD"));
}

#[tokio::test]
async fn a_skill_path_is_read_without_asking() {
    let fx = Fx::new();
    let d = fx
        .call("read_file", json!({"path": "skill:nope/SKILL.md"}))
        .await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
}

// ---------------------------------------------------------------------------
// The gate itself
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrong_stale_or_other_chat_answers_are_refused_and_change_nothing() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    let other = fx.store.create_chat("m/a").unwrap().id;
    let before = fx.cards().len();
    let call = fx.tool_call("edit_file", &edit_args("a.txt", "one", "uno").to_string());
    let ctx = fx.ctx();
    let (d, _) = tokio::join!(run_call(&ctx, &call), async {
        let card = fx.next_card(before).await;
        for (chat, id) in [
            (fx.chat.as_str(), "stale"),
            (fx.chat.as_str(), ""),
            (other.as_str(), card.id()),
        ] {
            let e = fx.approvals.answer(chat, id, Answer::Once).unwrap_err();
            assert_eq!(e.message, NOT_WAITING);
        }
        // Always on an edit card.
        let e = fx
            .approvals
            .answer(&fx.chat, card.id(), Answer::Always)
            .unwrap_err();
        assert_eq!(e.message, NOT_WAITING);
        assert_eq!(fx.read("a.txt"), "one\n");
        assert_eq!(fx.approvals.waiting(&fx.chat).unwrap().id(), card.id());
        fx.approvals
            .answer(&fx.chat, card.id(), Answer::Decline { note: None })
            .unwrap();
        // The same answer again.
        let e = fx
            .approvals
            .answer(&fx.chat, card.id(), Answer::Once)
            .unwrap_err();
        assert_eq!(e.message, NOT_WAITING);
    });
    declined(&d);
    assert_eq!(fx.read("a.txt"), "one\n");
}

#[tokio::test]
async fn an_answer_for_chat_b_cannot_approve_chat_a() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    // Chat B waits on its own card at the same time.
    let b = fx.store.create_chat("m/a").unwrap().id;
    let b_card = Approval::Secret {
        id: String::new(),
        path: ".env".into(),
    };
    let before = fx.cards().len();
    let call = fx.tool_call("edit_file", &edit_args("a.txt", "one", "uno").to_string());
    let ctx = fx.ctx();
    let (d, b_answer, _) = tokio::join!(
        run_call(&ctx, &call),
        fx.approvals.ask(&b, b_card, &*fx.emit, &fx.cancel),
        async {
            let first = fx.next_card(before).await;
            let second = fx.next_card(before + 1).await;
            let (a_card, b_card) = if matches!(first, Approval::Edit { .. }) {
                (first, second)
            } else {
                (second, first)
            };
            let e = fx
                .approvals
                .answer(&fx.chat, b_card.id(), Answer::Once)
                .unwrap_err();
            assert_eq!(e.message, NOT_WAITING);
            assert_eq!(fx.read("a.txt"), "one\n");
            fx.approvals
                .answer(&fx.chat, a_card.id(), Answer::Decline { note: None })
                .unwrap();
            fx.approvals.answer(&b, b_card.id(), Answer::Once).unwrap();
        }
    );
    declined(&d);
    assert_eq!(b_answer, Ok(Answer::Once));
    assert_eq!(fx.read("a.txt"), "one\n");
}

#[tokio::test]
async fn cancel_while_waiting_is_not_run_clears_the_card_and_a_late_answer_changes_nothing() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    let before = fx.cards().len();
    let call = fx.tool_call("edit_file", &edit_args("a.txt", "one", "uno").to_string());
    let ctx = fx.ctx();
    let (d, card) = tokio::join!(run_call(&ctx, &call), async {
        let card = fx.next_card(before).await;
        fx.cancel.cancel();
        card
    });
    assert_eq!(d.outcome.status, ToolStatus::NotRun);
    assert_eq!(d.outcome.text, "Not run: the task was stopped.");
    assert_eq!(d.outcome.summary, "Not run: edit a.txt");
    assert_eq!(d.user_message, None);
    assert!(fx.approvals.waiting(&fx.chat).is_none());
    assert!(fx.events().contains(&EngineEvent::ApprovalCleared {
        id: card.id().to_string()
    }));
    let e = fx
        .approvals
        .answer(&fx.chat, card.id(), Answer::Once)
        .unwrap_err();
    assert_eq!(e.message, NOT_WAITING);
    assert_eq!(fx.read("a.txt"), "one\n");
}

#[tokio::test]
async fn two_calls_in_one_chat_at_once_cannot_share_a_card() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    let before = fx.cards().len();
    let first = fx.tool_call("edit_file", &edit_args("a.txt", "one", "uno").to_string());
    let command = pick(
        &fx.shell,
        "echo x > made.txt",
        "Set-Content -Path made.txt -Value x",
    );
    let second = fx.tool_call("run_command", &json!({"command": command}).to_string());
    let ctx = fx.ctx();
    let (a, b) = tokio::join!(run_call(&ctx, &first), async {
        let card = fx.next_card(before).await;
        // The second call while the first card waits: refused, never shown.
        let b = run_call(&ctx, &second).await;
        assert_eq!(fx.approvals.waiting(&fx.chat).unwrap().id(), card.id());
        fx.approvals
            .answer(&fx.chat, card.id(), Answer::Once)
            .unwrap();
        b
    });
    assert_eq!(a.outcome.status, ToolStatus::Ok, "{}", a.outcome.text);
    assert_eq!(b.outcome.status, ToolStatus::Error);
    assert_eq!(
        b.outcome.text,
        "Another request is already waiting for the user's answer in this chat, so this one was not done."
    );
    assert_eq!(fx.cards().len(), before + 1);
    assert!(!fx.path("made.txt").exists());
    assert_eq!(fx.read("a.txt"), "uno\n");
}

#[tokio::test]
async fn unknown_tools_and_unreadable_arguments_are_errors_that_name_the_problem() {
    let fx = Fx::new();
    let d = fx.call_raw("delete_everything", "{}").await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert_eq!(
        d.outcome.text,
        "There is no tool named delete_everything. The tools are: read_file, list_dir, \
         find_files, search, edit_file, write_file, run_command, update_plan, command_output, \
         stop_command."
    );
    let d = fx.call_raw("read_file", "{\"path\": ").await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert!(
        d.outcome.text.contains("the arguments are not valid JSON"),
        "{}",
        d.outcome.text
    );
    let d = fx.call_raw("run_command", "[\"echo hi\"]").await;
    assert_eq!(d.outcome.status, ToolStatus::Error);
    assert!(
        d.outcome
            .text
            .contains("the arguments must be a JSON object"),
        "{}",
        d.outcome.text
    );
}

const TOOLS: [&str; 10] = [
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

#[tokio::test]
async fn every_tool_turns_bad_arguments_into_an_error_without_asking() {
    let fx = Fx::new();
    let big = "x".repeat(1_100_000);
    for name in TOOLS {
        let mut cases: Vec<String> = vec![
            "[1, 2]".into(),
            "\"text\"".into(),
            "42".into(),
            json!({"path": 5, "pattern": 5, "command": 5, "items": 5, "id": 5, "content": 5,
                   "old_text": 5, "new_text": 5})
            .to_string(),
            json!({"path": big, "pattern": big, "command": big, "items": [{"text": big, "state": "done"}],
                   "id": big, "content": big, "old_text": big, "new_text": "y"})
            .to_string(),
        ];
        // `list_dir` with no arguments lists the project folder.
        if name != "list_dir" {
            cases.push(String::new());
            cases.push("{}".into());
        }
        for args in cases {
            let d = fx.call_raw(name, &args).await;
            assert_eq!(
                d.outcome.status,
                ToolStatus::Error,
                "{name} {}: {}",
                &args[..args.len().min(60)],
                d.outcome.text
            );
        }
    }
    assert!(fx.background.list(&fx.chat).is_empty());
}

/// Every call gets exactly one ToolStarted, before exactly one ToolFinished,
/// whatever the tool and whatever the answer.
#[tokio::test]
async fn started_and_finished_pair_for_every_tool_and_every_answer() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.write(".env", "K=1\n");
    let mut ids = Vec::new();
    let mut record = |d: &Dispatched| ids.push(d.outcome.summary.clone());
    for name in TOOLS {
        record(&fx.call_raw(name, "[").await);
    }
    record(&fx.call("read_file", json!({"path": "a.txt"})).await);
    record(&fx.call("command_output", json!({"id": "bg1"})).await);
    record(&fx.call("stop_command", json!({"id": "bg1"})).await);
    record(&fx.call_raw("nope", "{}").await);
    for answer in [
        Answer::Once,
        Answer::Decline { note: None },
        Answer::Decline {
            note: Some("n".into()),
        },
        Answer::Message { text: "m".into() },
    ] {
        record(
            &fx.answered("read_file", json!({"path": ".env"}), answer.clone())
                .await
                .0,
        );
        record(
            &fx.answered("run_command", json!({"command": "echo hi"}), answer.clone())
                .await
                .0,
        );
        let (d, _) = fx
            .answered(
                "write_file",
                json!({"path": "w.txt", "content": "x"}),
                answer.clone(),
            )
            .await;
        record(&d);
        let _ = std::fs::remove_file(fx.path("w.txt"));
    }
    let (d, _) = fx
        .answered("run_command", json!({"command": "echo hi"}), Answer::Always)
        .await;
    record(&d);
    let (d, _) = fx
        .answered(
            "write_file",
            json!({"path": "w.txt", "content": "x"}),
            Answer::AllEdits,
        )
        .await;
    record(&d);
    // Cancelled while asking.
    let before = fx.cards().len();
    let call = fx.tool_call("run_command", &json!({"command": "echo bye"}).to_string());
    let ctx = fx.ctx();
    let (d, _) = tokio::join!(run_call(&ctx, &call), async {
        fx.next_card(before).await;
        fx.cancel.cancel();
    });
    record(&d);

    let events = fx.events();
    let mut started: HashMap<String, usize> = HashMap::new();
    let mut finished: HashMap<String, usize> = HashMap::new();
    for (i, e) in events.iter().enumerate() {
        match e {
            EngineEvent::ToolStarted { call_id, .. } => {
                assert!(
                    started.insert(call_id.clone(), i).is_none(),
                    "{call_id} started twice"
                );
            }
            EngineEvent::ToolFinished { call_id, .. } => {
                assert!(
                    finished.insert(call_id.clone(), i).is_none(),
                    "{call_id} finished twice"
                );
                assert!(
                    started.get(call_id).is_some_and(|s| *s < i),
                    "{call_id} finished first"
                );
            }
            _ => {}
        }
    }
    assert_eq!(started.len(), ids.len());
    assert_eq!(finished.len(), ids.len());
}

#[tokio::test]
async fn read_logs_are_kept_per_chat_and_forgotten() {
    let logs = ReadLogs::default();
    let a = logs.for_chat("a");
    assert!(Arc::ptr_eq(&a, &logs.for_chat("a")));
    assert!(!Arc::ptr_eq(&a, &logs.for_chat("b")));
    logs.forget("a");
    assert!(!Arc::ptr_eq(&a, &logs.for_chat("a")));
}

// ---------------------------------------------------------------------------
// Fix round 1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_folder_change_takes_back_all_edits() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    fx.store.set_edits_allowed(&fx.chat, true).unwrap();
    // Away to another folder and back: the grant is gone.
    let other = tempfile::tempdir().unwrap();
    fx.store
        .set_folder(&fx.chat, Some(&real_str(other.path())))
        .unwrap();
    fx.store
        .set_folder(&fx.chat, Some(fx.project.root.to_str().unwrap()))
        .unwrap();
    let (d, _) = fx
        .answered(
            "edit_file",
            edit_args("a.txt", "one", "uno"),
            Answer::Decline { note: None },
        )
        .await;
    declined(&d);
    assert_eq!(fx.read("a.txt"), "one\n");
}

const HIDDEN: &str = "That command contains hidden or control characters and cannot be run.";

#[tokio::test]
async fn commands_with_hidden_or_control_characters_are_refused_without_asking() {
    let fx = Fx::new();
    for c in [
        '\u{202E}', '\u{2066}', '\u{200B}', '\u{FEFF}', '\r', '\u{1B}', '\u{7F}',
    ] {
        for bg in [false, true] {
            let command = format!("echo safe{c} > made.txt");
            let d = fx
                .call("run_command", json!({"command": command, "background": bg}))
                .await;
            assert_eq!(d.outcome.status, ToolStatus::Error, "{c:?}");
            assert_eq!(d.outcome.text, HIDDEN, "{c:?}");
        }
    }
    assert!(!fx.path("made.txt").exists());
    assert!(fx.background.list(&fx.chat).is_empty());
}

#[tokio::test]
async fn a_multi_line_command_with_tabs_still_reaches_the_card() {
    let fx = Fx::new();
    let command = "echo one\n\techo two".to_string();
    let (d, card) = fx
        .answered(
            "run_command",
            json!({"command": command}),
            Answer::Decline { note: None },
        )
        .await;
    declined(&d);
    let Approval::Command { command: shown, .. } = card else {
        panic!();
    };
    assert_eq!(shown, command);
}

#[tokio::test]
async fn a_stop_between_approval_and_start_starts_nothing() {
    let fx = Fx::new();
    let command = pick(
        &fx.shell,
        "echo x > made.txt; exec sleep 30",
        "Set-Content -Path made.txt -Value x; Start-Sleep 60",
    );
    // The stop lands the moment the answer has been taken.
    let cancel = fx.cancel.clone();
    fx.on_event(move |e| {
        if matches!(e, EngineEvent::ApprovalCleared { .. }) {
            cancel.cancel();
        }
    });
    let (d, _) = fx
        .answered(
            "run_command",
            json!({"command": command, "background": true}),
            Answer::Once,
        )
        .await;
    assert_eq!(d.outcome.status, ToolStatus::NotRun, "{}", d.outcome.text);
    assert_eq!(d.outcome.text, "Not run: the task was stopped.");
    assert!(
        d.outcome.summary.starts_with("Not run: run "),
        "{}",
        d.outcome.summary
    );
    tokio::time::sleep(Duration::from_millis(long(1) * 500)).await;
    assert!(fx.background.list(&fx.chat).is_empty());
    assert!(!fx.path("made.txt").exists());
}

#[tokio::test]
async fn a_cancelled_turn_reaching_a_saved_rule_runs_nothing() {
    // No card: the stop must be seen after the rule check, before a spawn.
    let fx = Fx::new();
    fx.store.add_rule(&fx.folder(), "echo hi").unwrap();
    fx.cancel.cancel();
    for bg in [true, false] {
        let d = fx
            .call(
                "run_command",
                json!({"command": "echo hi", "background": bg}),
            )
            .await;
        assert_eq!(d.outcome.status, ToolStatus::NotRun, "{}", d.outcome.text);
        assert_eq!(d.outcome.text, "Not run: the task was stopped.");
        assert_eq!(d.outcome.detail, None);
    }
    assert!(fx.background.list(&fx.chat).is_empty());
    assert!(
        !fx.events()
            .iter()
            .any(|e| matches!(e, EngineEvent::ToolOutput { .. }))
    );
}

#[tokio::test]
async fn a_stopped_secret_read_or_command_is_not_run_not_declined() {
    let fx = Fx::new();
    fx.write(".env", "K=1\n");
    for (name, args, what) in [
        ("read_file", json!({"path": ".env"}), "Not run: read .env"),
        (
            "run_command",
            json!({"command": "echo hi"}),
            "Not run: run echo hi",
        ),
    ] {
        let cancel = CancellationToken::new();
        let before = fx.cards().len();
        let call = fx.tool_call(name, &args.to_string());
        let mut ctx = fx.ctx();
        ctx.cancel = &cancel;
        let (d, _) = tokio::join!(run_call(&ctx, &call), async {
            fx.next_card(before).await;
            cancel.cancel();
        });
        assert_eq!(d.outcome.status, ToolStatus::NotRun);
        assert_eq!(d.outcome.text, "Not run: the task was stopped.");
        assert_eq!(d.outcome.summary, what);
    }
}

#[tokio::test]
async fn a_call_dropped_while_its_card_waits_ends_as_not_run_and_does_nothing() {
    let fx = Fx::new();
    fx.write("a.txt", "one\n");
    fx.model_reads("a.txt").await;
    let command = pick(
        &fx.shell,
        "echo x > made.txt",
        "Set-Content -Path made.txt -Value x",
    );
    for (name, args) in [
        ("edit_file", edit_args("a.txt", "one", "uno")),
        (
            "run_command",
            json!({"command": command, "background": true}),
        ),
    ] {
        let start = fx.events().len();
        let call = fx.tool_call(name, &args.to_string());
        let ctx = fx.ctx();
        {
            let fut = run_call(&ctx, &call);
            tokio::pin!(fut);
            // Poll until the card shows, then drop the call.
            let shown = tokio::select! {
                _ = &mut fut => false,
                _ = fx.next_card(fx.cards().len()) => true,
            };
            assert!(shown, "{name}");
        }
        let ev = fx.events()[start..].to_vec();
        assert_eq!(ev.len(), 4, "{name}: {ev:?}");
        assert!(matches!(&ev[0], EngineEvent::ToolStarted { call_id, .. } if *call_id == call.id));
        let EngineEvent::ApprovalNeeded { approval } = &ev[1] else {
            panic!("{ev:?}");
        };
        assert_eq!(
            ev[2],
            EngineEvent::ApprovalCleared {
                id: approval.id().to_string()
            }
        );
        assert!(matches!(
            &ev[3],
            EngineEvent::ToolFinished { call_id, status: ToolStatus::NotRun, .. } if *call_id == call.id
        ));
        assert!(fx.approvals.waiting(&fx.chat).is_none());
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fx.read("a.txt"), "one\n");
    assert!(!fx.path("made.txt").exists());
    assert!(fx.background.list(&fx.chat).is_empty());
}
