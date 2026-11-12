//! The guards on their own: the spending limit, going in circles and
//! trimming. The loop's use of them is tested in `task_tests.rs`.

use super::*;
use crate::agent::{ToolCall, ToolOutcome, ToolStatus};
use crate::openrouter::models::ModelInfo;
use crate::openrouter::stream::{Part, Role, WireMessage};
use crate::store::Store;

fn model(free: bool) -> ModelInfo {
    ModelInfo {
        id: "m/x".into(),
        name: "X".into(),
        prompt_usd_per_mtok: if free { 0.0 } else { 1.0 },
        completion_usd_per_mtok: if free { 0.0 } else { 2.0 },
        is_free: free,
        supports_tools: true,
        accepts_images: true,
        context_length: 100_000,
    }
}

fn spend(store: &Store, usd: f64) {
    let chat = store.create_chat("m/x").unwrap().id;
    store.record_usage(&chat, 1, usd).unwrap();
}

// ---------------------------------------------------------------------------
// The spending limit
// ---------------------------------------------------------------------------

#[test]
fn the_limit_defaults_to_one_dollar_and_none_means_no_limit() {
    let store = Store::in_memory().unwrap();
    assert_eq!(spend_limit(&store), Some(1.00));
    store.set_setting(LIMIT_SETTING, "none").unwrap();
    assert_eq!(spend_limit(&store), None);
    store.set_setting(LIMIT_SETTING, "2.5").unwrap();
    assert_eq!(spend_limit(&store), Some(2.5));
    // A value the app never writes is not trusted: the default applies.
    for bad in ["", "abc", "0.01", "-3", "NaN", "inf"] {
        store.set_setting(LIMIT_SETTING, bad).unwrap();
        assert_eq!(spend_limit(&store), Some(1.00), "{bad:?}");
    }
}

#[test]
fn setting_the_limit_refuses_below_ten_cents_and_accepts_ten_cents_or_none() {
    let store = Store::in_memory().unwrap();
    for bad in [
        0.05,
        0.0999,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let e = set_spend_limit(&store, Some(bad)).unwrap_err();
        assert_eq!(e.message, "The limit must be at least $0.10.", "{bad}");
        assert_eq!(spend_limit(&store), Some(1.00), "{bad}");
    }
    set_spend_limit(&store, Some(0.10)).unwrap();
    assert_eq!(spend_limit(&store), Some(0.10));
    set_spend_limit(&store, Some(3.25)).unwrap();
    assert_eq!(spend_limit(&store), Some(3.25));
    set_spend_limit(&store, None).unwrap();
    assert_eq!(spend_limit(&store), None);
}

#[test]
fn over_limit_holds_for_a_paid_model_at_the_limit_with_the_exact_message() {
    let store = Store::in_memory().unwrap();
    let paid = model(false);
    spend(&store, 0.99);
    assert_eq!(over_limit(&store, Some(&paid)), None);
    spend(&store, 0.01);
    let e = over_limit(&store, Some(&paid)).expect("at the limit");
    assert_eq!(
        e.message,
        "You have reached today's spending limit of $1.00. You can change it in Settings."
    );
    assert_eq!(e.kind, "daily_limit");
    // A model missing from the model list is treated as paid.
    assert!(over_limit(&store, None).is_some());
    // A free model never.
    assert_eq!(over_limit(&store, Some(&model(true))), None);
    // No limit never.
    set_spend_limit(&store, None).unwrap();
    assert_eq!(over_limit(&store, Some(&paid)), None);
    // Two decimals in the message.
    set_spend_limit(&store, Some(0.5)).unwrap();
    assert!(
        over_limit(&store, Some(&paid))
            .unwrap()
            .message
            .contains("limit of $0.50.")
    );
}

#[test]
fn spend_within_a_millionth_of_a_dollar_of_the_limit_counts_as_reached() {
    let store = Store::in_memory().unwrap();
    spend(&store, 0.9999995);
    assert!(over_limit(&store, Some(&model(false))).is_some());
}

#[test]
fn today_is_the_utc_day_the_store_records_usage_under() {
    let store = Store::in_memory().unwrap();
    spend(&store, 0.25);
    let (requests, cost) = store.day_usage(&today()).unwrap();
    assert_eq!(requests, 1);
    assert!((cost - 0.25).abs() < 1e-9);
}

#[test]
fn the_pause_notes_are_exact() {
    let store = Store::in_memory().unwrap();
    assert_eq!(
        pause_note(DAILY_LIMIT, &store).as_deref(),
        Some("Paused: today's spending limit of $1.00 is reached. You can change it in Settings.")
    );
    assert_eq!(
        pause_note(NO_FREE_REQUESTS, &store).as_deref(),
        Some("Paused: no free requests are left today. They reset at midnight UTC.")
    );
    assert_eq!(
        pause_note(RATE_LIMITED, &store).as_deref(),
        Some("Paused: the model is limiting requests. Try again in a minute.")
    );
    assert_eq!(
        pause_note(GOING_IN_CIRCLES, &store).as_deref(),
        Some("This model is struggling with this task. A stronger model usually does better.")
    );
    assert_eq!(
        pause_note(CHAT_FULL, &store).as_deref(),
        Some("This chat is too full to continue. Use /compact to shorten it, then say continue.")
    );
    assert_eq!(pause_note(KEEP_GOING_DECLINED, &store), None);
}

// ---------------------------------------------------------------------------
// Going in circles
// ---------------------------------------------------------------------------

fn call(name: &str, args: &str) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: args.into(),
    }
}

fn outcome(status: ToolStatus, text: &str) -> ToolOutcome {
    ToolOutcome {
        status,
        text: text.into(),
        summary: "s".into(),
        detail: None,
    }
}

fn error(text: &str) -> ToolOutcome {
    outcome(ToolStatus::Error, text)
}

#[test]
fn three_identical_failing_calls_in_a_row_are_circles() {
    let mut c = Circles::default();
    let read = call("read_file", r#"{"path": "x.txt"}"#);
    assert!(!c.note(&read, &error("not found")));
    assert!(!c.note(&read, &error("not found")));
    assert!(c.note(&read, &error("not found")));
}

#[test]
fn the_same_arguments_written_differently_are_the_same_call() {
    let mut c = Circles::default();
    assert!(!c.note(&call("read_file", r#"{"path":"x.txt"}"#), &error("e")));
    assert!(!c.note(&call("read_file", r#"{ "path" : "x.txt" }"#), &error("e")));
    assert!(c.note(&call("read_file", r#"{"path": "x.txt"}"#), &error("e")));
}

#[test]
fn a_success_or_another_call_in_between_resets_the_count() {
    let mut c = Circles::default();
    let read = call("read_file", r#"{"path": "x.txt"}"#);
    assert!(!c.note(&read, &error("e")));
    assert!(!c.note(&read, &error("e")));
    assert!(!c.note(&call("list_dir", "{}"), &outcome(ToolStatus::Ok, "a")));
    assert!(!c.note(&read, &error("e")));
    assert!(!c.note(&read, &error("e")));
    assert!(!c.note(&call("read_file", r#"{"path": "y.txt"}"#), &error("e")));
    assert!(!c.note(&read, &error("e")));
}

#[test]
fn declined_and_not_run_calls_are_never_failures() {
    let mut c = Circles::default();
    let write = call("write_file", r#"{"path": "a", "content": "b"}"#);
    for _ in 0..5 {
        assert!(!c.note(&write, &outcome(ToolStatus::Declined, "no")));
        assert!(!c.note(&write, &outcome(ToolStatus::NotRun, "no")));
    }
}

#[test]
fn a_failing_command_counts_only_when_its_output_repeats_too() {
    let mut c = Circles::default();
    let test = call("run_command", r#"{"command": "cargo test"}"#);
    assert!(!c.note(&test, &error("exit code 101\n3 failed")));
    assert!(!c.note(&test, &error("exit code 101\n2 failed")));
    assert!(!c.note(&test, &error("exit code 101\n1 failed")));
    assert!(!c.note(&test, &error("exit code 101\n1 failed")));
    assert!(c.note(&test, &error("exit code 101\n1 failed")));
}

#[test]
fn five_unreadable_calls_in_a_row_are_circles() {
    let mut c = Circles::default();
    let unknown = call("make_coffee", "{}");
    let broken = call("read_file", r#"{"path": "#);
    for i in 0..4 {
        let which = if i % 2 == 0 { &unknown } else { &broken };
        assert!(!c.note(which, &error("could not be read")), "{i}");
    }
    assert!(c.note(&unknown, &error("no such tool")));
}

#[test]
fn a_readable_call_resets_the_unreadable_count_and_an_overflow_counts_toward_it() {
    let mut c = Circles::default();
    let unknown = call("make_coffee", "{}");
    for _ in 0..4 {
        assert!(!c.note(&unknown, &error("x")));
    }
    assert!(!c.note(&call("list_dir", "{}"), &outcome(ToolStatus::Ok, "a")));
    for _ in 0..3 {
        assert!(!c.note(&unknown, &error("x")));
    }
    assert!(!c.overflowed());
    assert!(c.note(&unknown, &error("x")));
}

// ---------------------------------------------------------------------------
// Trimming
// ---------------------------------------------------------------------------

fn text(role: Role, t: &str) -> WireMessage {
    WireMessage {
        role,
        parts: vec![Part::Text(t.into())],
        tool_calls: vec![],
        tool_call_id: None,
    }
}

/// "sys", "go", then `n` pairs of an assistant call (11 characters) and a
/// 1,000-character result: 5 + 8 × 1,011 = 8,093 characters for n = 8.
fn conversation(n: usize) -> Vec<WireMessage> {
    let mut out = vec![text(Role::System, "sys"), text(Role::User, "go")];
    for i in 0..n {
        out.push(WireMessage {
            role: Role::Assistant,
            parts: vec![],
            tool_calls: vec![ToolCall {
                id: format!("c{i}"),
                name: "read_file".into(),
                arguments: "{}".into(),
            }],
            tool_call_id: None,
        });
        out.push(WireMessage {
            role: Role::Tool,
            parts: vec![Part::Text("x".repeat(1_000))],
            tool_calls: vec![],
            tool_call_id: Some(format!("c{i}")),
        });
    }
    out
}

fn result_texts(messages: &[WireMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| match &m.parts[..] {
            [Part::Text(t)] => t.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn shape(messages: &[WireMessage]) -> Vec<(Role, Option<String>, usize)> {
    messages
        .iter()
        .map(|m| (m.role, m.tool_call_id.clone(), m.tool_calls.len()))
        .collect()
}

#[test]
fn over_seventy_percent_replaces_the_oldest_results_first_and_keeps_every_message() {
    // 8,093 characters of a 10,000-character (2,500-token) context: 81%.
    // One result trimmed leaves 7,123 (71%), two leave 6,153 (62%).
    let mut messages = conversation(8);
    let before = shape(&messages);
    let mut trimmed = HashSet::new();
    assert!(trim(&mut messages, 2_500, 0, &mut trimmed));
    assert_eq!(shape(&messages), before, "no message removed or moved");
    let texts = result_texts(&messages);
    assert_eq!(texts[0], TRIMMED);
    assert_eq!(texts[1], TRIMMED);
    for t in &texts[2..] {
        assert_eq!(t.len(), 1_000);
    }
    assert_eq!(trimmed, HashSet::from(["c0".to_string(), "c1".to_string()]));
}

#[test]
fn at_or_under_seventy_percent_nothing_changes() {
    let mut messages = conversation(8);
    let original = messages.clone();
    let mut trimmed = HashSet::new();
    // 8,093 characters of 12,000 (3,000 tokens) is 67%.
    assert!(trim(&mut messages, 3_000, 0, &mut trimmed));
    assert_eq!(messages, original);
    assert!(trimmed.is_empty());
    // An unknown context length skips trimming.
    assert!(trim(&mut messages, 0, 0, &mut trimmed));
    assert_eq!(messages, original);
}

#[test]
fn the_fixed_part_of_the_request_counts_toward_the_estimate() {
    let mut messages = conversation(8);
    let original = messages.clone();
    let mut trimmed = HashSet::new();
    // 67% alone; with 1,000 characters of tool definitions, 76%.
    assert!(trim(&mut messages, 3_000, 1_000, &mut trimmed));
    assert_ne!(messages, original);
    assert_eq!(result_texts(&messages)[0], TRIMMED);
}

#[test]
fn the_last_four_results_are_never_trimmed_and_then_it_cannot_fit() {
    // 2,800 characters is 70% of 1,000 tokens; four untouched results alone
    // are 4,000.
    let mut messages = conversation(8);
    let mut trimmed = HashSet::new();
    assert!(!trim(&mut messages, 1_000, 0, &mut trimmed));
    let texts = result_texts(&messages);
    for t in &texts[..4] {
        assert_eq!(t, TRIMMED);
    }
    for t in &texts[4..] {
        assert_eq!(t.len(), 1_000);
    }
    assert_eq!(messages.len(), conversation(8).len());
}

#[test]
fn a_result_trimmed_once_stays_trimmed_in_later_requests() {
    let mut first = conversation(8);
    let mut trimmed = HashSet::new();
    assert!(trim(&mut first, 2_500, 0, &mut trimmed));
    // The next request has more room (a larger model, a summary...), but
    // its start stays the same bytes as the request before.
    let mut next = conversation(9);
    assert!(trim(&mut next, 100_000, 0, &mut trimmed));
    assert_eq!(next[..first.len()], first[..]);
}

#[test]
fn assistant_user_and_system_messages_are_never_trimmed() {
    let mut messages = vec![
        text(Role::System, &"s".repeat(5_000)),
        text(Role::User, &"u".repeat(5_000)),
        text(Role::Assistant, &"a".repeat(5_000)),
    ];
    let original = messages.clone();
    let mut trimmed = HashSet::new();
    assert!(!trim(&mut messages, 1_000, 0, &mut trimmed));
    assert_eq!(messages, original);
}
