use super::endpoint;
use super::error::ApiError;
use crate::agent::ToolCall;
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
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMessage {
    pub role: Role,
    pub parts: Vec<Part>,
    /// Calls an assistant message made; empty for every other message.
    pub tool_calls: Vec<ToolCall>,
    /// For a `Role::Tool` message, the call it answers.
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    Usage {
        cost_usd: f64,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// One piece of a tool call; pieces with the same `index` belong together.
    ToolCallPart {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    /// The reply ended for a reason other than a plain `stop`.
    Finish {
        reason: String,
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
    if m.role == Role::Tool {
        // A tool result is plain string content.
        let text: String = m
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                Part::ImageDataUrl(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return json!({"role": m.role, "tool_call_id": m.tool_call_id, "content": text});
    }
    if !m.tool_calls.is_empty() {
        let calls: Vec<Value> = m
            .tool_calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": {"name": c.name, "arguments": c.arguments},
                })
            })
            .collect();
        let content = if content.is_empty() {
            Value::Null
        } else {
            Value::Array(content)
        };
        return json!({"role": m.role, "content": content, "tool_calls": calls});
    }
    json!({"role": m.role, "content": content})
}

fn request_body(
    model_id: &str,
    messages: &[WireMessage],
    tools: Option<&Value>,
    session_id: Option<&str>,
) -> Value {
    let mut body = json!({
        "model": model_id,
        "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
        "stream": true,
        "usage": {"include": true},
    });
    if let Some(tools) = tools {
        body["tools"] = tools.clone();
        body["provider"] = json!({"require_parameters": true});
        if let Some(id) = session_id {
            body["session_id"] = json!(id);
        }
    }
    body
}

/// Start a streaming chat completion. Non-2xx responses fail here; once the
/// stream has started, failures arrive as `Err` items and end the stream.
pub async fn chat_stream(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
    model_id: &str,
    messages: &[WireMessage],
    tools: Option<&Value>,
    session_id: Option<&str>,
) -> Result<impl Stream<Item = Result<Delta, ApiError>> + use<>, ApiError> {
    let resp = http
        .post(endpoint(base_url, "chat/completions"))
        .bearer_auth(key)
        .header("X-Title", brand::APP_NAME)
        .json(&request_body(model_id, messages, tools, session_id))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = resp
            .text()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        return Err(ApiError::from_response(status.as_u16(), &body)
            .with_retry_after(retry_after.as_deref()));
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
    let choice = &chunk["choices"][0];
    if let Some(parts) = choice["delta"]["tool_calls"].as_array() {
        for part in parts {
            let function = &part["function"];
            let arguments = match &function["arguments"] {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                // Objects are serialised; anything else is passed on as raw
                // JSON so the argument check rejects it.
                other => other.to_string(),
            };
            out.push_back(Ok(Delta::ToolCallPart {
                index: part["index"]
                    .as_u64()
                    .and_then(|i| u32::try_from(i).ok())
                    .unwrap_or(0),
                id: part["id"].as_str().map(str::to_string),
                name: function["name"].as_str().map(str::to_string),
                arguments,
            }));
        }
    }
    // A plain "stop" is the ordinary end and reports nothing.
    if let Some(reason) = choice["finish_reason"]
        .as_str()
        .filter(|r| !r.is_empty() && *r != "stop")
    {
        out.push_back(Ok(Delta::Finish {
            reason: reason.to_string(),
        }));
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
            tool_calls: vec![],
            tool_call_id: None,
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
        let s = chat_stream(
            &reqwest::Client::new(),
            &base,
            "sk-k",
            "m/x",
            &user("hi"),
            None,
            None,
        )
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
            None,
            None,
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
                tool_calls: vec![],
                tool_call_id: None,
            },
            WireMessage {
                role: Role::User,
                parts: vec![
                    Part::Text("what is this".into()),
                    Part::ImageDataUrl("data:image/png;base64,AAAA".into()),
                ],
                tool_calls: vec![],
                tool_call_id: None,
            },
        ];
        let s = chat_stream(
            &reqwest::Client::new(),
            &server.uri(),
            "sk-k",
            "m/x",
            &msgs,
            None,
            None,
        )
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
            None,
            None,
        )
        .await
        .unwrap();
        let _: Vec<_> = s.collect().await;
        let reqs: Vec<Request> = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(!reqs[0].url.as_str().contains("sk-secret-123"));
        assert!(!String::from_utf8_lossy(&reqs[0].body).contains("sk-secret-123"));
    }

    // ---- tools on the wire ----

    use crate::agent::ToolCall;
    use crate::openrouter::tool_calls::{Collector, Fragment};

    const T_FRAGMENTS: &str = include_str!("../../tests/fixtures/stream_tools_fragments.txt");
    const T_INTERLEAVED: &str = include_str!("../../tests/fixtures/stream_tools_interleaved.txt");
    const T_FINISH_STOP: &str = include_str!("../../tests/fixtures/stream_tools_finish_stop.txt");
    const T_TEXT_AND_CALL: &str =
        include_str!("../../tests/fixtures/stream_tools_text_and_call.txt");
    const T_NO_ID_NO_NAME: &str =
        include_str!("../../tests/fixtures/stream_tools_no_id_no_name.txt");

    /// Run a stream the way the engine will: text joined, fragments collected.
    async fn run_tools(body: &str) -> (String, Vec<ToolCall>, Option<String>) {
        let mut text = String::new();
        let mut collector = Collector::default();
        let mut finish = None;
        for item in collect_from(body).await {
            match item.unwrap() {
                Delta::Text(t) => text.push_str(&t),
                Delta::ToolCallPart {
                    index,
                    id,
                    name,
                    arguments,
                } => collector.push(Fragment {
                    index,
                    id,
                    name,
                    arguments,
                }),
                Delta::Finish { reason } => finish = Some(reason),
                _ => {}
            }
        }
        (text, collector.finish(), finish)
    }

    fn legacy_body(model_id: &str, messages: &[WireMessage]) -> Value {
        // The 0.2.0 body, as it was written.
        json!({
            "model": model_id,
            "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
            "stream": true,
            "usage": {"include": true},
        })
    }

    #[test]
    fn request_body_without_tools_is_byte_identical_to_0_2_0() {
        let msgs = vec![
            plain(Role::System, "be brief"),
            WireMessage {
                role: Role::User,
                parts: vec![
                    Part::Text("what".into()),
                    Part::ImageDataUrl("data:image/png;base64,AAAA".into()),
                ],
                tool_calls: vec![],
                tool_call_id: None,
            },
        ];
        let now = serde_json::to_string(&request_body("m/x", &msgs, None, None)).unwrap();
        let then = serde_json::to_string(&legacy_body("m/x", &msgs)).unwrap();
        assert_eq!(now, then);
        // A session id alone does not change a request without tools either.
        let with_sid = request_body("m/x", &msgs, None, Some("s1"));
        assert_eq!(serde_json::to_string(&with_sid).unwrap(), then);
    }

    fn plain(role: Role, text: &str) -> WireMessage {
        WireMessage {
            role,
            parts: vec![Part::Text(text.to_string())],
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    #[test]
    fn request_body_with_tools_sends_tools_provider_and_session() {
        let tools = json!([{"type": "function", "function": {"name": "list_dir"}}]);
        let v = request_body(
            "m/x",
            &[plain(Role::User, "hi")],
            Some(&tools),
            Some("chat-1"),
        );
        assert_eq!(v["tools"], tools);
        assert_eq!(v["provider"], json!({"require_parameters": true}));
        assert_eq!(v["session_id"], "chat-1");
        assert_eq!(v["stream"], true);
        assert_eq!(v["usage"]["include"], true);
    }

    #[test]
    fn assistant_calls_and_tool_result_serialise_in_the_openai_form() {
        let calls = vec![
            ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"a\"}".into(),
            },
            ToolCall {
                id: "call_2".into(),
                name: "list_dir".into(),
                arguments: "{}".into(),
            },
        ];
        let assistant = WireMessage {
            role: Role::Assistant,
            parts: vec![],
            tool_calls: calls,
            tool_call_id: None,
        };
        let v = message_json(&assistant);
        assert_eq!(v["role"], "assistant");
        assert!(v["content"].is_null());
        assert_eq!(
            v["tool_calls"],
            json!([
                {"id": "call_1", "type": "function",
                 "function": {"name": "read_file", "arguments": "{\"path\":\"a\"}"}},
                {"id": "call_2", "type": "function",
                 "function": {"name": "list_dir", "arguments": "{}"}},
            ])
        );
        let tool = WireMessage {
            role: Role::Tool,
            parts: vec![Part::Text("file text".into())],
            tool_calls: vec![],
            tool_call_id: Some("call_1".into()),
        };
        assert_eq!(
            message_json(&tool),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "file text"})
        );
    }

    #[tokio::test]
    async fn one_call_in_five_fragments_is_collected_whole() {
        let (_, calls, finish) = run_tools(T_FRAGMENTS).await;
        assert_eq!(
            calls,
            vec![ToolCall {
                id: "call_abc".into(),
                name: "read_file".into(),
                arguments: "{\"path\": \"src/app.ts\"}".into(),
            }]
        );
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }

    #[tokio::test]
    async fn interleaved_calls_come_out_in_index_order() {
        let (_, calls, _) = run_tools(T_INTERLEAVED).await;
        let got: Vec<_> = calls
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str(), c.arguments.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("call_a", "read_file", "{\"path\":\"a.txt\"}"),
                ("call_b", "list_dir", "{\"path\":\".\"}"),
            ]
        );
    }

    #[tokio::test]
    async fn a_call_followed_by_finish_stop_still_yields_the_call() {
        let (_, calls, finish) = run_tools(T_FINISH_STOP).await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(finish, None, "a plain stop is not reported as a finish");
    }

    #[tokio::test]
    async fn text_and_a_call_in_one_reply_yield_both() {
        let (text, calls, _) = run_tools(T_TEXT_AND_CALL).await;
        assert_eq!(text, "Let me look.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_t");
    }

    #[tokio::test]
    async fn no_id_gets_call_index_and_no_name_is_dropped() {
        let (_, calls, _) = run_tools(T_NO_ID_NO_NAME).await;
        assert_eq!(
            calls,
            vec![ToolCall {
                id: "call_0".into(),
                name: "list_dir".into(),
                arguments: "{\"path\":\".\"}".into(),
            }]
        );
    }

    #[tokio::test]
    async fn arguments_sent_as_an_object_are_serialised() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c\",",
            "\"function\":{\"name\":\"list_dir\",\"arguments\":{\"path\":\".\"}}}]}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (_, calls, _) = run_tools(body).await;
        assert_eq!(calls[0].arguments, "{\"path\":\".\"}");
    }

    // ---- streams recorded from real models in the live check (Task 16),
    // ids replaced by placeholders ----

    const LIVE_ERNIE: &str =
        include_str!("../../tests/fixtures/live/ernie-5.5-flash-preview-free.txt");
    const LIVE_NEMOTRON: &str = include_str!("../../tests/fixtures/live/nemotron-3-ultra-free.txt");
    const LIVE_LING: &str = include_str!("../../tests/fixtures/live/ling-3.0-tiny.txt");
    const LIVE_LLAMA: &str = include_str!("../../tests/fixtures/live/llama-3.1-8b-instruct.txt");

    fn tc(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    #[tokio::test]
    async fn live_baidu_text_then_two_calls() {
        let (text, calls, finish) = run_tools(LIVE_ERNIE).await;
        assert_eq!(
            text,
            "I'll start by reading notes.txt and finding the count script."
        );
        assert_eq!(
            calls,
            vec![
                tc("call_live_1", "read_file", "{\"path\": \"notes.txt\"}"),
                tc("call_live_2", "find_files", "{\"pattern\": \"*count*\"}"),
            ]
        );
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }

    #[tokio::test]
    async fn live_nvidia_processing_comments_and_one_call() {
        let (text, calls, finish) = run_tools(LIVE_NEMOTRON).await;
        assert_eq!(text, "");
        assert_eq!(
            calls,
            vec![tc("call_live_1", "list_dir", "{\"path\":\".\"}")]
        );
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }

    #[tokio::test]
    async fn live_ant_reasoning_chunks_and_two_calls() {
        let (text, calls, finish) = run_tools(LIVE_LING).await;
        assert_eq!(text, "\n");
        assert_eq!(
            calls,
            vec![
                tc("call_live_1", "read_file", "{\"path\": \"notes.txt\"}"),
                tc(
                    "call_live_2",
                    "find_files",
                    "{\"pattern\": \"count*\", \"path\": \".\"}"
                ),
            ]
        );
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }

    #[tokio::test]
    async fn live_groq_five_calls_in_one_reply() {
        let (text, calls, finish) = run_tools(LIVE_LLAMA).await;
        assert_eq!(text, "");
        let read = "{\"limit\":400,\"offset\":1,\"path\":\"notes.txt\"}";
        assert_eq!(
            calls,
            vec![
                tc("call_live_1", "read_file", read),
                tc(
                    "call_live_2",
                    "read_file",
                    "{\"limit\":400,\"offset\":401,\"path\":\"notes.txt\"}"
                ),
                tc(
                    "call_live_3",
                    "edit_file",
                    "{\"new_text\":\"beta\",\"old_text\":\"alpha\",\"path\":\"notes.txt\",\"replace_all\":false}"
                ),
                tc("call_live_4", "read_file", read),
                tc(
                    "call_live_5",
                    "run_command",
                    "{\"background\":false,\"command\":\"bash scripts/count.sh\",\"timeout_seconds\":120}"
                ),
            ]
        );
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }

    #[tokio::test]
    async fn arguments_that_are_not_a_string_or_object_are_kept_as_raw_json() {
        for (raw, want) in [("[1]", "[1]"), ("5", "5"), ("true", "true")] {
            let body = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"c\",\"function\":{{\"name\":\"list_dir\",\"arguments\":{raw}}}}}]}}}}]}}\n\ndata: [DONE]\n\n"
            );
            let (_, calls, _) = run_tools(&body).await;
            assert_eq!(calls[0].arguments, want);
            assert!(crate::openrouter::tool_calls::parse_arguments(&calls[0].arguments).is_err());
        }
    }
}
