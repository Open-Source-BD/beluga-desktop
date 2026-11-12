//! Shortening a chat: one request summarises the conversation, and from then
//! on the summary is sent in place of the summarised messages.

use super::chat_state::{CHARS_PER_TOKEN, message_chars, require_chat, sized_model};
use super::chats::local;
use super::turn::{
    Register, TurnResult, TurnStatus, plain_history, preamble, register_turn, run_registered,
    system,
};
use crate::engine::{TurnOutcome, TurnRequest, UiError};
use crate::state::AppState;
use crate::store::StoredMessage;

const SUMMARY_ASK: &str = "Summarise the conversation below so it can be continued later. Keep decisions, file names, code that matters and open questions. Be brief.";
const LEFT_OUT: &str = "The oldest messages were too long to include in the summary.\n\n";
const TURN_RUNNING: &str = "Wait for the reply to finish, or stop it, before shortening the chat.";
const TOO_SHORT: &str = "This chat is too short to shorten.";
const SUMMARISED_NOTE: &str = "Summarised above this line";
const MIN_MESSAGES: usize = 4;
/// The conversation sent for summary may use this share of the context.
const FIT_PERCENT: u64 = 60;

/// What is summarised: the existing summary (if any) and the sendable
/// messages, oldest first. Images are not sent with the summary request.
struct Conversation {
    summary: Option<String>,
    messages: Vec<StoredMessage>,
    left_out: bool,
}

impl Conversation {
    fn chars(&self) -> usize {
        self.summary.as_deref().map_or(0, |t| t.chars().count())
            + self
                .messages
                .iter()
                .map(|m| message_chars(m, 0))
                .sum::<usize>()
    }

    /// Leaves out the oldest parts, the existing summary first, until the
    /// rest fits in `max_tokens`. The newest message is always kept.
    fn fit(&mut self, max_tokens: u64) {
        while (self.chars() / CHARS_PER_TOKEN) as u64 > max_tokens {
            if self.summary.take().is_none() {
                if self.messages.len() <= 1 {
                    break;
                }
                self.messages.remove(0);
            }
            self.left_out = true;
        }
    }
}

/// Asks `model_id` to summarise the chat in one request, without the skill
/// messages. Only a completed, non-empty summary changes the chat: it is
/// stored with the boundary just past the last summarised message and the
/// note "Summarised above this line" is added. Usage is recorded whenever a
/// request was made. Refused while a turn runs in the chat; runs as the
/// chat's turn, so `stop` cancels it.
pub async fn compact_chat(
    s: &AppState,
    chat_id: &str,
    model_id: &str,
    keep: Option<&str>,
) -> Result<TurnResult, UiError> {
    require_chat(s, chat_id)?;
    // A reply cancelled by the compaction would be stored after the boundary
    // with no question before it, so a running reply is never replaced here.
    // This early check only saves the store reads; `register_turn` decides.
    if s.turns().contains_key(chat_id) {
        return Err(UiError::other(TURN_RUNNING));
    }
    let mut messages = s.store.sendable_messages(chat_id).map_err(local)?;
    // The boundary falls right before a user row: never between a call and
    // its result, never inside a task. When the last reply still has calls
    // without results, its task (from the user row that started it) stays
    // after the boundary; with no such user row there is nothing to shorten.
    let boundary = match open_reply(&messages) {
        Some(open) => {
            let Some(at) = messages[..open]
                .iter()
                .rposition(|m| m.kind == "message" && m.role == "user")
            else {
                return Err(UiError::other(TOO_SHORT));
            };
            let seq = messages[at].seq;
            messages.truncate(at);
            seq
        }
        None => messages.last().map_or(0, |m| m.seq + 1),
    };
    if messages.len() < MIN_MESSAGES {
        return Err(UiError::other(TOO_SHORT));
    }
    let mut conversation = Conversation {
        summary: s
            .store
            .summary(chat_id)
            .map_err(local)?
            .map(|(text, _)| text),
        messages,
        left_out: false,
    };
    if let Some(model) = sized_model(s, model_id) {
        conversation.fit(model.context_length * FIT_PERCENT / 100);
    }

    let mut ask = SUMMARY_ASK.to_string();
    if let Some(keep) = keep.map(str::trim).filter(|k| !k.is_empty()) {
        ask.push_str(&format!("\n\nMake sure to keep: {keep}"));
    }
    let mut wire_messages = vec![system(ask)];
    wire_messages.extend(preamble(&[], conversation.summary.as_deref()));
    // The summary request has no tools, so tool rows go as plain text.
    wire_messages.extend(
        plain_history(&conversation.messages, false)
            .into_iter()
            .filter(|w| !w.parts.is_empty()),
    );
    let request = TurnRequest {
        model_id: model_id.to_string(),
        messages: wire_messages,
    };
    // Checking and registering together is what keeps a reply that started
    // since the check above from being cancelled by the compaction.
    let Some(registration) = register_turn(s, chat_id, Register::IfFree) else {
        return Err(UiError::other(TURN_RUNNING));
    };
    // No task stores rows while the summary is made; a send that replaces
    // this turn waits for it.
    let _held = s.task_locks.hold(chat_id).await;
    let ran = run_registered(s, chat_id, registration, request, &|_| {}).await;

    // A turn that started in the chat meanwhile cancelled this one; should the
    // summary have arrived anyway, it no longer describes the chat.
    let result = if ran.superseded && ran.result.status == TurnStatus::Done {
        TurnResult::interrupted("stopped".to_string())
    } else {
        ran.result
    };
    let text = ran.outcome.text.trim();
    let summary = (result.status == TurnStatus::Done
        && !ran.outcome.interrupted
        && !text.is_empty())
    .then(|| {
        if conversation.left_out {
            format!("{LEFT_OUT}{text}")
        } else {
            text.to_string()
        }
    });
    // The request was made, so a failure to store is reported as a failed
    // turn, not as a compaction that never ran.
    let stored = record_usage(s, chat_id, &ran.outcome).and_then(|()| match &summary {
        Some(summary) => store_summary(s, chat_id, summary, boundary),
        None => Ok(()),
    });
    Ok(match stored {
        Ok(()) => result,
        Err(e) => TurnResult::failed(e),
    })
}

/// The index of the last reply when it made calls that are not all
/// answered yet (a task that ended before storing every result).
fn open_reply(messages: &[StoredMessage]) -> Option<usize> {
    let at = messages.iter().rposition(|m| m.kind != "tool")?;
    let calls = &messages[at].tool_calls;
    let answered = |id: &str| {
        messages[at + 1..]
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some(id))
    };
    calls.iter().any(|c| !answered(&c.id)).then_some(at)
}

fn record_usage(s: &AppState, chat_id: &str, out: &TurnOutcome) -> Result<(), UiError> {
    if out.requests > 0 {
        s.store
            .record_usage(chat_id, out.requests, out.cost_usd)
            .map_err(local)?;
    }
    Ok(())
}

fn store_summary(s: &AppState, chat_id: &str, summary: &str, boundary: i64) -> Result<(), UiError> {
    s.store
        .set_summary(chat_id, summary, boundary)
        .map_err(local)?;
    s.store
        .append_note(chat_id, SUMMARISED_NOTE)
        .map_err(local)?;
    Ok(())
}
