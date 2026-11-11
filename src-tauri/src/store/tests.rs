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
