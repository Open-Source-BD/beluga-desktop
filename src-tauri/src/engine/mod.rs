//! The single boundary between the app and whatever runs a chat turn.

mod direct;

pub use direct::DirectEngine;

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

    /// Produced when the app finds no keychain service on this computer.
    pub fn keychain_unavailable() -> Self {
        Self::new(
            "keychain_unavailable",
            "This computer has no keychain service.",
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
    TextDelta { text: String },
    Usage { requests: u32, cost_usd: f64 },
    Done,
    Interrupted { reason: String },
    Failed { error: UiError },
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
    async fn run_turn(
        &self,
        req: TurnRequest,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: CancellationToken,
    ) -> TurnOutcome;
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
        assert_eq!(e.message, "This computer has no keychain service.");
        assert!(!e.retryable);
    }
}
