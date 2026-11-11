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
fn delete_removes_messages_and_usage() {
    let s = store();
    let c = s.create_chat("m/a").unwrap();
    let keep = s.create_chat("m/a").unwrap();
    s.append_message(&c.id, "user", "hi", &[], false).unwrap();
    s.record_usage_at(&c.id, 2, 0.5, NOV_15).unwrap();
    s.record_usage_at(&keep.id, 1, 0.25, NOV_15).unwrap();
    s.delete_chat(&c.id).unwrap();
    assert!(s.get_messages(&c.id).unwrap().is_empty());
    assert_eq!(s.chat_usage(&c.id).unwrap(), (0, 0.0));
    assert_eq!(s.month_spend(NOV_15).unwrap(), 0.25);
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
    assert_eq!(version(&path), 2);
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
    assert_eq!(version(&path), 2);
    assert_eq!(s.get_messages("chat-a").unwrap().len(), 2);
}

#[test]
fn a_new_database_is_created_at_version_2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.db");
    drop(Store::open(&path).unwrap());
    assert_eq!(version(&path), 2);
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
        .execute_batch("CREATE TABLE future (x INTEGER); PRAGMA user_version = 3;")
        .unwrap();
    let err = Store::open(&path).err().expect("must be refused");
    assert_eq!(
        err.to_string(),
        "This database was made by a newer version of the app."
    );
    assert_eq!(version(&path), 3);
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
fn deleting_a_chat_still_cascades() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&v1_file(&dir)).unwrap();
    s.append_note("chat-a", "n").unwrap();
    s.delete_chat("chat-a").unwrap();
    assert!(s.get_messages("chat-a").unwrap().is_empty());
    assert_eq!(s.chat_usage("chat-a").unwrap(), (0, 0.0));
    assert_eq!(s.get_messages("chat-b").unwrap().len(), 2);
}
