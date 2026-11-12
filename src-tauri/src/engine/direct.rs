use super::{Engine, EngineEvent, TurnOutcome, TurnRequest, UiError};
use crate::openrouter::error::ApiError;
use crate::openrouter::stream::{Delta, chat_stream};
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

fn failed(
    emit: &(dyn Fn(EngineEvent) + Send + Sync),
    error: UiError,
    out: TurnOutcome,
) -> TurnOutcome {
    emit(EngineEvent::Failed { error });
    out
}

#[async_trait]
impl Engine for DirectEngine {
    async fn run_turn(
        &self,
        req: TurnRequest,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: CancellationToken,
    ) -> TurnOutcome {
        let mut out = TurnOutcome {
            text: String::new(),
            requests: 0,
            cost_usd: 0.0,
            interrupted: false,
        };
        // Read on every turn so signing out takes effect immediately.
        let key = match self.key_source.get() {
            Ok(Some(k)) => k,
            Ok(None) => return failed(emit, ApiError::Unauthorized.into(), out),
            Err(_) => return failed(emit, UiError::key_unreadable(), out),
        };

        out.requests = 1;
        let started = tokio::select! {
            _ = cancel.cancelled() => None,
            r = chat_stream(&self.http, &self.base_url, &key, &req.model_id, &req.messages, None, None) => Some(r),
        };
        let stream = match started {
            None => return stopped(emit, out),
            Some(Ok(s)) => s,
            Some(Err(e)) => return failed(emit, e.into(), out),
        };
        futures_util::pin_mut!(stream);

        loop {
            let item = tokio::select! {
                _ = cancel.cancelled() => return stopped(emit, out),
                item = stream.next() => item,
            };
            match item {
                Some(Ok(Delta::Text(text))) => {
                    out.text.push_str(&text);
                    emit(EngineEvent::TextDelta { text });
                }
                Some(Ok(Delta::Usage { cost_usd, .. })) => {
                    out.cost_usd += cost_usd;
                    emit(EngineEvent::Usage {
                        requests: out.requests,
                        cost_usd: out.cost_usd,
                    });
                }
                // Tool calls are not used yet; the next task reads them.
                Some(Ok(Delta::ToolCallPart { .. } | Delta::Finish { .. })) => {}
                Some(Ok(Delta::Done)) => {
                    emit(EngineEvent::Done);
                    return out;
                }
                Some(Err(e)) => {
                    let error = UiError::from(e);
                    if out.text.is_empty() {
                        return failed(emit, error, out);
                    }
                    out.interrupted = true;
                    emit(EngineEvent::Interrupted { reason: error.kind });
                    return out;
                }
                // Defensive: the stream always ends with Done or an error.
                None => {
                    out.interrupted = true;
                    emit(EngineEvent::Interrupted {
                        reason: "network".to_string(),
                    });
                    return out;
                }
            }
        }
    }
}

fn stopped(emit: &(dyn Fn(EngineEvent) + Send + Sync), mut out: TurnOutcome) -> TurnOutcome {
    out.interrupted = true;
    emit(EngineEvent::Interrupted {
        reason: "stopped".to_string(),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
