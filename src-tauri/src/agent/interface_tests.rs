//! The commands the interface calls (`commands/agent.rs`), against the task
//! loop of `task_tests.rs` and its fixture: a scripted engine, the real app
//! state with an in-memory store, a real temp project and the real
//! dispatcher. A child module of `task_tests`, so it uses its helpers.

use super::*;
use crate::agent::Approval;
use crate::agent::dispatch::folder_text;
use crate::commands::agent::{self as commands, ProjectRules};
use serde_json::Value;

const NO_CHAT: &str = "That chat no longer exists.";
const NOT_WAITING: &str = "That request is no longer waiting.";
const NOT_RETRIABLE: &str =
    "A task that changed things cannot be retried. Send a message to continue.";
const STORED_KEY: &str = "sk-or-v1-test";

fn echo() -> ToolCall {
    call("c1", "run_command", json!({"command": "echo hi"}))
}

/// The command card shown last.
fn last_command_card(f: &Fx) -> Approval {
    f.events()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            EngineEvent::ApprovalNeeded { approval } => Some(approval),
            _ => None,
        })
        .expect("a card was shown")
}

fn write_file(id: &str) -> Step {
    calls(
        "",
        vec![call(
            id,
            "write_file",
            json!({"path": "b.txt", "content": format!("new {id}")}),
        )],
    )
}

// ---------------------------------------------------------------------------
// answer_approval and waiting_approval
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answer_approval_reaches_a_waiting_task_and_the_task_continues() {
    let f = fx(vec![write_file("c1"), say("written")]);
    let answer = async {
        f.wait_for(card_waits).await;
        let waiting = commands::waiting_approval(&f.state, &f.chat)
            .unwrap()
            .expect("the card waits");
        assert!(matches!(waiting, Approval::Edit { .. }));
        commands::answer_approval(&f.state, &f.chat, waiting.id(), Answer::Once).unwrap();
        assert_eq!(commands::waiting_approval(&f.state, &f.chat).unwrap(), None);
    };
    let (r, ()) = tokio::join!(f.send("write"), answer);
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert_eq!(std::fs::read_to_string(f.path("b.txt")).unwrap(), "new c1");
    assert_eq!(
        f.shape(),
        ["user", "assistant[write_file]", "tool:ok", "assistant"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answer_approval_refuses_an_unknown_chat_a_stale_id_and_an_answer_the_card_does_not_offer()
{
    let f = fx(vec![write_file("c1"), say("written")]);
    let during = async {
        f.wait_for(card_waits).await;
        let id = commands::waiting_approval(&f.state, &f.chat)
            .unwrap()
            .unwrap()
            .id()
            .to_string();
        let unknown = commands::answer_approval(&f.state, "no-such-chat", &id, Answer::Once);
        assert_eq!(unknown.unwrap_err().message, NO_CHAT);
        let stale = commands::answer_approval(&f.state, &f.chat, "not-the-card", Answer::Once);
        assert_eq!(stale.unwrap_err().message, NOT_WAITING);
        // A "Keep going?" answer to an edit card.
        let wrong = commands::answer_approval(&f.state, &f.chat, &id, Answer::KeepGoing);
        assert_eq!(wrong.unwrap_err().message, NOT_WAITING);
        // Nothing changed: the card still waits and the file is not written.
        assert_eq!(
            commands::waiting_approval(&f.state, &f.chat)
                .unwrap()
                .map(|a| a.id().to_string()),
            Some(id.clone())
        );
        assert!(!f.path("b.txt").exists());
        commands::answer_approval(&f.state, &f.chat, &id, Answer::Once).unwrap();
        // The same card answered again is stale.
        let again = commands::answer_approval(&f.state, &f.chat, &id, Answer::Once);
        assert_eq!(again.unwrap_err().message, NOT_WAITING);
    };
    let (r, ()) = tokio::join!(f.send("write"), during);
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
}

#[test]
fn waiting_approval_refuses_an_unknown_chat_and_is_none_when_nothing_waits() {
    let f = fx(vec![]);
    assert_eq!(
        commands::waiting_approval(&f.state, "nope")
            .unwrap_err()
            .message,
        NO_CHAT
    );
    assert_eq!(commands::waiting_approval(&f.state, &f.chat).unwrap(), None);
}

// ---------------------------------------------------------------------------
// task_state, revoke_edits, background commands
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_state_reflects_the_plan_edits_allowed_the_card_and_background_commands() {
    let f = fx(vec![
        calls(
            "",
            vec![
                call(
                    "p1",
                    "update_plan",
                    json!({"items": [
                        {"text": "read", "state": "done"},
                        {"text": "write", "state": "in_progress"}
                    ]}),
                ),
                call(
                    "b1",
                    "run_command",
                    json!({"command": sleeper(), "background": true}),
                ),
            ],
        ),
        say("started"),
    ]);
    let fresh = commands::task_state(&f.state, &f.chat).unwrap();
    assert_eq!(
        fresh,
        commands::TaskState {
            plan: vec![],
            edits_allowed: false,
            background: vec![],
            project: true,
            waiting: None,
            running: false,
        }
    );
    let during = async {
        f.wait_for(card_waits).await;
        let st = commands::task_state(&f.state, &f.chat).unwrap();
        let card = st.waiting.expect("the waiting card is part of the state");
        assert!(matches!(
            card,
            Approval::Command {
                background: true,
                ..
            }
        ));
        assert_eq!(st.plan.len(), 2);
        assert!(st.running, "a turn is registered while it waits");
        f.answer_cards(vec![Answer::Once]).await;
    };
    let (r, ()) = tokio::join!(f.send("go"), during);
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    let st = commands::task_state(&f.state, &f.chat).unwrap();
    assert_eq!(st.plan[0].text, "read");
    assert_eq!(st.plan[1].text, "write");
    assert_eq!(st.waiting, None);
    assert!(!st.running, "no turn is registered once it ended");
    assert_eq!(st.background.len(), 1);
    assert!(st.background[0].running);
    assert_eq!(st.background[0].command, sleeper());

    // The kept output, then a stop; the command stays listed, ended.
    let id = st.background[0].id.clone();
    let kept = commands::background_output(&f.state, &f.chat, &id).unwrap();
    assert!(kept.is_empty() || kept.contains("started"), "{kept:?}");
    commands::stop_background(&f.state, &f.chat, &id)
        .await
        .unwrap();
    let after = commands::task_state(&f.state, &f.chat).unwrap();
    assert!(!after.background[0].running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_commands_are_refused_for_unknown_chats_unknown_ids_and_other_chats() {
    let f = fx(vec![
        calls(
            "",
            vec![call(
                "b1",
                "run_command",
                json!({"command": sleeper(), "background": true}),
            )],
        ),
        say("started"),
    ]);
    let (r, ()) = tokio::join!(f.send("go"), f.answer_cards(vec![Answer::Once]));
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    let id = commands::task_state(&f.state, &f.chat).unwrap().background[0]
        .id
        .clone();
    let other = f.state.store.create_chat(TOOLS_MODEL).unwrap().id;
    for chat in ["no-such-chat", other.as_str()] {
        let out = commands::background_output(&f.state, chat, &id).unwrap_err();
        let stop = commands::stop_background(&f.state, chat, &id)
            .await
            .unwrap_err();
        assert!(!out.message.is_empty() && !stop.message.is_empty());
    }
    assert_eq!(
        commands::background_output(&f.state, "no-such-chat", &id)
            .unwrap_err()
            .message,
        NO_CHAT
    );
    assert!(commands::background_output(&f.state, &f.chat, "nope").is_err());
    assert!(
        commands::stop_background(&f.state, &f.chat, "nope")
            .await
            .is_err()
    );
    // The other chat's attempts changed nothing.
    assert!(commands::task_state(&f.state, &f.chat).unwrap().background[0].running);
    assert!(
        commands::task_state(&f.state, &other)
            .unwrap()
            .background
            .is_empty()
    );
    commands::stop_background(&f.state, &f.chat, &id)
        .await
        .unwrap();
}

#[test]
fn task_state_and_revoke_edits_refuse_an_unknown_chat() {
    let f = fx(vec![]);
    assert_eq!(
        commands::task_state(&f.state, "nope").unwrap_err().message,
        NO_CHAT
    );
    assert_eq!(
        commands::revoke_edits(&f.state, "nope")
            .unwrap_err()
            .message,
        NO_CHAT
    );
}

#[test]
fn a_stored_plan_that_does_not_parse_is_an_empty_plan_not_an_error() {
    let f = fx(vec![]);
    for bad in ["not json", "{\"a\":1}", "[{\"text\":1}]", ""] {
        f.state.store.set_plan(&f.chat, bad).unwrap();
        let st = commands::task_state(&f.state, &f.chat).unwrap();
        assert!(st.plan.is_empty(), "{bad:?}");
    }
}

#[tokio::test]
async fn project_is_true_only_while_the_chats_folder_exists() {
    let f = fx(vec![]);
    let model = TOOLS_MODEL;
    assert!(commands::task_state(&f.state, &f.chat).unwrap().project);
    assert!(
        chat_state::chat_state(&f.state, &f.chat, model)
            .unwrap()
            .project
    );
    // No folder.
    let bare = f.state.store.create_chat(model).unwrap().id;
    assert!(!commands::task_state(&f.state, &bare).unwrap().project);
    assert!(
        !chat_state::chat_state(&f.state, &bare, model)
            .unwrap()
            .project
    );
    // A folder that is gone.
    let gone = tempfile::tempdir().unwrap();
    let path = real(gone.path());
    f.state.store.set_folder(&bare, Some(&path)).unwrap();
    assert!(commands::task_state(&f.state, &bare).unwrap().project);
    drop(gone);
    assert!(!commands::task_state(&f.state, &bare).unwrap().project);
    assert!(
        !chat_state::chat_state(&f.state, &bare, model)
            .unwrap()
            .project
    );
    // The folder itself is still reported.
    assert_eq!(
        chat_state::chat_state(&f.state, &bare, model)
            .unwrap()
            .folder,
        Some(path)
    );
}

// ---------------------------------------------------------------------------
// Saved command rules
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_project_rule_makes_the_next_matching_command_ask_again() {
    let f = fx(vec![
        calls("", vec![echo()]),
        calls(
            "",
            vec![call("c2", "run_command", json!({"command": "echo hi"}))],
        ),
        calls(
            "",
            vec![call("c3", "run_command", json!({"command": "echo hi"}))],
        ),
        say("done"),
    ]);
    // The first command asks; "Always" saves its rule.
    let first = async {
        f.wait_for(card_waits).await;
        f.answer_cards(vec![Answer::Always]).await;
    };
    let (r, ()) = tokio::join!(f.send("go"), first);
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    // Only the first command asked: the saved rule let the next two through.
    assert_eq!(f.cards(), 1);
    let Approval::Command {
        rule: Some(rule), ..
    } = last_command_card(&f)
    else {
        panic!("a command card with a rule");
    };
    let root = folder_text(&std::fs::canonicalize(f.project.path()).unwrap());
    let listed = commands::project_rules(&f.state).unwrap();
    assert_eq!(
        listed,
        vec![ProjectRules {
            folder: root.clone(),
            rules: vec![rule.clone()]
        }]
    );

    commands::remove_project_rule(&f.state, &root, &rule).unwrap();
    assert!(commands::project_rules(&f.state).unwrap().is_empty());
    // The next matching command asks again.
    f.engine.push(vec![
        calls(
            "",
            vec![call("c4", "run_command", json!({"command": "echo hi"}))],
        ),
        say("again"),
    ]);
    let second = f.answer_cards(vec![Answer::Once]);
    let (r, ()) = tokio::join!(f.send("again"), second);
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert_eq!(f.cards(), 2);
}

#[test]
fn rules_are_listed_by_the_folder_text_the_dispatcher_keys_them_by() {
    let f = fx(vec![]);
    // A chat's folder may be stored with a Windows `\\?\` prefix; the rules
    // are keyed without it, and removal takes the key as listed.
    let stored = r"\\?\C:\work\proj";
    let key = folder_text(std::path::Path::new(stored));
    assert_eq!(key, r"C:\work\proj");
    f.state.store.add_rule(&key, "git status").unwrap();
    f.state.store.add_rule(&key, "cargo test").unwrap();
    f.state.store.add_rule("/other", "ls").unwrap();
    // A chat whose stored folder carries the prefix is no part of the listing.
    f.state.store.set_folder(&f.chat, Some(stored)).unwrap();
    assert_eq!(
        commands::project_rules(&f.state).unwrap(),
        vec![
            ProjectRules {
                folder: "/other".into(),
                rules: vec!["ls".into()]
            },
            ProjectRules {
                folder: key.clone(),
                rules: vec!["cargo test".into(), "git status".into()]
            },
        ]
    );
    // The prefixed string has no rules: refused, nothing removed.
    let e = commands::remove_project_rule(&f.state, stored, "git status").unwrap_err();
    assert_eq!(
        e.message,
        "There are no saved command rules for that folder."
    );
    assert_eq!(f.state.store.rules(&key).unwrap().len(), 2);
    // The key as listed works.
    commands::remove_project_rule(&f.state, &key, "git status").unwrap();
    assert_eq!(f.state.store.rules(&key).unwrap(), ["cargo test"]);
}

#[test]
fn remove_project_rule_refuses_a_folder_or_a_rule_that_is_not_saved_and_changes_nothing() {
    let f = fx(vec![]);
    f.state.store.add_rule("/p", "ls").unwrap();
    let no_folder = commands::remove_project_rule(&f.state, "/q", "ls").unwrap_err();
    assert_eq!(
        no_folder.message,
        "There are no saved command rules for that folder."
    );
    let no_rule = commands::remove_project_rule(&f.state, "/p", "rm").unwrap_err();
    assert_eq!(
        no_rule.message,
        "That command rule is not saved for that folder."
    );
    // Folder text is only compared: a path, a relative path or an empty
    // string opens nothing and matches nothing.
    for odd in ["", ".", "..", "/p/", "/P", "/p/../p", "~"] {
        assert!(
            commands::remove_project_rule(&f.state, odd, "ls").is_err(),
            "{odd:?}"
        );
    }
    assert_eq!(f.state.store.rules("/p").unwrap(), ["ls"]);
}

// ---------------------------------------------------------------------------
// The spending limit
// ---------------------------------------------------------------------------

#[test]
fn spend_limit_reports_the_limit_and_todays_spend_and_set_refuses_below_ten_cents() {
    let f = fx(vec![]);
    assert_eq!(
        commands::spend_limit(&f.state).unwrap(),
        commands::SpendLimit {
            limit_usd: Some(1.0),
            spent_today_usd: 0.0
        }
    );
    spend(&f, 0.25);
    assert_eq!(
        commands::spend_limit(&f.state).unwrap().spent_today_usd,
        0.25
    );

    commands::set_spend_limit(&f.state, Some(2.5)).unwrap();
    assert_eq!(
        commands::spend_limit(&f.state).unwrap().limit_usd,
        Some(2.5)
    );
    commands::set_spend_limit(&f.state, Some(0.10)).unwrap();
    assert_eq!(
        commands::spend_limit(&f.state).unwrap().limit_usd,
        Some(0.10)
    );
    for bad in [0.09, 0.0, -1.0, f64::NAN, f64::INFINITY] {
        let e = commands::set_spend_limit(&f.state, Some(bad)).unwrap_err();
        assert_eq!(e.message, "The limit must be at least $0.10.", "{bad}");
        assert_eq!(
            commands::spend_limit(&f.state).unwrap().limit_usd,
            Some(0.10)
        );
    }
    commands::set_spend_limit(&f.state, None).unwrap();
    assert_eq!(commands::spend_limit(&f.state).unwrap().limit_usd, None);
}

#[test]
fn spend_limit_cannot_report_a_spend_it_cannot_read() {
    let f = fx(vec![]);
    f.state
        .store
        .exec_for_test("DROP TABLE daily_usage;")
        .unwrap();
    assert!(commands::spend_limit(&f.state).is_err());
}

// ---------------------------------------------------------------------------
// Retry after a task that stored tool rows, and rows_after
// ---------------------------------------------------------------------------

async fn retry(f: &Fx, regenerate: bool) -> Result<TurnResult, UiError> {
    let emit = f.emitter();
    turn::retry_turn(&f.state, &f.chat, TOOLS_MODEL, regenerate, &emit).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_after_a_task_with_tool_rows_is_refused_with_the_exact_message() {
    let f = fx(vec![list("c1"), say("all done")]);
    assert_eq!(f.send("look").await.status, TurnStatus::Done);
    let before = f.shape();
    for regenerate in [true, false] {
        let e = retry(&f, regenerate).await.unwrap_err();
        assert_eq!(e.message, NOT_RETRIABLE, "{regenerate}");
        assert_eq!(f.shape(), before, "nothing changed");
    }
    assert_eq!(f.requests().len(), 2);
    // A message continues it.
    f.engine.push(vec![say("more")]);
    assert_eq!(f.send("continue").await.status, TurnStatus::Done);
    assert_rows_paired(&f);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_of_an_interrupted_project_turn_with_tool_rows_is_refused_too() {
    let f = fx(vec![list("c1"), network_error("part way")]);
    let r = f.send("look").await;
    assert_eq!(r.status, TurnStatus::Interrupted);
    let e = retry(&f, false).await.unwrap_err();
    assert_eq!(e.message, NOT_RETRIABLE);
    assert_eq!(f.requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_after_a_plain_reply_still_works_and_a_new_task_clears_the_old_tool_rows() {
    let f = fx(vec![say("first answer"), say("second answer")]);
    f.send("hello").await;
    let r = retry(&f, true).await.unwrap();
    assert_eq!(r.status, TurnStatus::Done);
    assert_eq!(f.shape(), ["user", "assistant"]);
    assert_eq!(f.rows()[1].text, "second answer");

    // Tool rows of an earlier task do not block a retry of a later plain one.
    f.engine.push(vec![
        list("c9"),
        say("looked"),
        say("plain"),
        say("plain again"),
    ]);
    f.send("look").await;
    f.send("thanks").await;
    assert_eq!(retry(&f, true).await.unwrap().status, TurnStatus::Done);
    assert_eq!(f.rows().last().unwrap().text, "plain again");
}

#[tokio::test]
async fn a_plain_chat_retry_is_unchanged() {
    let f = fx(vec![say("one"), say("two")]);
    let chat = f.state.store.create_chat(PLAIN_MODEL).unwrap().id;
    let emit = f.emitter();
    turn::send_message(&f.state, &chat, "hi", vec![], PLAIN_MODEL, &emit)
        .await
        .unwrap();
    let r = turn::retry_turn(&f.state, &chat, PLAIN_MODEL, true, &emit)
        .await
        .unwrap();
    assert_eq!(r.status, TurnStatus::Done);
    let rows = chats::get_messages(&f.state, &chat).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].text, "two");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rows_after_counts_tool_rows_and_notes() {
    let f = fx(vec![list("c1"), say("done")]);
    f.send("look").await;
    f.state.store.append_note(&f.chat, "a note").unwrap();
    let rows = f.rows();
    assert!(rows.iter().any(|m| m.kind == "tool"));
    let first = rows[0].id.clone();
    let after = chats::rows_after(&f.state, &f.chat, &first).unwrap();
    assert_eq!(after as usize, rows.len() - 1);
    let taken = chats::take_user_message(&f.state, &f.chat, &first).unwrap();
    assert_eq!(taken.removed, after);
    assert!(f.rows().is_empty());
}

// ---------------------------------------------------------------------------
// The key never reaches a command result or an event
// ---------------------------------------------------------------------------

/// Every `EngineEvent` variant, one value each.
fn every_event() -> Vec<EngineEvent> {
    use crate::agent::ToolStatus;
    use crate::agent::plan::{PlanItem, PlanState};
    vec![
        EngineEvent::TextDelta { text: "t".into() },
        EngineEvent::Usage {
            requests: 1,
            cost_usd: 0.5,
        },
        EngineEvent::Done,
        EngineEvent::Interrupted {
            reason: "stopped".into(),
        },
        EngineEvent::Failed {
            error: UiError::other("x"),
        },
        EngineEvent::ToolStarted {
            call_id: "c".into(),
            name: "n".into(),
            summary: "s".into(),
        },
        EngineEvent::ToolOutput {
            call_id: "c".into(),
            text: "o".into(),
        },
        EngineEvent::ToolFinished {
            call_id: "c".into(),
            status: ToolStatus::Ok,
            summary: "s".into(),
            detail: None,
        },
        EngineEvent::ApprovalNeeded {
            approval: Approval::Secret {
                id: "a".into(),
                path: ".env".into(),
            },
        },
        EngineEvent::ApprovalCleared { id: "a".into() },
        EngineEvent::PlanUpdated {
            items: vec![PlanItem {
                text: "p".into(),
                state: PlanState::Done,
            }],
        },
        EngineEvent::Waiting { seconds: 1 },
        EngineEvent::Paused {
            reason: "daily_limit".into(),
        },
        EngineEvent::BackgroundChanged,
        EngineEvent::MessageStored,
    ]
}

fn event_tag(e: &EngineEvent) -> String {
    serde_json::to_value(e).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_new_command_result_or_event_carries_the_stored_key() {
    // A key-looking string in the project's file and in command output may
    // appear in tool output (it is file content). The app's own stored key
    // must appear nowhere.
    const FILE_KEY: &str = "sk-or-v1-FILEKEY123";
    let f = fx(vec![
        Step::Stream(
            vec!["Let me ".into(), "look".into()],
            StepOutcome {
                text: "Let me look".into(),
                tool_calls: vec![
                    call(
                        "p1",
                        "update_plan",
                        json!({"items": [{"text": "look", "state": "in_progress"}]}),
                    ),
                    call("r1", "read_file", json!({"path": "notes.txt"})),
                    call(
                        "e1",
                        "run_command",
                        json!({"command": format!("echo {FILE_KEY}")}),
                    ),
                    call(
                        "w1",
                        "write_file",
                        json!({"path": "out.txt", "content": FILE_KEY}),
                    ),
                    call(
                        "b1",
                        "run_command",
                        json!({"command": sleeper(), "background": true}),
                    ),
                ],
                cost_usd: Some(0.01),
                ..StepOutcome::default()
            },
        ),
        rate_limited(Some(1)),
        say("done"),
    ]);
    std::fs::write(
        f.path("notes.txt"),
        format!("OPENROUTER_API_KEY={FILE_KEY}\n"),
    )
    .unwrap();

    let mut outputs: Vec<String> = Vec::new();
    let mut record = |v: Value| outputs.push(v.to_string());

    let (r, ()) = tokio::join!(
        f.send("go"),
        f.answer_cards(vec![Answer::Once, Answer::Once, Answer::Once])
    );
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");

    // The new commands, after the task.
    record(to(&commands::task_state(&f.state, &f.chat)));
    record(to(&commands::waiting_approval(&f.state, &f.chat)));
    record(to(&commands::project_rules(&f.state)));
    record(to(&commands::spend_limit(&f.state)));
    record(to(&commands::set_spend_limit(&f.state, Some(3.0))));
    record(to(&commands::set_spend_limit(&f.state, Some(0.01))));
    record(to(&commands::revoke_edits(&f.state, &f.chat)));
    record(to(&commands::answer_approval(
        &f.state,
        &f.chat,
        "x",
        Answer::Once,
    )));
    record(to(&commands::remove_project_rule(&f.state, "nope", "nope")));
    record(to(&chat_state::chat_state(&f.state, &f.chat, TOOLS_MODEL)));
    let bg = commands::task_state(&f.state, &f.chat).unwrap().background;
    assert_eq!(bg.len(), 1);
    record(to(&commands::background_output(
        &f.state, &f.chat, &bg[0].id,
    )));
    record(to(&commands::background_output(&f.state, &f.chat, "nope")));
    record(to(
        &commands::stop_background(&f.state, &f.chat, &bg[0].id).await
    ));
    record(to(&commands::task_state(&f.state, &f.chat)));
    // A paused task, a failed one and an interrupted one.
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(0).unwrap());
    assert_eq!(f.send("again").await.status, TurnStatus::Paused);
    f.outside
        .fallback
        .lock()
        .unwrap()
        .replace(free_left(5).unwrap());
    f.engine
        .push(vec![network_error(""), network_error("part")]);
    assert_eq!(f.send("fail").await.status, TurnStatus::Failed);
    assert_eq!(f.send("cut").await.status, TurnStatus::Interrupted);
    record(to(&commands::task_state(&f.state, &f.chat)));
    record(to(&f.rows()));

    // Every kind of event the run produced, plus one of each variant.
    let events = f.events();
    let seen: std::collections::BTreeSet<String> = events.iter().map(event_tag).collect();
    for tag in [
        "text_delta",
        "usage",
        "done",
        "interrupted",
        "failed",
        "tool_started",
        "tool_output",
        "tool_finished",
        "approval_needed",
        "approval_cleared",
        "plan_updated",
        "waiting",
        "paused",
        "message_stored",
    ] {
        assert!(seen.contains(tag), "the run produced no {tag}: {seen:?}");
    }
    record(to(&events));
    record(to(&every_event()));

    assert!(
        outputs.iter().any(|o| o.contains(FILE_KEY)),
        "the file's own string is tool output and may appear"
    );
    for out in &outputs {
        assert!(!out.contains(STORED_KEY), "the key leaked in {out}");
    }
}

fn to<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap()
}

// ---------------------------------------------------------------------------
// JSON samples of every serialised shape, for the TypeScript tests
// ---------------------------------------------------------------------------

mod fixtures {
    use super::*;
    use crate::agent::background::BgInfo;
    use crate::agent::plan::{PlanItem, PlanState};
    use crate::commands::chat_state::{ActiveSkill, ChatState};
    use crate::store::{StoredAttachment, StoredMessage, StoredToolCall, ToolMeta};

    fn pretty<T: serde::Serialize>(samples: &T) -> String {
        // `\n` line endings and declared field order: the same bytes on
        // every system.
        let mut text = serde_json::to_string_pretty(samples).unwrap();
        text.push('\n');
        text
    }

    fn approvals() -> Vec<Approval> {
        vec![
            Approval::Command {
                id: "a1".into(),
                command: "cargo test".into(),
                folder: "/work/proj".into(),
                rule: Some("cargo test".into()),
                outside: false,
                first_in_project: true,
                background: false,
            },
            Approval::Command {
                id: "a2".into(),
                command: "npm run dev".into(),
                folder: "/work/proj".into(),
                rule: None,
                outside: true,
                first_in_project: false,
                background: true,
            },
            Approval::Edit {
                id: "a3".into(),
                path: "src/main.rs".into(),
                created: false,
                removed: vec!["old line".into()],
                added: vec!["new line".into(), "another".into()],
                total_lines: 120,
                removed_total: 1,
                added_total: 2,
            },
            Approval::Secret {
                id: "a4".into(),
                path: ".env".into(),
            },
            Approval::KeepGoing {
                id: "a5".into(),
                requests: 25,
                free_left: None,
                spent_today_usd: Some(0.42),
            },
            Approval::KeepGoing {
                id: "a6".into(),
                requests: 25,
                free_left: Some(12),
                spent_today_usd: None,
            },
        ]
    }

    fn answers() -> Vec<Answer> {
        vec![
            Answer::Once,
            Answer::Always,
            Answer::AllEdits,
            Answer::Decline { note: None },
            Answer::Decline {
                note: Some("Not that file".into()),
            },
            Answer::Message {
                text: "Do this instead".into(),
            },
            Answer::KeepGoing,
            Answer::StopHere,
        ]
    }

    fn turn_results() -> Vec<TurnResult> {
        vec![
            TurnResult::done(),
            TurnResult::interrupted("stopped".into()),
            TurnResult::failed(UiError::other("It failed.")),
            TurnResult::paused("daily_limit"),
        ]
    }

    fn ui_errors() -> Vec<UiError> {
        vec![
            UiError::other("Something went wrong."),
            UiError::daily_limit("You have reached today's spending limit of $1.00."),
            UiError::from(crate::openrouter::error::ApiError::LowCredit),
        ]
    }

    fn bg() -> Vec<BgInfo> {
        vec![
            BgInfo {
                id: "bg1".into(),
                command: "npm run dev".into(),
                running: true,
                exit_code: None,
                started_at: 1_700_000_000,
            },
            BgInfo {
                id: "bg2".into(),
                command: "cargo build".into(),
                running: false,
                exit_code: Some(0),
                started_at: 1_700_000_100,
            },
        ]
    }

    fn task_states() -> Vec<commands::TaskState> {
        vec![
            commands::TaskState {
                plan: vec![
                    PlanItem {
                        text: "Read".into(),
                        state: PlanState::Done,
                    },
                    PlanItem {
                        text: "Edit".into(),
                        state: PlanState::InProgress,
                    },
                    PlanItem {
                        text: "Test".into(),
                        state: PlanState::Pending,
                    },
                ],
                edits_allowed: true,
                background: bg(),
                project: true,
                waiting: Some(approvals().remove(0)),
                running: true,
            },
            commands::TaskState {
                plan: vec![],
                edits_allowed: false,
                background: vec![],
                project: false,
                waiting: None,
                running: false,
            },
        ]
    }

    fn spend_limits() -> Vec<commands::SpendLimit> {
        vec![
            commands::SpendLimit {
                limit_usd: Some(1.0),
                spent_today_usd: 0.25,
            },
            commands::SpendLimit {
                limit_usd: None,
                spent_today_usd: 0.0,
            },
        ]
    }

    fn rules() -> Vec<ProjectRules> {
        vec![ProjectRules {
            folder: "/work/proj".into(),
            rules: vec!["cargo test".into(), "git status".into()],
        }]
    }

    fn messages() -> Vec<StoredMessage> {
        let row = |id: &str, role: &str, text: &str, kind: &str, seq: i64| StoredMessage {
            id: id.into(),
            role: role.into(),
            text: text.into(),
            attachments: vec![],
            interrupted: false,
            created_at: 1_700_000_000 + seq,
            kind: kind.into(),
            seq,
            tool_calls: vec![],
            tool_call_id: None,
            tool_meta: None,
        };
        let mut user = row("m1", "user", "Fix the bug", "message", 1);
        user.attachments = vec![StoredAttachment {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            data_base64: "aGk=".into(),
        }];
        let mut assistant = row("m2", "assistant", "Looking", "message", 2);
        assistant.tool_calls = vec![StoredToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{\"path\":\"a.txt\"}".into(),
        }];
        let mut tool = row("m3", "tool", "hello", "tool", 3);
        tool.tool_call_id = Some("c1".into());
        tool.tool_meta = Some(ToolMeta {
            name: "read_file".into(),
            status: "ok".into(),
            summary: "Read a.txt".into(),
            detail: Some("1 line".into()),
        });
        let mut declined = row("m4", "tool", "Declined", "tool", 4);
        declined.tool_call_id = Some("c2".into());
        declined.tool_meta = Some(ToolMeta {
            name: "run_command".into(),
            status: "declined".into(),
            summary: "Did not run ls".into(),
            detail: None,
        });
        let mut partial = row("m5", "assistant", "Cut", "message", 5);
        partial.interrupted = true;
        vec![
            user,
            assistant,
            tool,
            declined,
            row("m6", "system", "Paused", "note", 6),
            partial,
        ]
    }

    fn chat_states() -> Vec<ChatState> {
        vec![ChatState {
            active_skills: vec![ActiveSkill {
                id: "p:a".into(),
                name: "a".into(),
                too_long: false,
                acts_on_computer: true,
                unavailable: false,
            }],
            folder: Some("/work/proj".into()),
            project: true,
            compacted: false,
        }]
    }

    /// Every sample file: its name and its exact text.
    pub fn all() -> Vec<(&'static str, String)> {
        vec![
            ("answer.json", pretty(&answers())),
            ("approval.json", pretty(&approvals())),
            ("bg_info.json", pretty(&bg())),
            ("chat_state.json", pretty(&chat_states())),
            ("engine_event.json", pretty(&every_event())),
            ("project_rules.json", pretty(&rules())),
            ("spend_limit.json", pretty(&spend_limits())),
            ("stored_message.json", pretty(&messages())),
            ("task_state.json", pretty(&task_states())),
            ("turn_result.json", pretty(&turn_results())),
            ("ui_error.json", pretty(&ui_errors())),
        ]
    }

    fn dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ipc")
    }

    /// The files under `tests/fixtures/ipc` are what the Rust types serialise
    /// to now; the TypeScript tests check them against `src/lib/ipc.ts`. After
    /// a change to a type run this test with `UPDATE_IPC_FIXTURES=1`.
    #[test]
    fn the_ipc_fixtures_are_up_to_date() {
        let update = std::env::var_os("UPDATE_IPC_FIXTURES").is_some();
        if update {
            std::fs::create_dir_all(dir()).unwrap();
        }
        for (name, text) in all() {
            assert!(!text.contains('\r'));
            let path = dir().join(name);
            if update {
                std::fs::write(&path, &text).unwrap();
            }
            let on_disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("{name} is missing: run with UPDATE_IPC_FIXTURES=1"));
            assert_eq!(
                on_disk.replace("\r\n", "\n"),
                text,
                "{name} is out of date: run with UPDATE_IPC_FIXTURES=1"
            );
        }
        // No stale file is left behind by a removed type.
        let known: Vec<&str> = all().iter().map(|(n, _)| *n).collect();
        for entry in std::fs::read_dir(dir()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(known.contains(&name.as_str()), "{name} has no sample");
        }
    }
}

// ---------------------------------------------------------------------------
// Fix round 1
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_edits_is_allowed_through_a_card_and_revoked_by_the_command_so_the_next_edit_asks() {
    let f = fx(vec![
        write_file("c1"),
        say("one"),
        write_file("c2"),
        say("two"),
        write_file("c3"),
        say("three"),
    ]);
    // Nothing the web view can call turns it on.
    assert!(
        !commands::task_state(&f.state, &f.chat)
            .unwrap()
            .edits_allowed
    );
    let (r, ()) = tokio::join!(f.send("a"), f.answer_cards(vec![Answer::AllEdits]));
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert!(
        commands::task_state(&f.state, &f.chat)
            .unwrap()
            .edits_allowed
    );
    // With it on, the next edit asks nothing.
    assert_eq!(f.send("b").await.status, TurnStatus::Done);
    assert_eq!(f.cards(), 1);
    commands::revoke_edits(&f.state, &f.chat).unwrap();
    assert!(
        !commands::task_state(&f.state, &f.chat)
            .unwrap()
            .edits_allowed
    );
    // Revoked, the next edit asks again.
    let (r, ()) = tokio::join!(f.send("c"), f.answer_cards(vec![Answer::Once]));
    assert_eq!(r.status, TurnStatus::Done, "{r:?}");
    assert_eq!(f.cards(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compaction_on_a_paid_model_records_an_estimate_when_no_cost_is_reported() {
    for (model, reported, expect) in [
        (PAID_PLAIN, None, true),
        (PAID_PLAIN, Some(0.5), false),
        (PLAIN_MODEL, None, false),
    ] {
        let f = fx(vec![Step::Reply(StepOutcome {
            text: "SUMMARY!".into(),
            cost_usd: reported,
            prompt_tokens: Some(1000),
            ..StepOutcome::default()
        })]);
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
        let r = compact::compact_chat(&f.state, &f.chat, model, None)
            .await
            .unwrap();
        assert_eq!(r.status, TurnStatus::Done, "{r:?}");
        let cost = f.state.store.chat_usage(&f.chat).unwrap().1;
        match (reported, expect) {
            // 1,000 tokens at $1/Mtok and 8 chars (2 tokens) at $2/Mtok.
            (None, true) => assert!((cost - 0.001_004).abs() < 1e-12, "{cost}"),
            (Some(c), _) => assert!((cost - c).abs() < 1e-12, "{cost}"),
            (None, false) => assert_eq!(cost, 0.0),
        }
    }
}
