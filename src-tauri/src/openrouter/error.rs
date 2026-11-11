const MAX_MESSAGE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Unauthorized,
    RateLimited { retry_after_secs: Option<u64> },
    NoCredit,
    ModelUnavailable,
    ContextTooLong,
    Network(String),
    Other { status: u16, message: String },
}

impl ApiError {
    /// Classify a non-success HTTP response. `Other.message` is a short,
    /// single-line string so a raw HTML or JSON body never travels onward.
    pub fn from_response(status: u16, body: &str) -> ApiError {
        let message = short_message(body);
        let lower = message.to_lowercase();
        match status {
            401 => ApiError::Unauthorized,
            402 => ApiError::NoCredit,
            429 => ApiError::RateLimited {
                retry_after_secs: None,
            },
            404 | 503 if lower.contains("no endpoints found") => ApiError::ModelUnavailable,
            400 if lower.contains("context length") => ApiError::ContextTooLong,
            _ => ApiError::Other { status, message },
        }
    }
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
}
