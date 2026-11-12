use super::{
    Engine, EngineEvent, StepError, StepOutcome, StepRequest, TurnOutcome, TurnRequest, UiError,
    finish_turn,
};
use crate::openrouter::error::ApiError;
use crate::openrouter::stream::{Delta, chat_stream};
use crate::openrouter::tool_calls::{Collector, Fragment};
use crate::secrets::KeyStore;
use async_trait::async_trait;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Talks to OpenRouter chat completions directly, one request per turn.
pub struct DirectEngine {
    http: reqwest::Client,
    base_url: String,
    key_source: Arc<dyn KeyStore>,
}

impl DirectEngine {
    pub fn new(http: reqwest::Client, base_url: String, key_source: Arc<dyn KeyStore>) -> Self {
        Self {
            http,
            base_url,
            key_source,
        }
    }
}

/// How one request went, beyond what the caller of a step sees.
struct Ran {
    out: StepOutcome,
    /// A request was actually sent.
    sent: bool,
    /// The cancel token ended the request.
    cancelled: bool,
}

fn error_outcome(error: UiError, retry_after_secs: Option<u64>) -> StepOutcome {
    StepOutcome {
        error: Some(StepError {
            ui: error,
            retry_after_secs,
        }),
        ..StepOutcome::default()
    }
}

fn api_error(e: ApiError) -> StepOutcome {
    let retry_after_secs = match &e {
        ApiError::RateLimited { retry_after_secs } => *retry_after_secs,
        _ => None,
    };
    error_outcome(e.into(), retry_after_secs)
}

impl DirectEngine {
    async fn step(
        &self,
        request: StepRequest,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Ran {
        // Read on every request so signing out takes effect immediately.
        let key = match self.key_source.get() {
            Ok(Some(k)) => k,
            Ok(None) => return unsent(api_error(ApiError::Unauthorized)),
            Err(_) => return unsent(error_outcome(UiError::key_unreadable(), None)),
        };

        let mut ran = Ran {
            out: StepOutcome::default(),
            sent: true,
            cancelled: false,
        };
        let session = (!request.session_id.is_empty()).then_some(request.session_id.as_str());
        let started = tokio::select! {
            _ = cancel.cancelled() => None,
            r = chat_stream(
                &self.http,
                &self.base_url,
                &key,
                &request.model_id,
                &request.messages,
                request.tools.as_ref(),
                session,
            ) => Some(r),
        };
        let stream = match started {
            None => {
                return Ran {
                    cancelled: true,
                    ..ran
                };
            }
            Some(Ok(s)) => s,
            Some(Err(e)) => {
                ran.out = api_error(e);
                return ran;
            }
        };
        futures_util::pin_mut!(stream);

        let mut collector = Collector::default();
        loop {
            let item = tokio::select! {
                _ = cancel.cancelled() => {
                    ran.cancelled = true;
                    break;
                }
                item = stream.next() => item,
            };
            match item {
                Some(Ok(Delta::Text(text))) => {
                    ran.out.text.push_str(&text);
                    emit(EngineEvent::TextDelta { text });
                }
                Some(Ok(Delta::Usage {
                    cost_usd,
                    prompt_tokens,
                    ..
                })) => {
                    ran.out.cost_usd = Some(ran.out.cost_usd.unwrap_or(0.0) + cost_usd);
                    ran.out.prompt_tokens = Some(u32::try_from(prompt_tokens).unwrap_or(u32::MAX));
                }
                Some(Ok(Delta::ToolCallPart {
                    index,
                    id,
                    name,
                    arguments,
                })) => collector.push(Fragment {
                    index,
                    id,
                    name,
                    arguments,
                }),
                // The reason is not needed: a cut-off call is rejected by the
                // collector's own checks.
                Some(Ok(Delta::Finish { .. })) => {}
                Some(Ok(Delta::Done)) => break,
                Some(Err(e)) => {
                    ran.out.error = api_error(e).error;
                    break;
                }
                // Defensive: the stream always ends with Done or an error.
                None => {
                    ran.out.error = api_error(ApiError::Network("stream closed".into())).error;
                    break;
                }
            }
        }
        ran.out.overflowed = collector.overflowed();
        // Calls from a reply that failed or was stopped may be cut short.
        if ran.out.error.is_none() && !ran.cancelled {
            ran.out.tool_calls = collector.finish();
        }
        ran
    }
}

fn unsent(out: StepOutcome) -> Ran {
    Ran {
        out,
        sent: false,
        cancelled: false,
    }
}

#[async_trait]
impl Engine for DirectEngine {
    async fn run_step(
        &self,
        request: StepRequest,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: CancellationToken,
    ) -> StepOutcome {
        let ran = self.step(request, emit, &cancel).await;
        StepOutcome {
            sent: ran.sent,
            ..ran.out
        }
    }

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
        let ran = self.step(step, emit, &cancel).await;
        finish_turn(ran.out, u32::from(ran.sent), ran.cancelled, emit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolCall;
    use crate::openrouter::stream::{Part, Role, WireMessage};
    use crate::secrets::{SecretError, SessionStore, StorageKind};
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const OK: &str = include_str!("../../tests/fixtures/stream_ok.txt");

    fn req() -> TurnRequest {
        TurnRequest {
            model_id: "m/x".into(),
            messages: vec![WireMessage {
                role: Role::User,
                parts: vec![Part::Text("hi".into())],
                tool_calls: vec![],
                tool_call_id: None,
            }],
        }
    }

    fn store(key: Option<&str>) -> Arc<dyn KeyStore> {
        let s = SessionStore::default();
        if let Some(k) = key {
            s.set(k).unwrap();
        }
        Arc::new(s)
    }

    async fn serve(resp: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/chat/completions"))
            .respond_with(resp)
            .mount(&server)
            .await;
        server
    }

    fn sse(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }

    async fn run(
        base: &str,
        keys: Arc<dyn KeyStore>,
        cancel: CancellationToken,
    ) -> (Vec<EngineEvent>, TurnOutcome) {
        let engine = DirectEngine::new(reqwest::Client::new(), base.to_string(), keys);
        let events = Mutex::new(Vec::new());
        let out = engine
            .run_turn(req(), &|e| events.lock().unwrap().push(e), cancel)
            .await;
        (events.into_inner().unwrap(), out)
    }

    fn base(server: &MockServer) -> String {
        format!("{}/api/v1", server.uri())
    }

    #[tokio::test]
    async fn emits_deltas_then_usage_then_done() {
        let server = serve(sse(OK)).await;
        let (events, out) = run(
            &base(&server),
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            events,
            vec![
                EngineEvent::TextDelta { text: "Hel".into() },
                EngineEvent::TextDelta { text: "lo".into() },
                EngineEvent::Usage {
                    requests: 1,
                    cost_usd: 0.0
                },
                EngineEvent::Done,
            ]
        );
        assert_eq!(out.text, "Hello");
        assert_eq!(out.requests, 1);
        assert!(!out.interrupted);
    }

    #[tokio::test]
    async fn failure_before_any_text_emits_failed_with_exact_copy() {
        let cases: Vec<(u16, &str, &str, &str, bool)> = vec![
            (
                401,
                "{}",
                "unauthorized",
                "OpenRouter no longer accepts this key. Sign in again to continue.",
                false,
            ),
            (
                429,
                "{}",
                "rate_limited",
                "OpenRouter is limiting requests right now. Wait a moment, then try again.",
                true,
            ),
            (
                402,
                "{}",
                "no_credit",
                "This model needs credit and your OpenRouter balance is empty. Pick a free model or add credit.",
                false,
            ),
            (
                402,
                r#"{"error":{"message":"You requested up to 131072 tokens, but can only afford 8450."}}"#,
                "low_credit",
                "Your balance is too low for this model's full reply. Add credit or pick a cheaper model.",
                false,
            ),
            (
                404,
                r#"{"error":{"message":"No endpoints found for m/x"}}"#,
                "model_unavailable",
                "This model is not available right now. Pick another model and try again.",
                true,
            ),
            (
                400,
                r#"{"error":{"message":"maximum context length is 10"}}"#,
                "context_too_long",
                "This chat is too long for the model. Start a new chat or pick a model with a larger limit.",
                false,
            ),
            (
                500,
                r#"{"error":{"message":"boom"}}"#,
                "other",
                "OpenRouter returned an error: boom",
                false,
            ),
        ];
        for (status, body, kind, message, retryable) in cases {
            let server = serve(ResponseTemplate::new(status).set_body_string(body)).await;
            let (events, out) = run(
                &base(&server),
                store(Some("sk-k")),
                CancellationToken::new(),
            )
            .await;
            let error = UiError {
                kind: kind.into(),
                message: message.into(),
                retryable,
            };
            assert_eq!(events, vec![EngineEvent::Failed { error }], "{kind}");
            assert_eq!(out.requests, 1, "{kind}");
            assert!(out.text.is_empty() && !out.interrupted, "{kind}");
        }
        // Network: nothing is listening.
        let (events, out) = run(
            "http://127.0.0.1:1",
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            events,
            vec![EngineEvent::Failed {
                error: UiError {
                    kind: "network".into(),
                    message: "Could not reach OpenRouter. Check your connection and try again."
                        .into(),
                    retryable: true
                }
            }]
        );
        assert_eq!(out.requests, 1);
    }

    #[tokio::test]
    async fn failure_after_text_emits_interrupted_and_keeps_partial() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"error\":{\"code\":429,\"message\":\"slow\"}}\n\n";
        let server = serve(sse(body)).await;
        let (events, out) = run(
            &base(&server),
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            events,
            vec![
                EngineEvent::TextDelta { text: "Hel".into() },
                EngineEvent::Interrupted {
                    reason: "rate_limited".into()
                },
            ]
        );
        assert_eq!(out.text, "Hel");
        assert!(out.interrupted);
        assert_eq!(out.requests, 1);
    }

    #[tokio::test]
    async fn cancel_stops_promptly() {
        // A raw server that sends one delta, then holds the connection open.
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n";
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n",
                chunk.len()
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        let cancel = CancellationToken::new();
        let engine = DirectEngine::new(
            reqwest::Client::new(),
            format!("http://{addr}"),
            store(Some("sk-k")),
        );
        let events = Mutex::new(Vec::new());
        let c = cancel.clone();
        let emit = |e: EngineEvent| {
            if matches!(e, EngineEvent::TextDelta { .. }) {
                c.cancel();
            }
            events.lock().unwrap().push(e);
        };
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_turn(req(), &emit, cancel),
        )
        .await
        .expect("cancel must stop the turn promptly");
        assert_eq!(
            events.into_inner().unwrap(),
            vec![
                EngineEvent::TextDelta { text: "Hel".into() },
                EngineEvent::Interrupted {
                    reason: "stopped".into()
                },
            ]
        );
        assert_eq!(out.text, "Hel");
        assert!(out.interrupted);
        assert_eq!(out.requests, 1);
    }

    #[tokio::test]
    async fn missing_key_emits_failed_unauthorized_without_any_http_call() {
        let server = serve(sse(OK)).await;
        let (events, out) = run(&base(&server), store(None), CancellationToken::new()).await;
        assert_eq!(
            events,
            vec![EngineEvent::Failed {
                error: ApiError::Unauthorized.into()
            }]
        );
        assert_eq!(out.requests, 0);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    struct BrokenStore;
    impl KeyStore for BrokenStore {
        fn get(&self) -> Result<Option<String>, SecretError> {
            Err(SecretError::Other("denied".into()))
        }
        fn set(&self, _: &str) -> Result<(), SecretError> {
            Ok(())
        }
        fn clear(&self) -> Result<(), SecretError> {
            Ok(())
        }
        fn kind(&self) -> StorageKind {
            StorageKind::Session
        }
    }

    #[tokio::test]
    async fn unreadable_key_emits_failed_other_without_any_http_call() {
        let server = serve(sse(OK)).await;
        let (events, out) = run(
            &base(&server),
            Arc::new(BrokenStore),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            events,
            vec![EngineEvent::Failed {
                error: UiError {
                    kind: "other".into(),
                    message: "OpenRouter returned an error: could not read the saved key".into(),
                    retryable: false
                }
            }]
        );
        assert_eq!(out.requests, 0);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn key_never_appears_in_events_or_outcome() {
        let server = serve(ResponseTemplate::new(500).set_body_string("{}")).await;
        let (events, out) = run(
            &base(&server),
            store(Some("sk-secret-123")),
            CancellationToken::new(),
        )
        .await;
        let dump = format!("{events:?}{out:?}");
        assert!(!dump.contains("sk-secret-123"));
    }

    fn step_req() -> StepRequest {
        StepRequest {
            model_id: "m/x".into(),
            messages: req().messages,
            tools: Some(serde_json::json!([{"type":"function","function":{"name":"read_file"}}])),
            session_id: "chat-1".into(),
        }
    }

    fn tool_chunk(index: usize, id: Option<&str>, name: Option<&str>, args: &str) -> String {
        let part = serde_json::json!({
            "index": index,
            "id": id,
            "function": {"name": name, "arguments": args},
        });
        let chunk = serde_json::json!({"choices":[{"delta":{"tool_calls":[part]}}]});
        format!("data: {chunk}\n\n")
    }

    async fn step(
        base: &str,
        keys: Arc<dyn KeyStore>,
        cancel: CancellationToken,
    ) -> (Vec<EngineEvent>, StepOutcome) {
        let engine = DirectEngine::new(reqwest::Client::new(), base.to_string(), keys);
        let events = Mutex::new(Vec::new());
        let out = engine
            .run_step(step_req(), &|e| events.lock().unwrap().push(e), cancel)
            .await;
        (events.into_inner().unwrap(), out)
    }

    #[tokio::test]
    async fn run_step_returns_text_tool_calls_and_cost() {
        let body = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"Let me look.\"}}}}]}}\n\n{}{}{}{}",
            tool_chunk(0, Some("call_1"), Some("read_file"), "{\"pa"),
            tool_chunk(0, None, None, "th\":\"a.txt\"}"),
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"cost\":0.002,\"prompt_tokens\":12,\"completion_tokens\":5}}\n\ndata: [DONE]\n\n",
        );
        let server = serve(sse(&body)).await;
        let (events, out) = step(
            &base(&server),
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            events,
            vec![EngineEvent::TextDelta {
                text: "Let me look.".into()
            }]
        );
        assert_eq!(out.text, "Let me look.");
        assert_eq!(
            out.tool_calls,
            vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"a.txt\"}".into(),
            }]
        );
        assert_eq!(out.cost_usd, Some(0.002));
        assert_eq!(out.prompt_tokens, Some(12));
        assert!(out.error.is_none() && !out.overflowed);

        let sent = &server.received_requests().await.unwrap()[0];
        let sent: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
        assert_eq!(sent["tools"][0]["function"]["name"], "read_file");
        assert_eq!(sent["session_id"], "chat-1");
    }

    #[tokio::test]
    async fn run_step_without_usage_has_no_cost() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\ndata: [DONE]\n\n";
        let server = serve(sse(body)).await;
        let (_, out) = step(
            &base(&server),
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(out.cost_usd, None);
        assert_eq!(out.prompt_tokens, None);
    }

    #[tokio::test]
    async fn run_step_rate_limit_carries_retry_after() {
        let server = serve(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "7")
                .set_body_string("{}"),
        )
        .await;
        let (events, out) = step(
            &base(&server),
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert!(events.is_empty(), "run_step never emits Failed");
        let error = out.error.expect("an error");
        assert_eq!(error.retry_after_secs, Some(7));
        assert_eq!(error.ui.kind, "rate_limited");
        assert!(error.ui.retryable);
        assert!(out.text.is_empty() && out.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn run_step_rate_limit_without_a_usable_header_has_no_wait() {
        for header in [None, Some("Wed, 21 Oct 2026 07:28:00 GMT"), Some("soon")] {
            let mut resp = ResponseTemplate::new(429).set_body_string("{}");
            if let Some(h) = header {
                resp = resp.insert_header("Retry-After", h);
            }
            let server = serve(resp).await;
            let (_, out) = step(
                &base(&server),
                store(Some("sk-k")),
                CancellationToken::new(),
            )
            .await;
            assert_eq!(out.error.unwrap().retry_after_secs, None, "{header:?}");
        }
    }

    #[tokio::test]
    async fn run_step_cancelled_mid_stream_returns_partial_text_and_no_error() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n";
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n",
                chunk.len()
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        let cancel = CancellationToken::new();
        let engine = DirectEngine::new(
            reqwest::Client::new(),
            format!("http://{addr}"),
            store(Some("sk-k")),
        );
        let c = cancel.clone();
        let emit = move |e: EngineEvent| {
            if matches!(e, EngineEvent::TextDelta { .. }) {
                c.cancel();
            }
        };
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.run_step(step_req(), &emit, cancel),
        )
        .await
        .expect("cancel must stop the step promptly");
        assert_eq!(out.text, "Hel");
        assert!(out.error.is_none());
        assert!(out.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn run_step_error_after_text_keeps_the_text() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"error\":{\"code\":429,\"message\":\"slow\"}}\n\n";
        let server = serve(sse(body)).await;
        let (_, out) = step(
            &base(&server),
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(out.text, "Hel");
        assert_eq!(out.error.unwrap().ui.kind, "rate_limited");
    }

    #[tokio::test]
    async fn run_step_overflowed_reply_has_text_no_calls_and_no_error() {
        let mut body =
            String::from("data: {\"choices\":[{\"delta\":{\"content\":\"Doing it all.\"}}]}\n\n");
        for i in 0..33 {
            body.push_str(&tool_chunk(
                i,
                Some(&format!("c{i}")),
                Some("read_file"),
                "{}",
            ));
        }
        body.push_str("data: [DONE]\n\n");
        let server = serve(sse(&body)).await;
        let (_, out) = step(
            &base(&server),
            store(Some("sk-k")),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(out.text, "Doing it all.");
        assert!(out.tool_calls.is_empty());
        assert!(out.overflowed);
        assert!(out.error.is_none());
    }

    #[tokio::test]
    async fn run_step_missing_key_is_unauthorized_without_any_http_call() {
        let server = serve(sse(OK)).await;
        let (events, out) = step(&base(&server), store(None), CancellationToken::new()).await;
        assert!(events.is_empty());
        assert_eq!(out.error.unwrap().ui.kind, "unauthorized");
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
