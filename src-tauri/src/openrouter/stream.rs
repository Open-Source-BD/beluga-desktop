use super::endpoint;
use super::error::ApiError;
use crate::brand;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    ImageDataUrl(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMessage {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    Usage {
        cost_usd: f64,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    Done,
}

fn message_json(m: &WireMessage) -> Value {
    let content: Vec<Value> = m
        .parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => json!({"type": "text", "text": t}),
            Part::ImageDataUrl(u) => json!({"type": "image_url", "image_url": {"url": u}}),
        })
        .collect();
    json!({"role": m.role, "content": content})
}

fn request_body(model_id: &str, messages: &[WireMessage]) -> Value {
    json!({
        "model": model_id,
        "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
        "stream": true,
        "usage": {"include": true},
    })
}

/// Start a streaming chat completion. Non-2xx responses fail here; once the
/// stream has started, failures arrive as `Err` items and end the stream.
pub async fn chat_stream(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
    model_id: &str,
    messages: &[WireMessage],
) -> Result<impl Stream<Item = Result<Delta, ApiError>> + use<>, ApiError> {
    let resp = http
        .post(endpoint(base_url, "chat/completions"))
        .bearer_auth(key)
        .header("X-Title", brand::APP_NAME)
        .json(&request_body(model_id, messages))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp
            .text()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        return Err(ApiError::from_response(status.as_u16(), &body));
    }
    Ok(decode(resp.bytes_stream()))
}

struct State<S> {
    events: S,
    pending: VecDeque<Result<Delta, ApiError>>,
    finished: bool,
}

/// Turn raw response bytes (split anywhere, including inside a multi-byte
/// character or an SSE line) into deltas. Ends after `Done` or the first error.
pub(crate) fn decode<S, E>(bytes: S) -> impl Stream<Item = Result<Delta, ApiError>>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: Display,
{
    let state = State {
        events: bytes.eventsource(),
        pending: VecDeque::new(),
        finished: false,
    };
    futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.pending.pop_front() {
                if matches!(item, Ok(Delta::Done) | Err(_)) {
                    st.finished = true;
                    st.pending.clear();
                }
                return Some((item, st));
            }
            if st.finished {
                return None;
            }
            match st.events.next().await {
                None => {
                    st.pending.push_back(Err(ApiError::Network(
                        "stream ended before completion".to_string(),
                    )));
                }
                Some(Err(e)) => st.pending.push_back(Err(ApiError::Network(e.to_string()))),
                Some(Ok(ev)) => handle_data(&ev.data, &mut st.pending),
            }
        }
    })
}

fn handle_data(data: &str, out: &mut VecDeque<Result<Delta, ApiError>>) {
    let data = data.trim();
    if data.is_empty() {
        return;
    }
    if data == "[DONE]" {
        out.push_back(Ok(Delta::Done));
        return;
    }
    let Ok(chunk) = serde_json::from_str::<Value>(data) else {
        out.push_back(Err(ApiError::Other {
            status: 200,
            message: "malformed stream data".to_string(),
        }));
        return;
    };
    if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
        let status = err
            .get("code")
            .and_then(Value::as_u64)
            .and_then(|c| u16::try_from(c).ok())
            .unwrap_or(500);
        out.push_back(Err(ApiError::from_response(status, data)));
        return;
    }
    if let Some(text) = chunk["choices"][0]["delta"]["content"]
        .as_str()
        .filter(|t| !t.is_empty())
    {
        out.push_back(Ok(Delta::Text(text.to_string())));
    }
    if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
        out.push_back(Ok(Delta::Usage {
            cost_usd: usage["cost"].as_f64().unwrap_or(0.0),
            prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
            completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    const OK: &str = include_str!("../../tests/fixtures/stream_ok.txt");
    const SPLIT: &str = include_str!("../../tests/fixtures/stream_split_utf8.txt");
    const ERROR_MID: &str = include_str!("../../tests/fixtures/stream_error_mid.txt");
    const NO_DONE: &str = include_str!("../../tests/fixtures/stream_no_done.txt");

    fn user(text: &str) -> Vec<WireMessage> {
        vec![WireMessage {
            role: Role::User,
            parts: vec![Part::Text(text.to_string())],
        }]
    }

    async fn collect_from(body: &str) -> Vec<Result<Delta, ApiError>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
        let base = format!("{}/api/v1", server.uri());
        let s = chat_stream(&reqwest::Client::new(), &base, "sk-k", "m/x", &user("hi"))
            .await
            .unwrap();
        s.collect().await
    }

    fn text(s: &str) -> Result<Delta, ApiError> {
        Ok(Delta::Text(s.to_string()))
    }

    #[tokio::test]
    async fn ok() {
        let items = collect_from(OK).await;
        assert_eq!(
            items,
            vec![
                text("Hel"),
                text("lo"),
                Ok(Delta::Usage {
                    cost_usd: 0.0,
                    prompt_tokens: 10,
                    completion_tokens: 2
                }),
                Ok(Delta::Done),
            ]
        );
    }

    #[tokio::test]
    async fn comment_lines_are_ignored() {
        let items = collect_from(": OPENROUTER PROCESSING\n\ndata: [DONE]\n\n").await;
        assert_eq!(items, vec![Ok(Delta::Done)]);
    }

    #[tokio::test]
    async fn fixture_with_multibyte_text() {
        let items = collect_from(SPLIT).await;
        assert_eq!(items, vec![text("নমস্কার"), Ok(Delta::Done)]);
    }

    #[tokio::test]
    async fn split_utf8() {
        let bytes = SPLIT.as_bytes();
        let ch = "ম".as_bytes();
        let at = bytes.windows(ch.len()).position(|w| w == ch).unwrap() + 1;
        // Also split inside the "data:" field name of the second event.
        let second = SPLIT.find("data: [DONE]").unwrap() + 2;
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::copy_from_slice(&bytes[..at])),
            Ok(Bytes::copy_from_slice(&bytes[at..second])),
            Ok(Bytes::copy_from_slice(&bytes[second..])),
        ];
        let items: Vec<_> = decode(stream::iter(chunks)).collect().await;
        assert_eq!(items, vec![text("নমস্কার"), Ok(Delta::Done)]);
    }

    #[tokio::test]
    async fn error_mid() {
        let items = collect_from(ERROR_MID).await;
        assert_eq!(
            items,
            vec![
                text("Hi"),
                Err(ApiError::RateLimited {
                    retry_after_secs: None
                })
            ]
        );
    }

    #[tokio::test]
    async fn no_done() {
        let items = collect_from(NO_DONE).await;
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], text("Hel"));
        assert_eq!(items[1], text("lo"));
        assert!(matches!(items[2], Err(ApiError::Network(_))));
    }

    #[tokio::test]
    async fn byte_stream_error_is_network_and_ends() {
        let chunks: Vec<Result<Bytes, String>> = vec![
            Ok(Bytes::from_static(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
            )),
            Err("connection reset".to_string()),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];
        let items: Vec<_> = decode(stream::iter(chunks)).collect().await;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], text("a"));
        assert!(matches!(items[1], Err(ApiError::Network(_))));
    }

    #[tokio::test]
    async fn chunks_without_content_produce_nothing() {
        let body = concat!(
            "data: {\"choices\":[]}\n\n",
            "data: {\"id\":\"x\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":null}}]}\n\n",
            "data: [DONE]\n\n",
        );
        assert_eq!(collect_from(body).await, vec![Ok(Delta::Done)]);
    }

    #[tokio::test]
    async fn malformed_json_ends_the_stream_with_other() {
        let body = "data: {not json\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\ndata: [DONE]\n\n";
        let items = collect_from(body).await;
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Err(ApiError::Other { .. })));
    }

    #[tokio::test]
    async fn http_401_before_stream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("{}"))
            .mount(&server)
            .await;
        let r = chat_stream(
            &reqwest::Client::new(),
            &server.uri(),
            "bad",
            "m",
            &user("x"),
        )
        .await;
        assert!(matches!(r, Err(ApiError::Unauthorized)));
    }

    #[tokio::test]
    async fn request_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-title", brand::APP_NAME))
            .respond_with(ResponseTemplate::new(200).set_body_string("data: [DONE]\n\n"))
            .mount(&server)
            .await;
        let msgs = vec![
            WireMessage {
                role: Role::System,
                parts: vec![Part::Text("be brief".into())],
            },
            WireMessage {
                role: Role::User,
                parts: vec![
                    Part::Text("what is this".into()),
                    Part::ImageDataUrl("data:image/png;base64,AAAA".into()),
                ],
            },
        ];
        let s = chat_stream(&reqwest::Client::new(), &server.uri(), "sk-k", "m/x", &msgs)
            .await
            .unwrap();
        let _: Vec<_> = s.collect().await;
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].headers.get("http-referer").is_none());
        let v: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(v["stream"], true);
        assert_eq!(v["usage"]["include"], true);
        assert_eq!(v["model"], "m/x");
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(
            v["messages"][0]["content"][0],
            json!({"type":"text","text":"be brief"})
        );
        assert_eq!(v["messages"][1]["role"], "user");
        assert_eq!(
            v["messages"][1]["content"][1],
            json!({"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}})
        );
    }

    #[tokio::test]
    async fn key_is_sent_as_bearer_and_not_in_the_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Bearer sk-secret-123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("data: [DONE]\n\n"))
            .mount(&server)
            .await;
        let s = chat_stream(
            &reqwest::Client::new(),
            &server.uri(),
            "sk-secret-123",
            "m",
            &user("x"),
        )
        .await
        .unwrap();
        let _: Vec<_> = s.collect().await;
        let reqs: Vec<Request> = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(!reqs[0].url.as_str().contains("sk-secret-123"));
        assert!(!String::from_utf8_lossy(&reqs[0].body).contains("sk-secret-123"));
    }
}
