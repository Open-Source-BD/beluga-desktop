//! The approval gate: at most one card waits per chat, and only the answer
//! for exactly that card is accepted.
//!
//! `ask` gives the card a fresh random id, registers it, emits
//! `ApprovalNeeded` and waits. It ends on an answer, on cancel, or when the
//! chat's waiting card is dropped (`drop_chat`); every way but an accepted
//! answer is `Unanswered::Stopped`, which callers report as not run, never
//! as the user's decline. `ApprovalCleared` follows exactly once, also when the
//! waiting future is dropped. `answer` refuses, with one fixed message and
//! without changing anything, an id that is not the one waiting for that
//! chat and an answer the card does not offer.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{Answer, Approval};
use crate::engine::{EngineEvent, UiError};

pub const NOT_WAITING: &str = "That request is no longer waiting.";
/// Longest note or message kept from an answer, in characters.
const MAX_NOTE_CHARS: usize = 4_000;

/// Why `ask` returned without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unanswered {
    /// Another card is already waiting in this chat.
    Busy,
    /// The turn was stopped, or the chat deleted or its folder changed.
    Stopped,
}

struct Pending {
    approval: Approval,
    tx: oneshot::Sender<Answer>,
}

#[derive(Default)]
pub struct Approvals {
    waiting: Mutex<HashMap<String, Pending>>,
}

/// Removes the card (if it is still this one) and emits `ApprovalCleared`
/// when the wait ends in any way, the future being dropped included.
struct Clear<'a> {
    approvals: &'a Approvals,
    chat_id: &'a str,
    id: String,
    emit: &'a (dyn Fn(EngineEvent) + Send + Sync),
}

impl Drop for Clear<'_> {
    fn drop(&mut self) {
        {
            let mut w = self.approvals.lock();
            if w.get(self.chat_id)
                .is_some_and(|p| p.approval.id() == self.id)
            {
                w.remove(self.chat_id);
            }
        }
        (self.emit)(EngineEvent::ApprovalCleared {
            id: self.id.clone(),
        });
    }
}

/// Whether `answer` is one of the answers `approval` offers.
fn offers(approval: &Approval, answer: &Answer) -> bool {
    match (approval, answer) {
        (_, Answer::Decline { .. } | Answer::Message { .. }) => true,
        (Approval::Command { .. }, Answer::Once) => true,
        (Approval::Command { rule, .. }, Answer::Always) => rule.is_some(),
        (Approval::Edit { .. }, Answer::Once | Answer::AllEdits) => true,
        (Approval::Secret { .. }, Answer::Once) => true,
        (Approval::KeepGoing { .. }, Answer::KeepGoing | Answer::StopHere) => true,
        _ => false,
    }
}

fn cut(s: String) -> String {
    if s.chars().count() > MAX_NOTE_CHARS {
        s.chars().take(MAX_NOTE_CHARS).collect()
    } else {
        s
    }
}

fn cut_answer(answer: Answer) -> Answer {
    match answer {
        Answer::Decline { note } => Answer::Decline {
            note: note.map(cut),
        },
        Answer::Message { text } => Answer::Message { text: cut(text) },
        other => other,
    }
}

impl Approvals {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Pending>> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Asks the user and waits for the answer. `Err(Busy)`: another card
    /// already waits in this chat (a programming error; nothing is shown).
    /// `Err(Stopped)`: the turn was cancelled, or the chat was deleted or its
    /// folder changed, before an answer arrived.
    pub async fn ask(
        &self,
        chat_id: &str,
        mut approval: Approval,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<Answer, Unanswered> {
        if cancel.is_cancelled() {
            return Err(Unanswered::Stopped);
        }
        let id = uuid::Uuid::new_v4().to_string();
        approval.set_id(id.clone());
        let (tx, rx) = oneshot::channel();
        {
            let mut w = self.lock();
            if w.contains_key(chat_id) {
                return Err(Unanswered::Busy);
            }
            w.insert(
                chat_id.to_string(),
                Pending {
                    approval: approval.clone(),
                    tx,
                },
            );
        }
        let clear = Clear {
            approvals: self,
            chat_id,
            id,
            emit,
        };
        emit(EngineEvent::ApprovalNeeded { approval });
        // The sender is dropped without an answer by `drop_chat`.
        let answer = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            r = rx => r.ok(),
        };
        drop(clear);
        // A cancel that raced the answer wins.
        match answer {
            Some(a) if !cancel.is_cancelled() => Ok(a),
            _ => Err(Unanswered::Stopped),
        }
    }

    /// Delivers the user's answer to the card waiting in `chat_id`.
    pub fn answer(&self, chat_id: &str, approval_id: &str, answer: Answer) -> Result<(), UiError> {
        let refused = || UiError::other(NOT_WAITING);
        let pending = {
            let mut w = self.lock();
            let fits = w
                .get(chat_id)
                .is_some_and(|p| p.approval.id() == approval_id && offers(&p.approval, &answer));
            if !fits {
                return Err(refused());
            }
            w.remove(chat_id).ok_or_else(refused)?
        };
        pending.tx.send(cut_answer(answer)).map_err(|_| refused())
    }

    /// The card waiting in the chat, if any (to draw it again).
    pub fn waiting(&self, chat_id: &str) -> Option<Approval> {
        self.lock().get(chat_id).map(|p| p.approval.clone())
    }

    /// Drops the chat's waiting card; its waiter ends as `Stopped`.
    pub fn drop_chat(&self, chat_id: &str) {
        self.lock().remove(chat_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    type Events = Arc<Mutex<Vec<EngineEvent>>>;

    fn recorder() -> (Events, impl Fn(EngineEvent) + Send + Sync) {
        let events: Events = Arc::default();
        let sink = events.clone();
        (events, move |e| sink.lock().unwrap().push(e))
    }

    fn secret() -> Approval {
        Approval::Secret {
            id: String::new(),
            path: ".env".into(),
            folder: "/p".into(),
        }
    }

    fn command(rule: Option<&str>) -> Approval {
        Approval::Command {
            id: String::new(),
            command: "npm test".into(),
            folder: "/p".into(),
            rule: rule.map(str::to_string),
            outside: false,
            first_in_project: false,
            background: false,
        }
    }

    fn edit() -> Approval {
        Approval::Edit {
            id: String::new(),
            path: "a.txt".into(),
            folder: "/p".into(),
            created: false,
            removed: vec![],
            added: vec![],
            removed_total: 0,
            added_total: 0,
            total_lines: 1,
        }
    }

    fn keep_going() -> Approval {
        Approval::KeepGoing {
            id: String::new(),
            requests: 25,
            free_left: None,
            spent_today_usd: None,
        }
    }

    /// Waits until a card is waiting in the chat and returns its id.
    async fn waiting_id(a: &Approvals, chat: &str) -> String {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(w) = a.waiting(chat) {
                    return w.id().to_string();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("no card was shown")
    }

    #[tokio::test]
    async fn an_answer_reaches_the_waiting_card_and_the_card_is_cleared() {
        let a = Approvals::new();
        let (events, emit) = recorder();
        let cancel = CancellationToken::new();
        let (got, _) = tokio::join!(a.ask("c1", secret(), &emit, &cancel), async {
            let id = waiting_id(&a, "c1").await;
            a.answer("c1", &id, Answer::Once).unwrap();
        });
        assert_eq!(got, Ok(Answer::Once));
        assert!(a.waiting("c1").is_none());
        let ev = events.lock().unwrap().clone();
        let EngineEvent::ApprovalNeeded { approval } = &ev[0] else {
            panic!("{ev:?}");
        };
        assert!(!approval.id().is_empty());
        assert_eq!(
            ev[1],
            EngineEvent::ApprovalCleared {
                id: approval.id().to_string()
            }
        );
        assert_eq!(ev.len(), 2);
    }

    #[tokio::test]
    async fn ids_are_fresh_for_every_ask() {
        let a = Approvals::new();
        let (_, emit) = recorder();
        let cancel = CancellationToken::new();
        let mut seen = Vec::new();
        for _ in 0..3 {
            let (_, id) = tokio::join!(a.ask("c", secret(), &emit, &cancel), async {
                let id = waiting_id(&a, "c").await;
                a.answer("c", &id, Answer::Once).unwrap();
                id
            });
            assert!(!seen.contains(&id));
            seen.push(id);
        }
    }

    #[tokio::test]
    async fn wrong_stale_or_other_chat_ids_are_refused_and_change_nothing() {
        let a = Approvals::new();
        let (_, emit) = recorder();
        let cancel = CancellationToken::new();
        let (got, _) = tokio::join!(
            async {
                tokio::join!(
                    a.ask("A", secret(), &emit, &cancel),
                    a.ask("B", secret(), &emit, &cancel)
                )
            },
            async {
                let ida = waiting_id(&a, "A").await;
                let idb = waiting_id(&a, "B").await;
                for (chat, id) in [
                    ("A", idb.as_str()),
                    ("B", ida.as_str()),
                    ("A", "nope"),
                    ("A", ""),
                ] {
                    let e = a.answer(chat, id, Answer::Once).unwrap_err();
                    assert_eq!(e.message, NOT_WAITING);
                }
                assert_eq!(a.waiting("A").unwrap().id(), ida);
                assert_eq!(a.waiting("B").unwrap().id(), idb);
                a.answer("A", &ida, Answer::Decline { note: None }).unwrap();
                // The same answer again: refused.
                let e = a.answer("A", &ida, Answer::Once).unwrap_err();
                assert_eq!(e.message, NOT_WAITING);
                a.answer("B", &idb, Answer::Once).unwrap();
            }
        );
        assert_eq!(got.0, Ok(Answer::Decline { note: None }));
        assert_eq!(got.1, Ok(Answer::Once));
    }

    #[tokio::test]
    async fn answers_a_card_does_not_offer_are_refused_and_the_card_still_waits() {
        let cases: Vec<(Approval, Vec<Answer>, Vec<Answer>)> = vec![
            (
                command(None),
                vec![
                    Answer::Always,
                    Answer::AllEdits,
                    Answer::KeepGoing,
                    Answer::StopHere,
                ],
                vec![Answer::Once],
            ),
            (
                command(Some("npm test")),
                vec![Answer::AllEdits, Answer::KeepGoing, Answer::StopHere],
                vec![Answer::Always],
            ),
            (
                edit(),
                vec![Answer::Always, Answer::KeepGoing, Answer::StopHere],
                vec![Answer::AllEdits],
            ),
            (
                secret(),
                vec![
                    Answer::Always,
                    Answer::AllEdits,
                    Answer::KeepGoing,
                    Answer::StopHere,
                ],
                vec![Answer::Once],
            ),
            (
                keep_going(),
                vec![Answer::Once, Answer::Always, Answer::AllEdits],
                vec![
                    Answer::KeepGoing,
                    Answer::StopHere,
                    Answer::Message { text: "x".into() },
                ],
            ),
        ];
        for (card, refused, accepted) in cases {
            for good in accepted {
                let a = Approvals::new();
                let (_, emit) = recorder();
                let cancel = CancellationToken::new();
                let (got, _) = tokio::join!(a.ask("c", card.clone(), &emit, &cancel), async {
                    let id = waiting_id(&a, "c").await;
                    for bad in &refused {
                        let e = a.answer("c", &id, bad.clone()).unwrap_err();
                        assert_eq!(e.message, NOT_WAITING, "{card:?} {bad:?}");
                        assert_eq!(a.waiting("c").unwrap().id(), id);
                    }
                    a.answer("c", &id, good.clone()).unwrap();
                });
                assert_eq!(got, Ok(good), "{card:?}");
            }
        }
    }

    #[tokio::test]
    async fn cancel_while_waiting_stops_and_a_late_answer_is_refused() {
        let a = Approvals::new();
        let (events, emit) = recorder();
        let cancel = CancellationToken::new();
        let (got, id) = tokio::join!(a.ask("c", secret(), &emit, &cancel), async {
            let id = waiting_id(&a, "c").await;
            cancel.cancel();
            id
        });
        assert_eq!(got, Err(Unanswered::Stopped));
        assert!(a.waiting("c").is_none());
        assert_eq!(
            a.answer("c", &id, Answer::Once).unwrap_err().message,
            NOT_WAITING
        );
        assert!(
            events
                .lock()
                .unwrap()
                .contains(&EngineEvent::ApprovalCleared { id })
        );
    }

    #[tokio::test]
    async fn a_cancel_racing_an_answer_wins() {
        let a = Approvals::new();
        let (_, emit) = recorder();
        let cancel = CancellationToken::new();
        let (got, _) = tokio::join!(a.ask("c", secret(), &emit, &cancel), async {
            let id = waiting_id(&a, "c").await;
            a.answer("c", &id, Answer::Once).unwrap();
            cancel.cancel();
        });
        assert_eq!(got, Err(Unanswered::Stopped));
    }

    #[tokio::test]
    async fn an_already_cancelled_ask_shows_nothing() {
        let a = Approvals::new();
        let (events, emit) = recorder();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            a.ask("c", secret(), &emit, &cancel).await,
            Err(Unanswered::Stopped)
        );
        assert!(events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_second_card_in_the_same_chat_is_refused_without_touching_the_first() {
        let a = Approvals::new();
        let (events, emit) = recorder();
        let cancel = CancellationToken::new();
        let (first, _) = tokio::join!(a.ask("c", secret(), &emit, &cancel), async {
            let id = waiting_id(&a, "c").await;
            assert_eq!(
                a.ask("c", command(None), &emit, &cancel).await,
                Err(Unanswered::Busy)
            );
            // Still the first card, still answerable.
            assert_eq!(a.waiting("c").unwrap().id(), id);
            a.answer("c", &id, Answer::Once).unwrap();
        });
        assert_eq!(first, Ok(Answer::Once));
        let shown = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, EngineEvent::ApprovalNeeded { .. }))
            .count();
        assert_eq!(shown, 1);
    }

    #[tokio::test]
    async fn chats_wait_independently() {
        let a = Approvals::new();
        let (_, emit) = recorder();
        let cancel = CancellationToken::new();
        let (x, _) = tokio::join!(a.ask("A", secret(), &emit, &cancel), async {
            let ida = waiting_id(&a, "A").await;
            // B asks and is answered while A is still waiting.
            let (b, _) = tokio::join!(a.ask("B", edit(), &emit, &cancel), async {
                let idb = waiting_id(&a, "B").await;
                a.answer("B", &idb, Answer::AllEdits).unwrap();
            });
            assert_eq!(b, Ok(Answer::AllEdits));
            assert_eq!(a.waiting("A").unwrap().id(), ida);
            a.answer("A", &ida, Answer::Once).unwrap();
        });
        assert_eq!(x, Ok(Answer::Once));
    }

    #[tokio::test]
    async fn dropping_the_chat_stops_the_waiting_card() {
        let a = Approvals::new();
        let (_, emit) = recorder();
        let cancel = CancellationToken::new();
        let (got, _) = tokio::join!(a.ask("c", secret(), &emit, &cancel), async {
            waiting_id(&a, "c").await;
            a.drop_chat("c");
        });
        assert_eq!(got, Err(Unanswered::Stopped));
        assert!(a.waiting("c").is_none());
    }

    #[tokio::test]
    async fn a_dropped_wait_clears_the_card() {
        let a = Approvals::new();
        let (events, emit) = recorder();
        let cancel = CancellationToken::new();
        let r = tokio::time::timeout(
            Duration::from_millis(50),
            a.ask("c", secret(), &emit, &cancel),
        )
        .await;
        assert!(r.is_err());
        assert!(a.waiting("c").is_none());
        assert!(matches!(
            events.lock().unwrap().last(),
            Some(EngineEvent::ApprovalCleared { .. })
        ));
    }

    #[tokio::test]
    async fn notes_and_messages_are_cut_at_4000_characters() {
        for answer in [
            Answer::Decline {
                note: Some("n".repeat(5_000)),
            },
            Answer::Message {
                text: "m".repeat(5_000),
            },
        ] {
            let a = Approvals::new();
            let (_, emit) = recorder();
            let cancel = CancellationToken::new();
            let (got, _) = tokio::join!(a.ask("c", secret(), &emit, &cancel), async {
                let id = waiting_id(&a, "c").await;
                a.answer("c", &id, answer.clone()).unwrap();
            });
            let len = match got.unwrap() {
                Answer::Decline { note: Some(n) } => n.chars().count(),
                Answer::Message { text } => text.chars().count(),
                other => panic!("{other:?}"),
            };
            assert_eq!(len, 4_000);
        }
    }

    #[test]
    fn cards_and_answers_serialise_with_their_tags() {
        use serde_json::json;
        assert_eq!(
            serde_json::to_value(command(Some("npm test"))).unwrap(),
            json!({"kind":"command","id":"","command":"npm test","folder":"/p","rule":"npm test",
                   "outside":false,"first_in_project":false,"background":false})
        );
        assert_eq!(
            serde_json::to_value(edit()).unwrap(),
            json!({"kind":"edit","id":"","path":"a.txt","folder":"/p","created":false,"removed":[],"added":[],
                   "removed_total":0,"added_total":0,"total_lines":1})
        );
        assert_eq!(
            serde_json::to_value(keep_going()).unwrap(),
            json!({"kind":"keep_going","id":"","requests":25,"free_left":null,"spent_today_usd":null})
        );
        for (a, j) in [
            (Answer::Once, json!({"decision":"once"})),
            (Answer::AllEdits, json!({"decision":"all_edits"})),
            (
                Answer::Decline { note: None },
                json!({"decision":"decline","note":null}),
            ),
            (
                Answer::Message { text: "t".into() },
                json!({"decision":"message","text":"t"}),
            ),
            (Answer::StopHere, json!({"decision":"stop_here"})),
        ] {
            assert_eq!(serde_json::to_value(&a).unwrap(), j);
            assert_eq!(serde_json::from_value::<Answer>(j).unwrap(), a);
        }
    }
}
