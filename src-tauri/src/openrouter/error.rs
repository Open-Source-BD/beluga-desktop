const MAX_MESSAGE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Unauthorized,
    RateLimited {
        retry_after_secs: Option<u64>,
    },
    NoCredit,
    /// The balance cannot cover the model's full reply (OpenRouter reserves
    /// the whole output when no `max_tokens` is sent), though it is not empty.
    LowCredit,
    ModelUnavailable,
    ContextTooLong,
    Network(String),
    Other {
        status: u16,
        message: String,
    },
}

impl ApiError {
    /// Classify a non-success HTTP response. `Other.message` is a short,
    /// single-line string so a raw HTML or JSON body never travels onward.
    pub fn from_response(status: u16, body: &str) -> ApiError {
        let message = short_message(body);
        let lower = message.to_lowercase();
        match status {
            401 => ApiError::Unauthorized,
            402 if body.to_lowercase().contains("can only afford") => ApiError::LowCredit,
            402 => ApiError::NoCredit,
            429 => ApiError::RateLimited {
                retry_after_secs: None,
            },
            404 | 503 if lower.contains("no endpoints found") => ApiError::ModelUnavailable,
            400 if lower.contains("context length") => ApiError::ContextTooLong,
            _ => ApiError::Other { status, message },
        }
    }

    /// Add the wait the server asked for; only a rate limit has one.
    pub fn with_retry_after(self, header: Option<&str>) -> ApiError {
        match self {
            ApiError::RateLimited { .. } => ApiError::RateLimited {
                retry_after_secs: header.and_then(parse_retry_after),
            },
            other => other,
        }
    }
}

/// The seconds form of a `Retry-After` header. An HTTP date or anything else
/// that is not plain digits gives `None`.
pub fn parse_retry_after(value: &str) -> Option<u64> {
    let v = value.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse().ok()
}

/// Prefer the `error.message` of an OpenRouter JSON error; otherwise the raw
/// body. Whitespace is collapsed and the result capped.
fn short_message(body: &str) -> String {
    let text = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned());
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(MAX_MESSAGE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 402 from the live check: no `max_tokens` was sent, so OpenRouter
    /// reserved the model's whole output.
    const CANNOT_AFFORD_REPLY: &str = r#"{"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 131072 tokens, but can only afford 8450. To increase, visit https://openrouter.ai/workspaces/default/keys/[key-hash] and adjust the key's total limit","code":402}}"#;

    #[test]
    fn a_402_that_can_only_afford_part_of_the_reply_is_low_credit() {
        assert_eq!(
            ApiError::from_response(402, CANNOT_AFFORD_REPLY),
            ApiError::LowCredit
        );
        assert_eq!(
            ApiError::from_response(402, "{\"error\":{\"message\":\"CAN ONLY AFFORD 3\"}}"),
            ApiError::LowCredit
        );
        // Any other 402 is still an empty balance.
        assert_eq!(
            ApiError::from_response(402, "{\"error\":{\"message\":\"Insufficient credits\"}}"),
            ApiError::NoCredit
        );
    }

    #[test]
    fn maps_status_codes() {
        assert_eq!(ApiError::from_response(401, "{}"), ApiError::Unauthorized);
        assert_eq!(ApiError::from_response(402, "{}"), ApiError::NoCredit);
        assert_eq!(
            ApiError::from_response(429, "{}"),
            ApiError::RateLimited {
                retry_after_secs: None
            }
        );
    }

    #[test]
    fn no_endpoints_found_is_model_unavailable() {
        let body = r#"{"error":{"message":"No endpoints found for foo/bar:free.","code":404}}"#;
        assert_eq!(
            ApiError::from_response(404, body),
            ApiError::ModelUnavailable
        );
        assert_eq!(
            ApiError::from_response(503, body),
            ApiError::ModelUnavailable
        );
    }

    #[test]
    fn context_length_is_context_too_long() {
        let body = r#"{"error":{"message":"This endpoint's maximum context length is 8192 tokens.","code":400}}"#;
        assert_eq!(ApiError::from_response(400, body), ApiError::ContextTooLong);
    }

    #[test]
    fn html_body_becomes_trimmed_single_line_other() {
        let html = format!(
            "<html>\n<body>\n  <h1>Bad gateway</h1>\n{}</body></html>",
            "x".repeat(1000)
        );
        match ApiError::from_response(502, &html) {
            ApiError::Other { status, message } => {
                assert_eq!(status, 502);
                assert!(message.chars().count() <= 200);
                assert!(!message.contains('\n'));
                assert!(message.starts_with("<html> <body> <h1>Bad gateway</h1>"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn json_error_message_is_extracted() {
        match ApiError::from_response(400, r#"{"error":{"message":"bad  field","code":400}}"#) {
            ApiError::Other { message, .. } => assert_eq!(message, "bad field"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn retry_after_reads_seconds_only() {
        assert_eq!(parse_retry_after("7"), Some(7));
        assert_eq!(parse_retry_after(" 30 "), Some(30));
        assert_eq!(parse_retry_after("0"), Some(0));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("-3"), None);
        assert_eq!(parse_retry_after("+3"), None);
        assert_eq!(parse_retry_after("1.5"), None);
        assert_eq!(parse_retry_after(""), None);
        assert_eq!(parse_retry_after("99999999999999999999999"), None);
    }

    #[test]
    fn retry_after_applies_only_to_rate_limits() {
        assert_eq!(
            ApiError::from_response(429, "{}").with_retry_after(Some("7")),
            ApiError::RateLimited {
                retry_after_secs: Some(7)
            }
        );
        assert_eq!(
            ApiError::from_response(401, "{}").with_retry_after(Some("7")),
            ApiError::Unauthorized
        );
        assert_eq!(
            ApiError::from_response(429, "{}").with_retry_after(None),
            ApiError::RateLimited {
                retry_after_secs: None
            }
        );
    }
}
