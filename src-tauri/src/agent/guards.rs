//! The limits around a project task: the "Keep going?" pause every 25
//! requests, the free allowance, the daily spending limit for paid models
//! (which every paid request the app makes obeys), waiting out a rate
//! limit, going in circles, and trimming old tool output to fit the context.
//!
//! A pause is not a failure: everything stored stays stored, the task ends
//! `paused` with its reason, and the next send continues it.
//!
//! Money is never taken from the model or the interface: spend is what
//! `Store::record_usage` stored from the provider's reported cost, summed
//! per UTC day.

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::approval::Unanswered;
use super::dispatch::TOOL_NAMES;
use super::{Answer, Approval, ToolCall, ToolOutcome, ToolStatus};
use crate::commands::chat_state::{percent_of_context, wire_chars};
use crate::commands::chats::local;
use crate::engine::{EngineEvent, StepError, UiError};
use crate::openrouter::key_info::KeyStatus;
use crate::openrouter::models::{ModelCache, ModelInfo};
use crate::openrouter::stream::{Part, Role, WireMessage};
use crate::openrouter::tool_calls::parse_arguments;
use crate::state::AppState;
use crate::store::Store;

/// Settings key of the daily limit: a decimal number of dollars, or `none`.
pub const LIMIT_SETTING: &str = "daily_limit_usd";
pub const DEFAULT_LIMIT_USD: f64 = 1.00;
pub const MIN_LIMIT_USD: f64 = 0.10;
/// Amounts this close are equal.
const MONEY_TOLERANCE: f64 = 0.000_001;

/// Requests between two "Keep going?" cards.
pub const KEEP_GOING_EVERY: u32 = 25;
/// Requests between two reads of the key's free allowance.
const KEY_CHECK_EVERY: u32 = 10;
/// Rate-limit waits per task, the longest wait, and the wait when the
/// server names none, in seconds.
pub const MAX_RATE_WAITS: u32 = 3;
pub const MAX_WAIT_SECS: u64 = 60;
pub const DEFAULT_WAIT_SECS: u64 = 10;
/// The same failing call this many times in a row is going in circles…
pub const SAME_FAILURES: u32 = 3;
/// …and so is this many calls in a row that cannot be understood.
pub const UNREADABLE_CALLS: u32 = 5;
/// Tool output is trimmed from what is sent above this share of the context.
pub const TRIM_ABOVE_PERCENT: f64 = 70.0;
/// The newest results are never trimmed.
const KEEP_RESULTS: usize = 4;
/// What a trimmed result says instead.
pub const TRIMMED: &str = "[output removed to save space]";

// Pause reasons (`TurnResult.reason`).
pub const KEEP_GOING_DECLINED: &str = "keep_going_declined";
pub const NO_FREE_REQUESTS: &str = "no_free_requests";
pub const DAILY_LIMIT: &str = "daily_limit";
pub const CHAT_FULL: &str = "chat_full";
pub const GOING_IN_CIRCLES: &str = "going_in_circles";
pub const RATE_LIMITED: &str = "rate_limited";

pub const STRUGGLING: &str =
    "This model is struggling with this task. A stronger model usually does better.";
pub const TOO_FULL: &str =
    "This chat is too full to continue. Use /compact to shorten it, then say continue.";
pub const NO_FREE_NOTE: &str =
    "Paused: no free requests are left today. They reset at midnight UTC.";
pub const RATE_LIMITED_NOTE: &str =
    "Paused: the model is limiting requests. Try again in a minute.";
const LIMIT_TOO_LOW: &str = "The limit must be at least $0.10.";

// ---------------------------------------------------------------------------
// Spending
// ---------------------------------------------------------------------------

fn acceptable_limit(usd: f64) -> bool {
    usd.is_finite() && usd >= MIN_LIMIT_USD - MONEY_TOLERANCE
}

/// Today's spending limit for paid models: `None` is no limit. A setting the
/// app did not write (unreadable, below the minimum) counts as the default.
pub fn spend_limit(store: &Store) -> Option<f64> {
    let Ok(Some(value)) = store.setting(LIMIT_SETTING) else {
        return Some(DEFAULT_LIMIT_USD);
    };
    let value = value.trim();
    if value == "none" {
        return None;
    }
    match value.parse::<f64>() {
        Ok(usd) if acceptable_limit(usd) => Some(usd),
        _ => Some(DEFAULT_LIMIT_USD),
    }
}

/// Sets the limit (`None`: no limit). Anything under $0.10, and anything
/// that is not a number, is refused.
pub fn set_spend_limit(store: &Store, limit: Option<f64>) -> Result<(), UiError> {
    let value = match limit {
        None => "none".to_string(),
        Some(usd) if acceptable_limit(usd) => usd.to_string(),
        Some(_) => return Err(UiError::other(LIMIT_TOO_LOW)),
    };
    store.set_setting(LIMIT_SETTING, &value).map_err(local)
}

/// The UTC day as the store keys daily usage, `YYYY-MM-DD`.
pub fn today() -> String {
    let d = time::OffsetDateTime::now_utc().date();
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// What requests cost today, in all chats, as recorded; `None` when the
/// store cannot be read (callers that guard spending treat that as "limit
/// reached", never as nothing spent).
pub fn spent_today(store: &Store) -> Option<f64> {
    store.day_usage(&today()).ok().map(|(_, cost)| cost)
}

/// The cached model list's entry for the model.
pub fn model_info(s: &AppState, model_id: &str) -> Option<ModelInfo> {
    ModelCache::load(&s.model_cache_file())?
        .into_iter()
        .find(|m| m.id == model_id)
}

/// An estimate of what a request cost, used only when the provider reports
/// no cost for a request on a paid model (so it is never recorded as $0): the
/// prompt tokens the provider reported (else the request's characters / 4) at
/// the model's prompt price, plus the reply's characters / 4 tokens at its
/// completion price. A free model, and a model missing from the model list
/// (its prices are unknown), estimate nothing.
pub fn estimate_cost(
    model: Option<&ModelInfo>,
    prompt_tokens: Option<u32>,
    request_chars: usize,
    reply_chars: usize,
) -> f64 {
    let Some(model) = model.filter(|m| !m.is_free) else {
        return 0.0;
    };
    let prompt = prompt_tokens.map_or(request_chars as f64 / 4.0, f64::from);
    let reply = reply_chars as f64 / 4.0;
    (prompt * model.prompt_usd_per_mtok + reply * model.completion_usd_per_mtok) / 1_000_000.0
}

/// A model missing from the model list may cost money, so it counts as paid.
pub fn is_paid(model: Option<&ModelInfo>) -> bool {
    model.is_none_or(|m| !m.is_free)
}

fn dollars(usd: f64) -> String {
    format!("${usd:.2}")
}

/// The refusal of a request on a paid model once today's spend has reached
/// the limit; `None` lets it through. A free model is never refused.
pub fn over_limit(store: &Store, model: Option<&ModelInfo>) -> Option<UiError> {
    if !is_paid(model) {
        return None;
    }
    let limit = spend_limit(store)?;
    // A spend that cannot be read fails closed: the limit counts as reached.
    spent_today(store)
        .is_none_or(|spent| spent >= limit - MONEY_TOLERANCE)
        .then(|| {
            UiError::daily_limit(&format!(
                "You have reached today's spending limit of {}. You can change it in Settings.",
                dollars(limit)
            ))
        })
}

/// `over_limit` for a model named by id.
pub fn over_limit_for(s: &AppState, model_id: &str) -> Option<UiError> {
    over_limit(&s.store, model_info(s, model_id).as_ref())
}

/// The note stored when a task pauses for `reason`, if it has one.
pub fn pause_note(reason: &str, store: &Store) -> Option<String> {
    Some(match reason {
        DAILY_LIMIT => format!(
            "Paused: today's spending limit of {} is reached. You can change it in Settings.",
            dollars(spend_limit(store).unwrap_or(DEFAULT_LIMIT_USD))
        ),
        NO_FREE_REQUESTS => NO_FREE_NOTE.to_string(),
        RATE_LIMITED => RATE_LIMITED_NOTE.to_string(),
        GOING_IN_CIRCLES => STRUGGLING.to_string(),
        CHAT_FULL => TOO_FULL.to_string(),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Going in circles
// ---------------------------------------------------------------------------

/// Watches the task's calls for the same failure again and again, or for
/// calls that cannot be understood.
#[derive(Debug, Default)]
pub struct Circles {
    /// The last failing call: name, arguments as parsed JSON text, and for a
    /// command its output too.
    last_failure: Option<(String, String, Option<String>)>,
    same_failures: u32,
    unreadable: u32,
}

/// A call naming no tool, or whose arguments cannot be read.
fn unreadable(call: &ToolCall) -> bool {
    !TOOL_NAMES.contains(&call.name.as_str()) || parse_arguments(&call.arguments).is_err()
}

impl Circles {
    /// Records one dispatched call; `true` when the task is going in
    /// circles. Only an `Error` is a failure: a declined call or one not run
    /// is the user's or the app's doing, not the model's. A command failing
    /// with different output is the model working (fixing a test, say), so
    /// a command counts only when its output repeats too.
    pub fn note(&mut self, call: &ToolCall, outcome: &ToolOutcome) -> bool {
        if unreadable(call) {
            self.last_failure = None;
            self.same_failures = 0;
            self.unreadable += 1;
            return self.unreadable >= UNREADABLE_CALLS;
        }
        self.unreadable = 0;
        if outcome.status != ToolStatus::Error {
            self.last_failure = None;
            self.same_failures = 0;
            return false;
        }
        let arguments = parse_arguments(&call.arguments)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let output = (call.name == "run_command").then(|| outcome.text.clone());
        let failure = (call.name.clone(), arguments, output);
        if self.last_failure.as_ref() == Some(&failure) {
            self.same_failures += 1;
        } else {
            self.last_failure = Some(failure);
            self.same_failures = 1;
        }
        self.same_failures >= SAME_FAILURES
    }

    /// A reply whose calls were too many or too large counts as one
    /// unreadable call.
    pub fn overflowed(&mut self) -> bool {
        self.unreadable += 1;
        self.unreadable >= UNREADABLE_CALLS
    }
}

// ---------------------------------------------------------------------------
// Trimming
// ---------------------------------------------------------------------------

fn replace_content(m: &mut WireMessage) {
    m.parts = vec![Part::Text(TRIMMED.to_string())];
}

fn is_trimmed(m: &WireMessage) -> bool {
    matches!(&m.parts[..], [Part::Text(t)] if t == TRIMMED)
}

/// Fits the request in 70% of a context of `context_length` tokens, counting
/// `fixed_chars` (the tool definitions) and every message, at four
/// characters a token as `chat_fullness` does. Results trimmed by earlier
/// requests of the task (`trimmed`, by call id) are trimmed again, so the
/// start of the request stays the same bytes; then, while over, the content
/// of the oldest tool message not yet trimmed is replaced by `TRIMMED`. The
/// last four tool messages and every other role are never touched, and no
/// message is removed. `false`: still over. A context length of 0
/// (unknown) skips all of it.
pub fn trim(
    messages: &mut [WireMessage],
    context_length: u64,
    fixed_chars: usize,
    trimmed: &mut HashSet<String>,
) -> bool {
    if context_length == 0 {
        return true;
    }
    for m in messages.iter_mut() {
        if m.role == Role::Tool
            && m.tool_call_id
                .as_ref()
                .is_some_and(|id| trimmed.contains(id))
        {
            replace_content(m);
        }
    }
    let mut total = fixed_chars + messages.iter().map(wire_chars).sum::<usize>();
    let over = |total: usize| percent_of_context(total, context_length) > TRIM_ABOVE_PERCENT;
    let results: Vec<usize> = (0..messages.len())
        .filter(|&i| messages[i].role == Role::Tool)
        .collect();
    let trimmable = &results[..results.len().saturating_sub(KEEP_RESULTS)];
    for &i in trimmable {
        if !over(total) {
            break;
        }
        let m = &mut messages[i];
        if is_trimmed(m) {
            continue;
        }
        let before = wire_chars(m);
        replace_content(m);
        total = total - before + wire_chars(m);
        if let Some(id) = &m.tool_call_id {
            trimmed.insert(id.clone());
        }
    }
    !over(total)
}

// ---------------------------------------------------------------------------
// The task's guards
// ---------------------------------------------------------------------------

/// What the guards need from outside the app: the key's status from the
/// server, and the clock. Tests script both.
#[async_trait]
pub trait Outside: Send + Sync {
    /// The key's status; `None` when it cannot be had (never blocks a task).
    async fn key_status(&self, s: &AppState) -> Option<KeyStatus>;
    async fn wait(&self, seconds: u64);
}

/// OpenRouter's `GET /key` and the real clock.
pub struct Server;

#[async_trait]
impl Outside for Server {
    async fn key_status(&self, s: &AppState) -> Option<KeyStatus> {
        crate::commands::auth::key_status(s).await.ok()
    }

    async fn wait(&self, seconds: u64) {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }
}

/// The task the guards act for.
pub struct Env<'a> {
    pub s: &'a AppState,
    pub chat_id: &'a str,
    pub emit: &'a (dyn Fn(EngineEvent) + Send + Sync),
    pub cancel: CancellationToken,
}

/// What a guard decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Continue,
    /// Stop and wait for the user; the reason is a `TurnResult` reason.
    Pause(&'static str),
}

/// The answer to "Keep going?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepGoing {
    Continue,
    /// Keep going with this as the user's next instruction.
    Message(String),
    Pause(&'static str),
    /// Stop, the chat deleted, or the folder changed.
    Stopped,
}

/// What to do after a failed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Not a rate limit: the error ends the task as before.
    No,
    /// Waited out: send the same request again.
    Again,
    Pause(&'static str),
    /// Cancelled during the wait.
    Stopped,
}

/// The guards' state for one task.
#[derive(Debug, Default)]
pub struct Guards {
    model: Option<ModelInfo>,
    /// Characters of the request outside the messages (the tool definitions).
    fixed_chars: usize,
    circles: Circles,
    /// Call ids whose results are trimmed from every request from now on.
    trimmed: HashSet<String>,
    rate_waits: u32,
    /// Requests sent since the task started or the last "Keep going?".
    since_asked: u32,
    key: Option<KeyStatus>,
    /// The request count when the key status was last read.
    key_read_at: Option<u32>,
    /// Read the key status before the next request.
    key_due: bool,
}

impl Guards {
    pub fn new(model: Option<ModelInfo>, fixed_chars: usize) -> Self {
        Self {
            model,
            fixed_chars,
            ..Self::default()
        }
    }

    fn paid(&self) -> bool {
        is_paid(self.model.as_ref())
    }

    /// The cost to record for a request on this task's model that reported
    /// none: `estimate_cost` over the request (`message_chars` plus the tool
    /// definitions) and the reply. A reply with nothing in it (a request that
    /// failed before any answer) is not estimated.
    pub fn estimate(
        &self,
        message_chars: usize,
        prompt_tokens: Option<u32>,
        reply_chars: usize,
    ) -> f64 {
        if reply_chars == 0 {
            return 0.0;
        }
        estimate_cost(
            self.model.as_ref(),
            prompt_tokens,
            message_chars + self.fixed_chars,
            reply_chars,
        )
    }

    /// One request was sent.
    pub fn counted(&mut self) {
        self.since_asked += 1;
    }

    /// 25 requests since the start or the last answer: ask before the next.
    pub fn keep_going_due(&self) -> bool {
        self.since_asked >= KEEP_GOING_EVERY
    }

    fn free_left(&self) -> Option<u32> {
        Some(self.key.as_ref()?.free_daily?.remaining)
    }

    async fn read_key(&mut self, env: &Env<'_>, requests: u32) {
        self.key = env.s.outside.key_status(env.s).await;
        self.key_read_at = Some(requests);
        self.key_due = false;
    }

    /// Asks "Keep going?" with the task's request count and, for a free
    /// model, the free requests left, for a paid one today's spend.
    pub async fn keep_going(&mut self, env: &Env<'_>, requests: u32) -> KeepGoing {
        let paid = self.paid();
        let card = Approval::KeepGoing {
            id: String::new(),
            requests,
            free_left: if paid { None } else { self.free_left() },
            spent_today_usd: paid.then(|| spent_today(&env.s.store)).flatten(),
        };
        let answer = env
            .s
            .approvals
            .ask(env.chat_id, card, env.emit, &env.cancel)
            .await;
        self.since_asked = 0;
        match answer {
            Ok(Answer::KeepGoing) => KeepGoing::Continue,
            Ok(Answer::Message { text }) => KeepGoing::Message(text),
            Ok(_) => KeepGoing::Pause(KEEP_GOING_DECLINED),
            Err(Unanswered::Stopped) => KeepGoing::Stopped,
            // Another card waits in the chat (it cannot, between requests):
            // the task does not go on unasked.
            Err(Unanswered::Busy) => KeepGoing::Pause(KEEP_GOING_DECLINED),
        }
    }

    /// Before each request (`requests` sent so far): the daily limit on a
    /// paid model; on a free one the free allowance, read from the server
    /// before the first request, every 10 requests, and after a request was
    /// refused as rate-limited or out of credit; then trimming
    /// `messages` to fit the context.
    pub async fn before_request(
        &mut self,
        env: &Env<'_>,
        requests: u32,
        messages: &mut [WireMessage],
    ) -> Next {
        if self.paid() {
            if over_limit(&env.s.store, self.model.as_ref()).is_some() {
                return Next::Pause(DAILY_LIMIT);
            }
        } else {
            let due = self.key_due
                || self
                    .key_read_at
                    .is_none_or(|at| requests >= at + KEY_CHECK_EVERY);
            if due {
                self.read_key(env, requests).await;
            }
            // A count of 0 is always one just read (a task pauses on it), so
            // it is never a stale one.
            if self.free_left() == Some(0) {
                return Next::Pause(NO_FREE_REQUESTS);
            }
        }
        let context_length = self.model.as_ref().map_or(0, |m| m.context_length);
        if !trim(
            messages,
            context_length,
            self.fixed_chars,
            &mut self.trimmed,
        ) {
            return Next::Pause(CHAT_FULL);
        }
        Next::Continue
    }

    /// After each dispatched call: going in circles.
    pub fn after_call(&mut self, call: &ToolCall, outcome: &ToolOutcome) -> Next {
        if self.circles.note(call, outcome) {
            Next::Pause(GOING_IN_CIRCLES)
        } else {
            Next::Continue
        }
    }

    /// A reply overflowed: it counts as one unreadable call.
    pub fn overflowed(&mut self) -> Next {
        if self.circles.overflowed() {
            Next::Pause(GOING_IN_CIRCLES)
        } else {
            Next::Continue
        }
    }

    /// A request failed. A rate limit is waited out (what the server asked
    /// for, at most 60 seconds, 10 when it named nothing), up to three
    /// times a task; the fourth pauses. On a free model the key status is
    /// read first, and no free requests left pauses at once. A rate limit or
    /// an empty or low balance also has the key status read again before the next
    /// request.
    pub async fn retry_after(&mut self, env: &Env<'_>, requests: u32, error: &StepError) -> Retry {
        if matches!(
            error.ui.kind.as_str(),
            "rate_limited" | "no_credit" | "low_credit"
        ) {
            self.key_due = true;
        }
        if error.ui.kind != "rate_limited" {
            return Retry::No;
        }
        // On a free model the limit may be the day's free allowance running
        // out, which no wait cures: read it before waiting.
        if !self.paid() {
            self.read_key(env, requests).await;
            if self.free_left() == Some(0) {
                return Retry::Pause(NO_FREE_REQUESTS);
            }
        }
        if self.rate_waits >= MAX_RATE_WAITS {
            return Retry::Pause(RATE_LIMITED);
        }
        self.rate_waits += 1;
        let seconds = error
            .retry_after_secs
            .unwrap_or(DEFAULT_WAIT_SECS)
            .min(MAX_WAIT_SECS);
        (env.emit)(EngineEvent::Waiting { seconds });
        tokio::select! {
            biased;
            () = env.cancel.cancelled() => Retry::Stopped,
            () = env.s.outside.wait(seconds) => {
                if env.cancel.is_cancelled() { Retry::Stopped } else { Retry::Again }
            }
        }
    }
}

#[cfg(test)]
#[path = "guards_tests.rs"]
mod tests;
