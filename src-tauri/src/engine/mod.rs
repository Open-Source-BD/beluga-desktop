//! The single boundary between the app and whatever runs a chat turn.

mod direct;
#[cfg(test)]
pub mod scripted;

pub use direct::DirectEngine;

use crate::agent::plan::PlanItem;
use crate::agent::{Approval, ToolCall, ToolStatus};
use crate::openrouter::error::ApiError;
use crate::openrouter::stream::WireMessage;
use async_trait::async_trait;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiError {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
}

impl UiError {
    fn new(kind: &str, message: &str, retryable: bool) -> Self {
        Self {
            kind: kind.to_string(),
            message: message.to_string(),
            retryable,
        }
    }

    /// Produced when the keychain is missing or refuses to save the key.
    pub fn keychain_unavailable() -> Self {
        Self::new(
            "keychain_unavailable",
            "Your key could not be saved in the keychain.",
            false,
        )
    }

    /// A local failure (store, file, or app state), shown with a fixed message.
    pub(crate) fn other(message: &str) -> Self {
        Self::new("other", message, false)
    }

    /// Signing out could not delete the stored key.
    pub(crate) fn key_not_removed() -> Self {
        Self::new(
            "other",
            "Could not remove your key from this computer. Try signing out again.",
            true,
        )
    }

    /// The saved key could not be read at the start of a turn.
    pub(crate) fn key_unreadable() -> Self {
        Self::new(
            "other",
            "OpenRouter returned an error: could not read the saved key",
            false,
        )
    }
}

impl From<ApiError> for UiError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Unauthorized => Self::new(
                "unauthorized",
                "OpenRouter no longer accepts this key. Sign in again to continue.",
                false,
            ),
            ApiError::RateLimited { .. } => Self::new(
                "rate_limited",
                "OpenRouter is limiting requests right now. Wait a moment, then try again.",
                true,
            ),
            ApiError::NoCredit => Self::new(
                "no_credit",
                "This model needs credit and your OpenRouter balance is empty. Pick a free model or add credit.",
                false,
            ),
            ApiError::ModelUnavailable => Self::new(
                "model_unavailable",
                "This model is not available right now. Pick another model and try again.",
                true,
            ),
            ApiError::ContextTooLong => Self::new(
                "context_too_long",
                "This chat is too long for the model. Start a new chat or pick a model with a larger limit.",
                false,
            ),
            ApiError::Network(_) => Self::new(
                "network",
                "Could not reach OpenRouter. Check your connection and try again.",
                true,
            ),
            ApiError::Other { message, .. } => Self::new(
                "other",
                &format!("OpenRouter returned an error: {message}"),
                false,
            ),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    TextDelta {
        text: String,
    },
    Usage {
        requests: u32,
        cost_usd: f64,
    },
    Done,
    Interrupted {
        reason: String,
    },
    Failed {
        error: UiError,
    },
    ToolStarted {
        call_id: String,
        name: String,
        summary: String,
    },
    ToolOutput {
        call_id: String,
        text: String,
    },
    ToolFinished {
        call_id: String,
        status: ToolStatus,
        summary: String,
        detail: Option<String>,
    },
    ApprovalNeeded {
        approval: Approval,
    },
    ApprovalCleared {
        id: String,
    },
    PlanUpdated {
        items: Vec<PlanItem>,
    },
    /// A rate limit is being waited out.
    Waiting {
        seconds: u64,
    },
    Paused {
        reason: String,
    },
    BackgroundChanged,
    MessageStored,
}

/// One request to the model, with tools if the caller offers any.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRequest {
    pub model_id: String,
    pub messages: Vec<WireMessage>,
    pub tools: Option<serde_json::Value>,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepError {
    pub ui: UiError,
    /// What the server's `Retry-After` asked for, in seconds.
    pub retry_after_secs: Option<u64>,
}

/// What one request produced. When `error` is set, `text` is whatever arrived
/// first. A cancelled step has `error: None`; the caller checks the token.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepOutcome {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub cost_usd: Option<f64>,
    pub prompt_tokens: Option<u32>,
    pub error: Option<StepError>,
    /// The reply held too many calls or too much argument text, so it
    /// returned no calls; the caller should say so to the model.
    pub overflowed: bool,
    /// The request was dispatched to the server. False when it never left
    /// the app (no key, or the key could not be read); only a sent request
    /// counts toward usage.
    pub sent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnRequest {
    pub model_id: String,
    pub messages: Vec<WireMessage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnOutcome {
    pub text: String,
    /// HTTP requests actually sent to chat completions during this turn.
    pub requests: u32,
    pub cost_usd: f64,
    pub interrupted: bool,
}

#[async_trait]
pub trait Engine: Send + Sync {
    /// One streamed request. Emits only `TextDelta`, never `Done` or `Failed`;
    /// the caller decides how the task ends.
    async fn run_step(
        &self,
        request: StepRequest,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: CancellationToken,
    ) -> StepOutcome;

    /// A whole plain chat turn: one step without tools, then the terminal
    /// event.
    async fn run_turn(
        &self,
        req: TurnRequest,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: CancellationToken,
    ) -> TurnOutcome {
        let step = StepRequest {
            model_id: req.model_id,
            messages: req.messages,
            tools: None,
            session_id: String::new(),
        };
        let out = self.run_step(step, emit, cancel.clone()).await;
        finish_turn(out, 1, cancel.is_cancelled(), emit)
    }
}

/// Turn a finished step into a turn's outcome and its terminal event.
pub(crate) fn finish_turn(
    step: StepOutcome,
    requests: u32,
    cancelled: bool,
    emit: &(dyn Fn(EngineEvent) + Send + Sync),
) -> TurnOutcome {
    let mut out = TurnOutcome {
        text: step.text,
        requests,
        cost_usd: step.cost_usd.unwrap_or(0.0),
        interrupted: false,
    };
    if let Some(cost_usd) = step.cost_usd {
        emit(EngineEvent::Usage { requests, cost_usd });
    }
    match step.error {
        _ if cancelled => {
            out.interrupted = true;
            emit(EngineEvent::Interrupted {
                reason: "stopped".to_string(),
            });
        }
        Some(e) if out.text.is_empty() => emit(EngineEvent::Failed { error: e.ui }),
        Some(e) => {
            out.interrupted = true;
            emit(EngineEvent::Interrupted { reason: e.ui.kind });
        }
        None => emit(EngineEvent::Done),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_json_shape() {
        let v = serde_json::to_value(EngineEvent::TextDelta { text: "a".into() }).unwrap();
        assert_eq!(v, serde_json::json!({"type":"text_delta","text":"a"}));
    }

    #[test]
    fn keychain_unavailable_fields() {
        let e = UiError::keychain_unavailable();
        assert_eq!(e.kind, "keychain_unavailable");
        assert_eq!(e.message, "Your key could not be saved in the keychain.");
        assert!(!e.retryable);
    }

    #[test]
    fn new_events_serialise_with_their_tags_and_fields() {
        use crate::agent::ToolStatus;
        use serde_json::json;
        let cases: Vec<(EngineEvent, serde_json::Value)> = vec![
            (
                EngineEvent::ToolStarted {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    summary: "Reading a.txt".into(),
                },
                json!({"type":"tool_started","call_id":"c1","name":"read_file","summary":"Reading a.txt"}),
            ),
            (
                EngineEvent::ToolOutput {
                    call_id: "c1".into(),
                    text: "hi".into(),
                },
                json!({"type":"tool_output","call_id":"c1","text":"hi"}),
            ),
            (
                EngineEvent::ToolFinished {
                    call_id: "c1".into(),
                    status: ToolStatus::NotRun,
                    summary: "Read a.txt".into(),
                    detail: Some("d".into()),
                },
                json!({"type":"tool_finished","call_id":"c1","status":"not_run","summary":"Read a.txt","detail":"d"}),
            ),
            (
                EngineEvent::ApprovalNeeded {
                    approval: crate::agent::Approval::Secret {
                        id: "a1".into(),
                        path: ".env".into(),
                    },
                },
                json!({"type":"approval_needed","approval":{"kind":"secret","id":"a1","path":".env"}}),
            ),
            (
                EngineEvent::ApprovalCleared { id: "a1".into() },
                json!({"type":"approval_cleared","id":"a1"}),
            ),
            (
                EngineEvent::PlanUpdated {
                    items: vec![crate::agent::plan::PlanItem {
                        text: "x".into(),
                        state: crate::agent::plan::PlanState::InProgress,
                    }],
                },
                json!({"type":"plan_updated","items":[{"text":"x","state":"in_progress"}]}),
            ),
            (
                EngineEvent::Waiting { seconds: 12 },
                json!({"type":"waiting","seconds":12}),
            ),
            (
                EngineEvent::Paused {
                    reason: "daily_limit".into(),
                },
                json!({"type":"paused","reason":"daily_limit"}),
            ),
            (
                EngineEvent::BackgroundChanged,
                json!({"type":"background_changed"}),
            ),
            (EngineEvent::MessageStored, json!({"type":"message_stored"})),
        ];
        for (event, expected) in cases {
            assert_eq!(serde_json::to_value(&event).unwrap(), expected, "{event:?}");
        }
    }
}
