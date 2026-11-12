use super::*;

// 2026-11-15T12:00:00Z
const NOV_15: i64 = 1_794_744_000;
// 2026-11-01T00:00:00Z and 2026-12-01T00:00:00Z
const NOV_1: i64 = 1_793_491_200;
const DEC_1: i64 = 1_796_083_200;

fn store() -> Store {
    Store::in_memory().unwrap()
}

#[test]
fn create_then_list_newest_first() {
    let s = store();
    let a = s.create_chat("m/a").unwrap();
    let b = s.create_chat("m/b").unwrap();
    assert_eq!(a.model_id, "m/a");
    assert_ne!(a.id, b.id);
    let ids: Vec<_> = s.list_chats().unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![b.id.clone(), a.id.clone()]);
    // Activity in the older chat moves it to the top.
    s.append_message(&a.id, "user", "hi", &[], false).unwrap();
    s.touch_chat_at(&a.id, a.updated_at + 10).unwrap();
    let ids: Vec<_> = s.list_chats().unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![a.id, b.id]);
}

#[test]
fn rename() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.rename_chat(&c.id, "Borrow checker").unwrap();
    assert_eq!(s.list_chats().unwrap()[0].title, "Borrow checker");
}

#[test]
fn delete_removes_messages_and_chat_usage_but_not_recorded_spend() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let keep = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "hi", &[], false).unwrap();
    s.record_usage_at(&c.id, 2, 0.5, NOV_15).unwrap();
    s.record_usage_at(&keep.id, 1, 0.25, NOV_15).unwrap();
    s.delete_chat(&c.id).unwrap();
    assert!(s.get_messages(&c.id).unwrap().is_empty());
    assert_eq!(s.chat_usage(&c.id).unwrap(), (0, 0.0));
    // Spend is kept per day, so deleting a chat does not lower it.
    assert_eq!(s.month_spend(NOV_15).unwrap(), 0.75);
    let ids: Vec<_> = s.list_chats().unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![keep.id]);
}

#[test]
fn title_defaults_to_first_40_chars_of_first_user_message_on_a_word_boundary() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.append_message(
        &c.id,
        "user",
        "Explain how the   borrow checker handles closures that capture by reference",
        &[],
        false,
    )
    .unwrap();
    assert_eq!(
        s.get_chat(&c.id).unwrap().unwrap().title,
        "Explain how the borrow checker handles"
    );
    // Later messages do not change it.
    s.append_message(&c.id, "user", "Another question", &[], false)
        .unwrap();
    assert_eq!(
        s.get_chat(&c.id).unwrap().unwrap().title,
        "Explain how the borrow checker handles"
    );
    // A short message is used whole; one long word is cut at 40 chars.
    let short = s.create_chat("m/a").unwrap();
    s.append_message(&short.id, "user", "  Fix this  ", &[], false)
        .unwrap();
    assert_eq!(s.get_chat(&short.id).unwrap().unwrap().title, "Fix this");
    let long = s.create_chat("m/a").unwrap();
    let word = "x".repeat(60);
    s.append_message(&long.id, "user", &word, &[], false)
        .unwrap();
    assert_eq!(s.get_chat(&long.id).unwrap().unwrap().title, "x".repeat(40));
}

#[test]
fn month_spend_sums_only_the_current_calendar_month() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.record_usage_at(&c.id, 1, 1.0, NOV_1).unwrap(); // first second of November
    s.record_usage_at(&c.id, 1, 2.0, NOV_1 - 1).unwrap(); // last second of October
    s.record_usage_at(&c.id, 1, 4.0, NOV_15).unwrap();
    s.record_usage_at(&c.id, 1, 8.0, DEC_1).unwrap(); // December 1st
    assert_eq!(s.month_spend(NOV_15).unwrap(), 5.0);
    assert_eq!(s.chat_usage(&c.id).unwrap(), (4, 15.0));
}

#[test]
fn reopening_the_same_file_keeps_chats() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chats.db");
    let id = {
        let s = Store::open(&path).unwrap();
        let c = s.create_chat("m/a").unwrap();
        s.append_message(&c.id, "user", "hello", &[], false)
            .unwrap();
        c.id
    };
    let s = Store::open(&path).unwrap();
    assert_eq!(s.list_chats().unwrap()[0].id, id);
    assert_eq!(s.get_messages(&id).unwrap()[0].text, "hello");
}

#[test]
fn a_message_with_interrupted_true_roundtrips() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let att = StoredAttachment {
        name: "a.png".into(),
        mime: "image/png".into(),
        data_base64: "AAAA".into(),
    };
    s.append_message(&c.id, "user", "look", std::slice::from_ref(&att), false)
        .unwrap();
    let m = s
        .append_message(&c.id, "assistant", "Hel", &[], true)
        .unwrap();
    let all = s.get_messages(&c.id).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].attachments, vec![att]);
    assert!(!all[0].interrupted);
    assert_eq!(all[1], m);
    assert!(all[1].interrupted);
    assert_eq!(all[1].role, "assistant");
}

#[test]
fn set_chat_model_remembers_the_last_model() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.set_chat_model(&c.id, "m/b").unwrap();
    assert_eq!(s.get_chat(&c.id).unwrap().unwrap().model_id, "m/b");
}

#[test]
fn delete_message_removes_only_that_message() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let other = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "q", &[], false).unwrap();
    let partial = s
        .append_message(&c.id, "assistant", "par", &[], true)
        .unwrap();
    let foreign = s
        .append_message(&other.id, "user", "x", &[], false)
        .unwrap();
    s.delete_message(&c.id, &foreign.id).unwrap();
    assert_eq!(s.get_messages(&other.id).unwrap().len(), 1);
    s.delete_message(&c.id, &partial.id).unwrap();
    let texts: Vec<_> = s
        .get_messages(&c.id)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(texts, vec!["q"]);
}

// ---- schema version 2 ----

const DB_V1: &str = include_str!("../../tests/fixtures/db_v1.sql");

fn version(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn v1_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("beluga.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(DB_V1)
        .unwrap();
    path
}

#[test]
fn a_v1_database_upgrades_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_file(&dir);
    assert_eq!(version(&path), 1);
    let s = Store::open(&path).unwrap();
    assert_eq!(version(&path), 4);
    let chats = s.list_chats().unwrap();
    assert_eq!(chats.len(), 2);
    let a = s.get_messages("chat-a").unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(a[0].text, "Look at this");
    assert_eq!(a[0].attachments.len(), 1);
    assert_eq!(a[0].attachments[0].name, "notes.md");
    let b = s.get_messages("chat-b").unwrap();
    assert_eq!(b.len(), 2);
    assert!(b[1].interrupted);
    assert_eq!(s.chat_usage("chat-a").unwrap(), (1, 0.25));
    assert_eq!(s.chat_usage("chat-b").unwrap(), (2, 0.5));
    // New state defaults on upgraded chats.
    assert!(s.active_skills("chat-a").unwrap().is_empty());
    assert_eq!(s.folder("chat-a").unwrap(), None);
    assert_eq!(s.summary("chat-a").unwrap(), None);
}

#[test]
fn upgraded_messages_have_kind_message() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&v1_file(&dir)).unwrap();
    for id in ["chat-a", "chat-b"] {
        assert!(
            s.get_messages(id)
                .unwrap()
                .iter()
                .all(|m| m.kind == "message")
        );
    }
}

#[test]
fn opening_twice_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_file(&dir);
    drop(Store::open(&path).unwrap());
    let s = Store::open(&path).unwrap();
    assert_eq!(version(&path), 4);
    assert_eq!(s.get_messages("chat-a").unwrap().len(), 2);
}

#[test]
fn a_new_database_is_created_at_version_4() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.db");
    drop(Store::open(&path).unwrap());
    assert_eq!(version(&path), 4);
    let s = Store::in_memory().unwrap();
    let c = s.create_chat("m/a").unwrap();
    assert!(s.active_skills(&c.id).unwrap().is_empty());
}

#[test]
fn a_newer_database_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE future (x INTEGER); PRAGMA user_version = 5;")
        .unwrap();
    let err = Store::open(&path).err().expect("must be refused");
    assert_eq!(
        err.to_string(),
        "This database was made by a newer version of the app."
    );
    assert_eq!(version(&path), 5);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
}

#[test]
fn notes_are_listed_by_get_messages_but_not_by_sendable_messages() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "hi", &[], false).unwrap();
    let n = s.append_note(&c.id, "Skill turned on").unwrap();
    assert_eq!(n.kind, "note");
    assert_eq!(n.text, "Skill turned on");
    s.append_message(&c.id, "assistant", "yo", &[], false)
        .unwrap();
    let all = s.get_messages(&c.id).unwrap();
    assert_eq!(
        all.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>(),
        ["message", "note", "message"]
    );
    assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(all[1].id, n.id);
    let sendable = s.sendable_messages(&c.id).unwrap();
    assert_eq!(
        sendable.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["hi", "yo"]
    );
}

#[test]
fn active_skills_roundtrip_and_default_to_empty() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    assert!(s.active_skills(&c.id).unwrap().is_empty());
    let ids = vec!["p:one".to_string(), "two".to_string()];
    s.set_active_skills(&c.id, &ids).unwrap();
    assert_eq!(s.active_skills(&c.id).unwrap(), ids);
    s.set_active_skills(&c.id, &[]).unwrap();
    assert!(s.active_skills(&c.id).unwrap().is_empty());
}

#[test]
fn folder_roundtrip_and_clear() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    assert_eq!(s.folder(&c.id).unwrap(), None);
    s.set_folder(&c.id, Some("/tmp/work")).unwrap();
    assert_eq!(s.folder(&c.id).unwrap().as_deref(), Some("/tmp/work"));
    s.set_folder(&c.id, None).unwrap();
    assert_eq!(s.folder(&c.id).unwrap(), None);
}

#[test]
fn a_folder_change_clears_the_edits_flag_and_the_same_folder_keeps_it() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.set_folder(&c.id, Some("/tmp/a")).unwrap();
    s.set_edits_allowed(&c.id, true).unwrap();
    // The same folder again: kept.
    s.set_folder(&c.id, Some("/tmp/a")).unwrap();
    assert!(s.edits_allowed(&c.id).unwrap());
    // Another folder: cleared.
    s.set_folder(&c.id, Some("/tmp/b")).unwrap();
    assert!(!s.edits_allowed(&c.id).unwrap());
    // Closed, then the same folder reopened: cleared by the close.
    s.set_edits_allowed(&c.id, true).unwrap();
    s.set_folder(&c.id, None).unwrap();
    assert!(!s.edits_allowed(&c.id).unwrap());
    s.set_folder(&c.id, Some("/tmp/b")).unwrap();
    assert!(!s.edits_allowed(&c.id).unwrap());
    // From no folder to a folder (a chat opened with edits allowed).
    s.set_edits_allowed(&c.id, true).unwrap();
    s.set_folder(&c.id, None).unwrap();
    s.set_edits_allowed(&c.id, true).unwrap();
    s.set_folder(&c.id, Some("/tmp/c")).unwrap();
    assert!(!s.edits_allowed(&c.id).unwrap());
    // Another chat is untouched.
    let d = s.create_chat("m/a").unwrap();
    s.set_edits_allowed(&d.id, true).unwrap();
    s.set_folder(&c.id, Some("/tmp/d")).unwrap();
    assert!(s.edits_allowed(&d.id).unwrap());
}

#[test]
fn summary_sets_the_boundary() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let m1 = s.append_message(&c.id, "user", "one", &[], false).unwrap();
    let m2 = s
        .append_message(&c.id, "assistant", "two", &[], false)
        .unwrap();
    let m3 = s
        .append_message(&c.id, "user", "three", &[], false)
        .unwrap();
    assert_eq!(s.sendable_messages(&c.id).unwrap().len(), 3);
    assert_eq!(s.summary(&c.id).unwrap(), None);
    s.set_summary(&c.id, "Earlier: one and two", m3.seq)
        .unwrap();
    assert_eq!(
        s.summary(&c.id).unwrap(),
        Some(("Earlier: one and two".to_string(), m3.seq))
    );
    let sendable = s.sendable_messages(&c.id).unwrap();
    assert_eq!(
        sendable.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [m3.id.as_str()]
    );
    assert!(m1.seq < m2.seq && m2.seq < m3.seq);
}

#[test]
fn delete_from_removes_the_message_and_everything_after_including_notes_and_keeps_earlier_rows() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let other = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "one", &[], false).unwrap();
    let m2 = s
        .append_message(&c.id, "assistant", "two", &[], false)
        .unwrap();
    s.append_note(&c.id, "note").unwrap();
    s.append_message(&c.id, "user", "four", &[], false).unwrap();
    s.append_message(&other.id, "user", "elsewhere", &[], false)
        .unwrap();
    s.delete_from(&c.id, &m2.id).unwrap();
    let left = s.get_messages(&c.id).unwrap();
    assert_eq!(
        left.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["one"]
    );
    assert_eq!(s.get_messages(&other.id).unwrap().len(), 1);
}

#[test]
fn delete_from_with_an_unknown_id_changes_nothing_and_errors() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let other = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "one", &[], false).unwrap();
    let theirs = s
        .append_message(&other.id, "user", "theirs", &[], false)
        .unwrap();
    s.append_message(&other.id, "assistant", "reply", &[], false)
        .unwrap();
    assert!(s.delete_from(&c.id, "no-such-id").is_err());
    // An id from another chat counts as unknown.
    assert!(s.delete_from(&c.id, &theirs.id).is_err());
    assert_eq!(s.get_messages(&c.id).unwrap().len(), 1);
    assert_eq!(s.get_messages(&other.id).unwrap().len(), 2);
}

#[test]
fn switches_roundtrip_and_unknown_is_none() {
    let s = store();
    assert_eq!(s.switch("pack/x").unwrap(), None);
    s.set_switch("pack/x", true).unwrap();
    assert_eq!(s.switch("pack/x").unwrap(), Some(true));
    s.set_switch("pack/x", false).unwrap();
    assert_eq!(s.switch("pack/x").unwrap(), Some(false));
    assert_eq!(s.switch("p:skill").unwrap(), None);
}

#[test]
fn delete_switch_and_delete_switches_under_forget_only_what_they_name() {
    let s = store();
    for id in ["pack/x", "x:a", "x:b", "xy:a", "x_:a", "a"] {
        s.set_switch(id, true).unwrap();
    }
    s.delete_switch("a").unwrap();
    s.delete_switches_under("x:").unwrap();
    s.delete_switch("pack/x").unwrap();
    for gone in ["a", "x:a", "x:b", "pack/x"] {
        assert_eq!(s.switch(gone).unwrap(), None, "{gone}");
    }
    // A prefix is matched exactly, never as a LIKE pattern.
    for kept in ["xy:a", "x_:a"] {
        assert_eq!(s.switch(kept).unwrap(), Some(true), "{kept}");
    }
    s.delete_switches_under("x_").unwrap();
    assert_eq!(s.switch("x_:a").unwrap(), None);
    assert_eq!(s.switch("xy:a").unwrap(), Some(true));
}

#[test]
fn deleting_a_chat_still_cascades() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&v1_file(&dir)).unwrap();
    s.append_note("chat-a", "n").unwrap();
    s.delete_chat("chat-a").unwrap();
    assert!(s.get_messages("chat-a").unwrap().is_empty());
    assert_eq!(s.chat_usage("chat-a").unwrap(), (0, 0.0));
    assert_eq!(s.get_messages("chat-b").unwrap().len(), 2);
}

#[test]
fn delete_from_and_fix_summary_clears_a_summary_that_described_deleted_rows_in_the_same_step() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let m1 = s.append_message(&c.id, "user", "one", &[], false).unwrap();
    s.append_message(&c.id, "assistant", "two", &[], false)
        .unwrap();
    let m3 = s
        .append_message(&c.id, "user", "three", &[], false)
        .unwrap();
    s.append_note(&c.id, "Summarised above this line").unwrap();
    s.set_summary(&c.id, "Earlier", m3.seq).unwrap();

    // An unknown id changes neither the rows nor the summary.
    assert!(matches!(
        s.delete_from_and_fix_summary(&c.id, "nope"),
        Err(rusqlite::Error::QueryReturnedNoRows)
    ));
    assert_eq!(s.get_messages(&c.id).unwrap().len(), 4);
    assert_eq!(
        s.summary(&c.id).unwrap(),
        Some(("Earlier".to_string(), m3.seq))
    );

    // At the boundary: rows go, the summary stays.
    s.delete_from_and_fix_summary(&c.id, &m3.id).unwrap();
    assert_eq!(s.get_messages(&c.id).unwrap().len(), 2);
    assert_eq!(
        s.summary(&c.id).unwrap(),
        Some(("Earlier".to_string(), m3.seq))
    );

    // Before the boundary: rows and summary go together.
    s.delete_from_and_fix_summary(&c.id, &m1.id).unwrap();
    assert!(s.get_messages(&c.id).unwrap().is_empty());
    assert_eq!(s.summary(&c.id).unwrap(), None);
}

#[test]
fn approved_folders_survive_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    let s = Store::open(&path).unwrap();
    s.approve_folder("/one").unwrap();
    s.approve_folder("/two").unwrap();
    s.approve_folder("/one").unwrap();
    assert!(s.is_folder_approved("/two").unwrap());
    assert!(!s.is_folder_approved("/three").unwrap());
    drop(s);
    let s = Store::open(&path).unwrap();
    assert!(s.is_folder_approved("/one").unwrap());
    assert_eq!(s.approved_folders().unwrap(), vec!["/one", "/two"]);
}

// ---- schema version 4 ----

const DB_V2: &str = include_str!("../../tests/fixtures/db_v2.sql");
const DB_V3: &str = include_str!("../../tests/fixtures/db_v3.sql");
const NOV_15_DAY: &str = "2026-11-15";
const NOV_16_DAY: &str = "2026-11-16";

fn file_from(dir: &tempfile::TempDir, sql: &str) -> std::path::PathBuf {
    let path = dir.path().join("beluga.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
    path
}

fn call(id: &str) -> StoredToolCall {
    StoredToolCall {
        id: id.to_string(),
        name: "read_file".to_string(),
        arguments: "{\"path\":\"a.txt\"}".to_string(),
    }
}

fn meta(status: &str) -> ToolMeta {
    ToolMeta {
        name: "read_file".to_string(),
        status: status.to_string(),
        summary: "Read a.txt".to_string(),
        detail: Some("one line".to_string()),
    }
}

#[test]
fn a_v3_database_upgrades_to_4_and_keeps_every_chat_and_message_sendable() {
    let dir = tempfile::tempdir().unwrap();
    let path = file_from(&dir, DB_V3);
    assert_eq!(version(&path), 3);
    let s = Store::open(&path).unwrap();
    assert_eq!(version(&path), 4);
    assert_eq!(s.list_chats().unwrap().len(), 2);
    assert_eq!(s.get_messages("chat-a").unwrap().len(), 3);
    assert_eq!(s.get_messages("chat-b").unwrap().len(), 2);
    let sendable = s.sendable_messages("chat-a").unwrap();
    assert_eq!(
        sendable.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["It says hi."]
    );
    assert!(
        sendable
            .iter()
            .all(|m| m.tool_calls.is_empty() && m.tool_call_id.is_none() && m.tool_meta.is_none())
    );
    assert_eq!(s.sendable_messages("chat-b").unwrap().len(), 2);
    assert_eq!(s.folder("chat-a").unwrap().as_deref(), Some("/work/one"));
    assert!(s.is_folder_approved("/work/one").unwrap());
    assert!(!s.edits_allowed("chat-a").unwrap());
    assert_eq!(s.plan("chat-a").unwrap(), "[]");
    assert_eq!(
        s.summary("chat-a").unwrap(),
        Some(("Earlier: they looked at a file.".to_string(), 2))
    );
    assert_eq!(s.active_skills("chat-a").unwrap(), ["skill-a"]);
    assert_eq!(
        s.get_messages("chat-a").unwrap()[2].kind,
        "note",
        "the note row survives"
    );
}

#[test]
fn a_v2_database_upgrades_with_its_chat_state_and_an_empty_approved_folders() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&file_from(&dir, DB_V2)).unwrap();
    assert_eq!(s.approved_folders().unwrap(), Vec::<String>::new());
    assert_eq!(s.folder("chat-a").unwrap().as_deref(), Some("/work/one"));
    assert_eq!(s.active_skills("chat-a").unwrap(), ["skill-a"]);
    assert_eq!(
        s.summary("chat-a").unwrap(),
        Some(("Earlier: they looked at a file.".to_string(), 2))
    );
    let all = s.get_messages("chat-a").unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[2].kind, "note");
    assert_eq!(all[2].text, "Skill turned on");
    assert_eq!(s.sendable_messages("chat-a").unwrap().len(), 1);
}

#[test]
fn a_v3_database_keeps_its_approved_folder() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&file_from(&dir, DB_V3)).unwrap();
    assert_eq!(s.approved_folders().unwrap(), ["/work/one"]);
}

#[test]
fn v1_and_v2_fixtures_upgrade_through_to_4() {
    for sql in [DB_V1, DB_V2] {
        let dir = tempfile::tempdir().unwrap();
        let path = file_from(&dir, sql);
        let s = Store::open(&path).unwrap();
        assert_eq!(version(&path), 4);
        assert_eq!(s.list_chats().unwrap().len(), 2);
        assert_eq!(s.sendable_messages("chat-b").unwrap().len(), 2);
        assert_eq!(s.chat_usage("chat-b").unwrap(), (2, 0.5));
        assert!(s.rules("/work/one").unwrap().is_empty());
        assert_eq!(s.setting("anything").unwrap(), None);
    }
}

#[test]
fn upgrade_backfills_daily_usage_from_usage_rows_grouped_by_utc_day() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&file_from(&dir, DB_V3)).unwrap();
    assert_eq!(s.day_usage(NOV_15_DAY).unwrap(), (3, 0.75));
    assert_eq!(s.day_usage(NOV_16_DAY).unwrap(), (1, 0.125));
    assert_eq!(s.day_usage("2026-11-14").unwrap(), (0, 0.0));
    assert_eq!(s.month_spend(NOV_15).unwrap(), 0.875);
}

#[test]
fn deleting_a_chat_leaves_day_usage_and_month_spend_unchanged() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.record_usage_at(&c.id, 2, 0.5, NOV_15).unwrap();
    s.record_usage_at(&c.id, 1, 0.25, NOV_15 + 60).unwrap();
    assert_eq!(s.day_usage(NOV_15_DAY).unwrap(), (3, 0.75));
    s.delete_chat(&c.id).unwrap();
    assert_eq!(s.chat_usage(&c.id).unwrap(), (0, 0.0));
    assert_eq!(s.day_usage(NOV_15_DAY).unwrap(), (3, 0.75));
    assert_eq!(s.month_spend(NOV_15).unwrap(), 0.75);
}

#[test]
fn append_assistant_stores_and_returns_tool_calls() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let m = s
        .append_assistant(&c.id, "Looking.", &[call("c1"), call("c2")], false)
        .unwrap();
    assert_eq!(m.role, "assistant");
    assert_eq!(m.kind, "message");
    assert_eq!(m.text, "Looking.");
    assert_eq!(m.tool_calls, vec![call("c1"), call("c2")]);
    let back = s.get_messages(&c.id).unwrap();
    assert_eq!(back, vec![m]);
    let plain = s.append_assistant(&c.id, "Done.", &[], true).unwrap();
    assert!(plain.tool_calls.is_empty());
    assert!(plain.interrupted);
}

#[test]
fn append_tool_result_stores_call_id_meta_and_text() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let r = s
        .append_tool_result(&c.id, "c1", &meta("ok"), "file text")
        .unwrap();
    assert_eq!(r.role, "tool");
    assert_eq!(r.kind, "tool");
    assert_eq!(r.text, "file text");
    assert_eq!(r.tool_call_id.as_deref(), Some("c1"));
    assert_eq!(r.tool_meta, Some(meta("ok")));
    let back = s.get_messages(&c.id).unwrap();
    assert_eq!(back, vec![r]);
    let json = serde_json::to_value(&back[0]).unwrap();
    assert_eq!(json["tool_meta"]["status"], "ok");
    assert_eq!(json["tool_meta"]["detail"], "one line");
    assert_eq!(json["tool_call_id"], "c1");
}

#[test]
fn sendable_messages_returns_message_and_tool_rows_in_seq_order_and_never_notes() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "go", &[], false).unwrap();
    s.append_assistant(&c.id, "", &[call("c1")], false).unwrap();
    s.append_note(&c.id, "a note").unwrap();
    s.append_tool_result(&c.id, "c1", &meta("ok"), "result")
        .unwrap();
    s.append_assistant(&c.id, "all done", &[], false).unwrap();
    let sendable = s.sendable_messages(&c.id).unwrap();
    assert_eq!(
        sendable.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>(),
        ["message", "message", "tool", "message"]
    );
    assert!(sendable.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(s.get_messages(&c.id).unwrap().len(), 5);
}

#[test]
fn unanswered_calls_lists_a_call_with_no_result_and_nothing_once_it_is_appended() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "go", &[], false).unwrap();
    s.append_assistant(&c.id, "", &[call("c1"), call("c2")], false)
        .unwrap();
    s.append_tool_result(&c.id, "c1", &meta("ok"), "r").unwrap();
    assert_eq!(s.unanswered_calls(&c.id).unwrap(), vec![call("c2")]);
    s.append_tool_result(&c.id, "c2", &meta("not_run"), "r")
        .unwrap();
    assert!(s.unanswered_calls(&c.id).unwrap().is_empty());
}

#[test]
fn delete_from_removes_tool_rows_after_the_edited_message() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let u = s.append_message(&c.id, "user", "go", &[], false).unwrap();
    s.append_assistant(&c.id, "", &[call("c1")], false).unwrap();
    s.append_tool_result(&c.id, "c1", &meta("ok"), "r").unwrap();
    s.delete_from(&c.id, &u.id).unwrap();
    assert!(s.get_messages(&c.id).unwrap().is_empty());
    assert!(s.unanswered_calls(&c.id).unwrap().is_empty());
}

#[test]
fn rules_are_per_folder_add_is_idempotent_and_removing_an_unknown_rule_is_fine() {
    let s = store();
    s.add_rule("/p/one", "git status").unwrap();
    s.add_rule("/p/one", "git status").unwrap();
    s.add_rule("/p/one", "npm test").unwrap();
    s.add_rule("/p/two", "ls").unwrap();
    let mut one = s.rules("/p/one").unwrap();
    one.sort();
    assert_eq!(one, ["git status", "npm test"]);
    assert_eq!(s.rules("/p/two").unwrap(), ["ls"]);
    s.remove_rule("/p/one", "never added").unwrap();
    s.remove_rule("/p/one", "git status").unwrap();
    assert_eq!(s.rules("/p/one").unwrap(), ["npm test"]);
    assert_eq!(s.rules("/p/two").unwrap(), ["ls"]);
}

#[test]
fn plan_and_edits_allowed_round_trip_and_default_off() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    assert!(!s.edits_allowed(&c.id).unwrap());
    assert_eq!(s.plan(&c.id).unwrap(), "[]");
    s.set_edits_allowed(&c.id, true).unwrap();
    assert!(s.edits_allowed(&c.id).unwrap());
    let plan = r#"[{"step":"Read","status":"done"}]"#;
    s.set_plan(&c.id, plan).unwrap();
    assert_eq!(s.plan(&c.id).unwrap(), plan);
    s.set_edits_allowed(&c.id, false).unwrap();
    assert!(!s.edits_allowed(&c.id).unwrap());
}

#[test]
fn settings_round_trip_and_overwrite() {
    let s = store();
    assert_eq!(s.setting("daily_limit").unwrap(), None);
    s.set_setting("daily_limit", "1.00").unwrap();
    s.set_setting("daily_limit", "2.50").unwrap();
    assert_eq!(s.setting("daily_limit").unwrap().as_deref(), Some("2.50"));
}

// ---- tool calls and the compaction boundary ----

/// user, assistant with call `id`, tool result for `id`; returns the seqs.
fn tool_turn(s: &Store, chat: &str, id: &str, answer: bool) -> Vec<i64> {
    let mut seqs = vec![
        s.append_message(chat, "user", "go", &[], false)
            .unwrap()
            .seq,
    ];
    seqs.push(
        s.append_assistant(chat, "", &[call(id)], false)
            .unwrap()
            .seq,
    );
    if answer {
        seqs.push(
            s.append_tool_result(chat, id, &meta("ok"), "r")
                .unwrap()
                .seq,
        );
    }
    seqs
}

#[test]
fn a_boundary_before_the_assistant_row_keeps_the_whole_pair() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let seqs = tool_turn(&s, &c.id, "c1", true);
    s.set_summary(&c.id, "sum", seqs[1]).unwrap();
    let kinds: Vec<_> = s
        .sendable_messages(&c.id)
        .unwrap()
        .iter()
        .map(|m| m.kind.clone())
        .collect();
    assert_eq!(kinds, ["message", "tool"]);
    assert!(s.unanswered_calls(&c.id).unwrap().is_empty());
}

#[test]
fn a_boundary_splitting_the_pair_drops_the_orphan_tool_rows() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let seqs = tool_turn(&s, &c.id, "c1", true);
    s.set_summary(&c.id, "sum", seqs[2]).unwrap();
    assert!(s.sendable_messages(&c.id).unwrap().is_empty());
    assert!(s.unanswered_calls(&c.id).unwrap().is_empty());
    // Rows after the orphan are kept.
    s.append_assistant(&c.id, "done", &[], false).unwrap();
    let rows = s.sendable_messages(&c.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "done");
}

#[test]
fn a_boundary_after_the_pair_leaves_later_rows() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    tool_turn(&s, &c.id, "c1", true);
    let later = tool_turn(&s, &c.id, "c2", false);
    s.set_summary(&c.id, "sum", later[0]).unwrap();
    let rows = s.sendable_messages(&c.id).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(s.unanswered_calls(&c.id).unwrap(), vec![call("c2")]);
}

#[test]
fn a_reused_call_id_in_a_later_turn_is_not_hidden_by_an_old_result() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    tool_turn(&s, &c.id, "call_0", true);
    tool_turn(&s, &c.id, "call_0", false);
    assert_eq!(s.unanswered_calls(&c.id).unwrap(), vec![call("call_0")]);
    s.append_tool_result(&c.id, "call_0", &meta("ok"), "r")
        .unwrap();
    assert!(s.unanswered_calls(&c.id).unwrap().is_empty());
}
