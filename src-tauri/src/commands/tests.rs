use super::auth::{self, StorageChoice};
use super::turn::{TurnResult, TurnStatus};
use super::{chats, models, turn};
use crate::engine::{EngineEvent, UiError};
use crate::openrouter::models::{ModelCache, ModelInfo};
use crate::secrets::{KeyStore, SecretError, SessionStore, StorageKind};
use crate::state::{AppState, EventSink, HttpClients, HttpTimeouts};
use crate::store::{Store, StoredAttachment};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-or-v1-secret-key-0123456789";
const KEY_FREE: &str = include_str!("../../tests/fixtures/key_free.json");
const MODELS: &str = include_str!("../../tests/fixtures/models.json");

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<(String, Value)>>,
}

impl Recorder {
    fn names(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.0.clone())
            .collect()
    }
    fn all(&self) -> Vec<(String, Value)> {
        self.events.lock().unwrap().clone()
    }
}

impl EventSink for Recorder {
    fn emit(&self, event: &str, payload: Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
    }
}

/// A keychain that is missing on this machine.
struct NoKeychain;
impl KeyStore for NoKeychain {
    fn get(&self) -> Result<Option<String>, SecretError> {
        Err(SecretError::Unavailable)
    }
    fn set(&self, _: &str) -> Result<(), SecretError> {
        Err(SecretError::Unavailable)
    }
    fn clear(&self) -> Result<(), SecretError> {
        Err(SecretError::Unavailable)
    }
    fn kind(&self) -> StorageKind {
        StorageKind::Keychain
    }
}

struct Harness {
    state: Arc<AppState>,
    events: Arc<Recorder>,
    _dir: tempfile::TempDir,
}

fn harness_with(origin: &str, keys: Arc<dyn KeyStore>) -> Harness {
    harness_timed(origin, keys, HttpTimeouts::PRODUCTION)
}

fn harness_timed(origin: &str, keys: Arc<dyn KeyStore>, timeouts: HttpTimeouts) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Recorder::default());
    let http =
        HttpClients::from_builder(|| reqwest::Client::builder().no_proxy(), timeouts).unwrap();
    let state = Arc::new(AppState::new(
        http,
        origin.to_string(),
        keys,
        Store::in_memory().unwrap(),
        dir.path().to_path_buf(),
        events.clone(),
    ));
    Harness {
        state,
        events,
        _dir: dir,
    }
}

fn signed_in(origin: &str) -> Harness {
    let keys = SessionStore::default();
    keys.set(KEY).unwrap();
    harness_with(origin, Arc::new(keys))
}

fn signed_out(origin: &str) -> Harness {
    harness_with(origin, Arc::new(SessionStore::default()))
}

async fn mock(server: &MockServer, verb: &str, p: &str, resp: ResponseTemplate) {
    Mock::given(method(verb))
        .and(path(p))
        .respond_with(resp)
        .mount(server)
        .await;
}

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_string())
}

const PAID_REPLY: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"cost\":0.25,\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n\
data: [DONE]\n\n";

type Events = Arc<Mutex<Vec<EngineEvent>>>;

async fn send(
    h: &Harness,
    chat_id: &str,
    text: &str,
    model: &str,
    attachments: Vec<turn::AttachmentInput>,
) -> (Result<TurnResult, UiError>, Vec<EngineEvent>) {
    let events: Events = Arc::default();
    let sink = events.clone();
    let r = turn::send_message(&h.state, chat_id, text, attachments, model, &move |e| {
        sink.lock().unwrap().push(e)
    })
    .await;
    let out = events.lock().unwrap().clone();
    (r, out)
}

#[tokio::test]
async fn sign_in_with_key_rejects_a_401_key_and_saves_nothing() {
    let server = MockServer::start().await;
    mock(&server, "GET", "/api/v1/key", ResponseTemplate::new(401)).await;
    let h = signed_out(&server.uri());
    let err = auth::sign_in_with_key(&h.state, KEY).await.unwrap_err();
    assert_eq!(err.kind, "unauthorized");
    assert_eq!(h.state.keys.get().unwrap(), None);
    assert!(!auth::auth_status(&h.state).signed_in);
    // A rejected candidate key says nothing about a saved key.
    assert!(h.events.names().is_empty());
}

#[tokio::test]
async fn sign_in_with_key_saves_then_auth_status_is_signed_in() {
    let server = MockServer::start().await;
    mock(
        &server,
        "GET",
        "/api/v1/key",
        ResponseTemplate::new(200).set_body_string(KEY_FREE),
    )
    .await;
    let h = signed_out(&server.uri());
    assert_eq!(
        serde_json::to_value(auth::auth_status(&h.state)).unwrap(),
        json!({"signed_in": false, "storage": null})
    );
    auth::sign_in_with_key(&h.state, &format!("  {KEY}\n"))
        .await
        .unwrap();
    assert_eq!(h.state.keys.get().unwrap(), Some(KEY.to_string()));
    assert_eq!(
        serde_json::to_value(auth::auth_status(&h.state)).unwrap(),
        json!({"signed_in": true, "storage": "session"})
    );
}

#[tokio::test]
async fn keychain_unavailable_holds_the_key_until_resolve_key_storage_session() {
    let server = MockServer::start().await;
    mock(
        &server,
        "GET",
        "/api/v1/key",
        ResponseTemplate::new(200).set_body_string(KEY_FREE),
    )
    .await;
    let h = harness_with(&server.uri(), Arc::new(NoKeychain));
    let err = auth::sign_in_with_key(&h.state, KEY).await.unwrap_err();
    assert_eq!(err, UiError::keychain_unavailable());
    assert!(!auth::auth_status(&h.state).signed_in);
    assert!(h.state.pending_key().is_some());

    auth::resolve_key_storage(&h.state, StorageChoice::Session).unwrap();
    assert!(h.state.pending_key().is_none());
    let status = auth::auth_status(&h.state);
    assert!(status.signed_in);
    assert_eq!(status.storage, Some(StorageKind::Session));
    // The engine reads through the same swapped handle.
    let engine_view: Arc<dyn KeyStore> = h.state.keys.clone();
    assert_eq!(engine_view.get().unwrap(), Some(KEY.to_string()));
    // Nothing left to resolve.
    assert!(auth::resolve_key_storage(&h.state, StorageChoice::Session).is_err());
}

#[tokio::test]
async fn resolve_key_storage_file_writes_the_key_file_in_the_data_dir() {
    let server = MockServer::start().await;
    mock(
        &server,
        "GET",
        "/api/v1/key",
        ResponseTemplate::new(200).set_body_string(KEY_FREE),
    )
    .await;
    let h = harness_with(&server.uri(), Arc::new(NoKeychain));
    auth::sign_in_with_key(&h.state, KEY).await.unwrap_err();
    auth::resolve_key_storage(&h.state, StorageChoice::File).unwrap();
    assert_eq!(
        std::fs::read_to_string(h.state.data_dir.join("key")).unwrap(),
        KEY
    );
    assert_eq!(auth::auth_status(&h.state).storage, Some(StorageKind::File));
}

#[tokio::test]
async fn sign_in_with_code_exchanges_with_the_pending_verifier() {
    let server = MockServer::start().await;
    mock(
        &server,
        "POST",
        "/api/v1/auth/keys",
        ResponseTemplate::new(200).set_body_json(json!({"key": KEY})),
    )
    .await;
    let h = signed_out(&server.uri());
    // No sign-in started: nothing to exchange against.
    assert!(auth::sign_in_with_code(&h.state, "c1").await.is_err());

    let url = auth::sign_in_code_url(&h.state).auth_url;
    assert!(url.starts_with("https://openrouter.ai/auth?"));
    assert!(!url.contains("callback_url"));
    auth::sign_in_with_code(&h.state, " c1 ").await.unwrap();
    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    let verifier = sent["code_verifier"].as_str().unwrap();
    assert!(url.contains(&format!(
        "code_challenge={}",
        crate::auth::pkce::challenge_for(verifier)
    )));
    assert_eq!(sent["code"], "c1");
    assert!(auth::auth_status(&h.state).signed_in);
    assert!(h.state.pending_sign_in().is_none());
}

#[tokio::test]
async fn loopback_sign_in_with_no_keychain_finishes_with_keychain_unavailable() {
    let server = MockServer::start().await;
    mock(
        &server,
        "POST",
        "/api/v1/auth/keys",
        ResponseTemplate::new(200).set_body_json(json!({"key": KEY})),
    )
    .await;
    let h = harness_with(&server.uri(), Arc::new(NoKeychain));
    let url = auth::sign_in_start(&h.state, Duration::from_secs(10))
        .await
        .unwrap()
        .auth_url;
    let parsed = url::Url::parse(&url).unwrap();
    let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    let callback = format!("{}?code=c1&state={}", q["callback_url"], q["state"]);
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(callback)
        .send()
        .await
        .unwrap();
    wait_for_event(&h.events, "auth://finished").await;
    assert_eq!(
        h.events.all(),
        vec![(
            "auth://finished".to_string(),
            json!({"ok": false, "reason": "keychain_unavailable"})
        )]
    );
    assert!(h.state.pending_key().is_some());
    auth::resolve_key_storage(&h.state, StorageChoice::Session).unwrap();
    assert!(auth::auth_status(&h.state).signed_in);
}

#[tokio::test]
async fn sign_in_cancel_finishes_with_cancelled() {
    let h = signed_out("http://127.0.0.1:1");
    auth::sign_in_start(&h.state, Duration::from_secs(10))
        .await
        .unwrap();
    auth::sign_in_cancel(&h.state);
    wait_for_event(&h.events, "auth://finished").await;
    assert_eq!(
        h.events.all()[0].1,
        json!({"ok": false, "reason": "cancelled"})
    );
}

async fn wait_for_event(rec: &Recorder, name: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rec.names().iter().any(|n| n == name) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {name} event"));
}

#[tokio::test]
async fn sign_out_clears_the_key_and_keeps_chats() {
    let h = signed_in("http://127.0.0.1:1");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    auth::sign_in_code_url(&h.state);
    auth::sign_out(&h.state).unwrap();
    assert_eq!(h.state.keys.get().unwrap(), None);
    assert!(!auth::auth_status(&h.state).signed_in);
    assert!(h.state.pending_sign_in().is_none());
    assert_eq!(chats::list_chats(&h.state).unwrap(), vec![chat]);
}

/// Holds a key but cannot delete it.
struct UndeletableKey;
impl KeyStore for UndeletableKey {
    fn get(&self) -> Result<Option<String>, SecretError> {
        Ok(Some(KEY.to_string()))
    }
    fn set(&self, _: &str) -> Result<(), SecretError> {
        Ok(())
    }
    fn clear(&self) -> Result<(), SecretError> {
        Err(SecretError::Other("delete refused".into()))
    }
    fn kind(&self) -> StorageKind {
        StorageKind::Keychain
    }
}

#[tokio::test]
async fn sign_out_reports_a_failed_key_delete_after_stopping_everything() {
    let h = harness_with("http://127.0.0.1:1", Arc::new(UndeletableKey));
    assert!(auth::auth_status(&h.state).signed_in);
    auth::sign_in_code_url(&h.state);
    let turn = tokio_util::sync::CancellationToken::new();
    h.state.turns().insert(
        "c".to_string(),
        crate::state::RunningTurn {
            id: 0,
            cancel: turn.clone(),
        },
    );
    let err = auth::sign_out(&h.state).unwrap_err();
    assert_eq!(
        err,
        UiError {
            kind: "other".into(),
            message: "Could not remove your key from this computer. Try signing out again.".into(),
            retryable: true,
        }
    );
    assert!(turn.is_cancelled());
    assert!(h.state.turns().is_empty());
    assert!(h.state.pending_sign_in().is_none());
    assert!(!auth::auth_status(&h.state).signed_in);
}

#[tokio::test]
async fn send_message_persists_user_and_assistant_messages_and_records_usage() {
    let server = MockServer::start().await;
    mock(&server, "POST", "/api/v1/chat/completions", sse(PAID_REPLY)).await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let att = turn::AttachmentInput {
        name: "notes.txt".into(),
        mime: "text/plain".into(),
        data_base64: "aGk=".into(),
    };
    let (r, events) = send(&h, &chat.id, "Hi there", "m/b", vec![att.clone()]).await;
    assert_eq!(r.unwrap(), done());
    assert_eq!(events.last(), Some(&EngineEvent::Done));

    let msgs = chats::get_messages(&h.state, &chat.id).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(
        (msgs[0].role.as_str(), msgs[0].text.as_str()),
        ("user", "Hi there")
    );
    assert_eq!(
        msgs[0].attachments,
        vec![StoredAttachment {
            name: att.name,
            mime: att.mime,
            data_base64: att.data_base64
        }]
    );
    assert_eq!(
        (msgs[1].role.as_str(), msgs[1].text.as_str()),
        ("assistant", "Hello")
    );
    assert!(!msgs[1].interrupted);

    let usage = serde_json::to_value(chats::chat_usage(&h.state, &chat.id).unwrap()).unwrap();
    assert_eq!(usage, json!({"requests": 1, "cost_usd": 0.25}));
    assert_eq!(chats::month_spend(&h.state).unwrap(), 0.25);
    // The chat remembers the model it last used and takes its title.
    let listed = &chats::list_chats(&h.state).unwrap()[0];
    assert_eq!(listed.model_id, "m/b");
    assert_eq!(listed.title, "Hi there");

    // The next turn sends the whole history, in order, as text.
    send(&h, &chat.id, "And again", "m/b", vec![])
        .await
        .0
        .unwrap();
    let reqs = server.received_requests().await.unwrap();
    let body: Value = reqs[1].body_json().unwrap();
    assert_eq!(body["model"], "m/b");
    let sent: Vec<(String, String)> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap().to_string(),
                m["content"][0]["text"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        sent,
        vec![
            ("user".into(), "Hi there".into()),
            ("assistant".into(), "Hello".into()),
            ("user".into(), "And again".into()),
        ]
    );
    assert!(h.state.turns().is_empty());
}

#[tokio::test]
async fn send_message_interrupted_persists_partial_text_with_interrupted_true() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
                data: {\"error\":{\"code\":429,\"message\":\"slow\"}}\n\n";
    mock(&server, "POST", "/api/v1/chat/completions", sse(body)).await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let (r, events) = send(&h, &chat.id, "Hi", "m/a", vec![]).await;
    assert_eq!(
        r.unwrap(),
        TurnResult {
            status: TurnStatus::Interrupted,
            reason: Some("rate_limited".into()),
            error: None,
        }
    );
    assert_eq!(
        events.last(),
        Some(&EngineEvent::Interrupted {
            reason: "rate_limited".into()
        })
    );
    let msgs = chats::get_messages(&h.state, &chat.id).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].text, "Hel");
    assert!(msgs[1].interrupted);
    assert_eq!(chats::chat_usage(&h.state, &chat.id).unwrap().requests, 1);
}

async fn retry(
    h: &Harness,
    chat_id: &str,
    model: &str,
) -> (Result<TurnResult, UiError>, Vec<EngineEvent>) {
    let events: Events = Arc::default();
    let sink = events.clone();
    let r = turn::retry_turn(&h.state, chat_id, model, &move |e| {
        sink.lock().unwrap().push(e)
    })
    .await;
    let out = events.lock().unwrap().clone();
    (r, out)
}

fn roles_and_texts(h: &Harness, chat_id: &str) -> Vec<(String, String, bool)> {
    chats::get_messages(&h.state, chat_id)
        .unwrap()
        .into_iter()
        .map(|m| (m.role, m.text, m.interrupted))
        .collect()
}

#[tokio::test]
async fn retry_turn_after_an_interrupted_reply_replaces_the_partial() {
    let server = MockServer::start().await;
    mock(&server, "POST", "/api/v1/chat/completions", sse(PAID_REPLY)).await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    h.state
        .store
        .append_message(&chat.id, "user", "Hi", &[], false)
        .unwrap();
    h.state
        .store
        .append_message(&chat.id, "assistant", "Partial", &[], true)
        .unwrap();
    let (r, events) = retry(&h, &chat.id, "m/b").await;
    assert_eq!(r.unwrap(), done());
    assert_eq!(events.last(), Some(&EngineEvent::Done));
    assert_eq!(
        roles_and_texts(&h, &chat.id),
        vec![
            ("user".into(), "Hi".into(), false),
            ("assistant".into(), "Hello".into(), false)
        ]
    );
    assert_eq!(chats::list_chats(&h.state).unwrap()[0].model_id, "m/b");
    assert_eq!(chats::chat_usage(&h.state, &chat.id).unwrap().requests, 1);
    // The model saw the question once and no partial reply.
    let reqs = server.received_requests().await.unwrap();
    let body: Value = reqs[0].body_json().unwrap();
    let sent: Vec<(&str, &str)> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap(),
                m["content"][0]["text"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(sent, vec![("user", "Hi")]);
}

#[tokio::test]
async fn retry_turn_after_a_failed_turn_stores_one_reply_and_no_second_user_message() {
    let server = MockServer::start().await;
    mock(&server, "POST", "/api/v1/chat/completions", sse(PAID_REPLY)).await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    h.state
        .store
        .append_message(&chat.id, "user", "Hi", &[], false)
        .unwrap();
    retry(&h, &chat.id, "m/a").await.0.unwrap();
    assert_eq!(
        roles_and_texts(&h, &chat.id),
        vec![
            ("user".into(), "Hi".into(), false),
            ("assistant".into(), "Hello".into(), false)
        ]
    );
}

#[tokio::test]
async fn retry_turn_with_nothing_to_retry_errors_and_changes_nothing() {
    let server = MockServer::start().await;
    mock(&server, "POST", "/api/v1/chat/completions", sse(PAID_REPLY)).await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let expect = UiError {
        kind: "other".into(),
        message: "There is nothing to retry in this chat.".into(),
        retryable: false,
    };
    // Empty chat.
    assert_eq!(retry(&h, &chat.id, "m/a").await.0, Err(expect.clone()));
    // Last message is a completed assistant reply.
    send(&h, &chat.id, "Hi", "m/a", vec![]).await.0.unwrap();
    let before = roles_and_texts(&h, &chat.id);
    assert_eq!(retry(&h, &chat.id, "m/a").await.0, Err(expect));
    assert_eq!(roles_and_texts(&h, &chat.id), before);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn send_message_401_emits_auth_revoked_and_auth_status_becomes_signed_out() {
    let server = MockServer::start().await;
    mock(
        &server,
        "POST",
        "/api/v1/chat/completions",
        ResponseTemplate::new(401).set_body_string("{}"),
    )
    .await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    assert!(auth::auth_status(&h.state).signed_in);
    let (r, events) = send(&h, &chat.id, "Hi", "m/a", vec![]).await;
    let r = r.unwrap();
    assert_eq!(r.status, TurnStatus::Failed);
    assert_eq!(r.reason, None);
    assert_eq!(r.error.map(|e| e.kind), Some("unauthorized".to_string()));
    assert!(matches!(
        events.last(),
        Some(EngineEvent::Failed { error }) if error.kind == "unauthorized"
    ));
    assert_eq!(
        h.events.all(),
        vec![("auth://revoked".to_string(), Value::Null)]
    );
    assert!(!auth::auth_status(&h.state).signed_in);
    // The key is kept until sign-out or a new sign-in replaces it.
    assert_eq!(h.state.keys.get().unwrap(), Some(KEY.to_string()));
    // Chats survive; only the user's message was stored.
    assert_eq!(chats::list_chats(&h.state).unwrap().len(), 1);
    assert_eq!(chats::get_messages(&h.state, &chat.id).unwrap().len(), 1);
}

// ---- timeouts ----

const SHORT: HttpTimeouts = HttpTimeouts {
    connect: Duration::from_secs(2),
    read: Duration::from_millis(300),
    request: Duration::from_millis(300),
};
const STALL: Duration = Duration::from_secs(3);

fn signed_in_short(origin: &str) -> Harness {
    let keys = SessionStore::default();
    keys.set(KEY).unwrap();
    harness_timed(origin, Arc::new(keys), SHORT)
}

/// Run `f` and require it to finish well before the server would answer.
async fn promptly<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_millis(2500), f)
        .await
        .expect("a stalled request must time out")
}

#[tokio::test]
async fn a_stalled_model_list_times_out_as_a_network_error() {
    let server = MockServer::start().await;
    let slow = ResponseTemplate::new(200)
        .set_body_string(MODELS)
        .set_delay(STALL);
    mock(&server, "GET", "/api/v1/models", slow).await;
    let h = signed_in_short(&server.uri());
    let err = promptly(models::list_models(&h.state, true))
        .await
        .unwrap_err();
    assert_eq!(err.kind, "network");
}

#[tokio::test]
async fn a_stalled_key_check_times_out_as_a_network_error() {
    let server = MockServer::start().await;
    let slow = ResponseTemplate::new(200)
        .set_body_string(KEY_FREE)
        .set_delay(STALL);
    mock(&server, "GET", "/api/v1/key", slow).await;
    let h = harness_timed(&server.uri(), Arc::new(SessionStore::default()), SHORT);
    let err = promptly(auth::sign_in_with_key(&h.state, KEY))
        .await
        .unwrap_err();
    assert_eq!(err.kind, "network");
    let h = signed_in_short(&server.uri());
    let err = promptly(auth::key_status(&h.state)).await.unwrap_err();
    assert_eq!(err.kind, "network");
}

#[tokio::test]
async fn a_stalled_code_exchange_times_out_as_a_network_error() {
    let server = MockServer::start().await;
    let slow = ResponseTemplate::new(200)
        .set_body_json(json!({"key": KEY}))
        .set_delay(STALL);
    mock(&server, "POST", "/api/v1/auth/keys", slow).await;
    let h = harness_timed(&server.uri(), Arc::new(SessionStore::default()), SHORT);
    auth::sign_in_code_url(&h.state);
    let err = promptly(auth::sign_in_with_code(&h.state, "c1"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, "network");
}

#[tokio::test]
async fn a_reply_that_goes_silent_fails_as_a_network_error() {
    let server = MockServer::start().await;
    let slow = sse(PAID_REPLY).set_delay(STALL);
    mock(&server, "POST", "/api/v1/chat/completions", slow).await;
    let h = signed_in_short(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let (r, _) = promptly(send(&h, &chat.id, "Hi", "m/a", vec![])).await;
    let r = r.unwrap();
    assert_eq!(r.status, TurnStatus::Failed);
    assert_eq!(r.error.map(|e| e.kind), Some("network".to_string()));
}

#[test]
fn production_timeouts_match_the_ruling() {
    let t = HttpTimeouts::PRODUCTION;
    assert_eq!(t.connect, Duration::from_secs(15));
    assert_eq!(t.read, Duration::from_secs(60));
    assert_eq!(t.request, Duration::from_secs(30));
}

#[tokio::test]
async fn key_status_401_also_revokes() {
    let server = MockServer::start().await;
    mock(&server, "GET", "/api/v1/key", ResponseTemplate::new(401)).await;
    let h = signed_in(&server.uri());
    assert_eq!(
        auth::key_status(&h.state).await.unwrap_err().kind,
        "unauthorized"
    );
    assert_eq!(h.events.names(), vec!["auth://revoked"]);
    assert!(!auth::auth_status(&h.state).signed_in);
}

#[tokio::test]
async fn list_models_falls_back_to_cache_when_the_server_is_down() {
    let server = MockServer::start().await;
    mock(&server, "GET", "/api/v1/models", ResponseTemplate::new(503)).await;
    let h = signed_in(&server.uri());
    // No cache yet: the failure is reported.
    assert_eq!(
        models::list_models(&h.state, true).await.unwrap_err().kind,
        "other"
    );
    let cached = vec![ModelInfo {
        id: "a/b:free".into(),
        name: "B".into(),
        prompt_usd_per_mtok: 0.0,
        completion_usd_per_mtok: 0.0,
        is_free: true,
        supports_tools: true,
        accepts_images: false,
        context_length: 1000,
    }];
    ModelCache::save(&h.state.model_cache_file(), &cached).unwrap();
    assert_eq!(models::list_models(&h.state, true).await.unwrap(), cached);
}

#[tokio::test]
async fn list_models_uses_a_fresh_cache_unless_forced() {
    let server = MockServer::start().await;
    mock(
        &server,
        "GET",
        "/api/v1/models",
        ResponseTemplate::new(200).set_body_string(MODELS),
    )
    .await;
    let h = signed_in(&server.uri());
    let first = models::list_models(&h.state, false).await.unwrap();
    assert_eq!(first.len(), 6);
    assert_eq!(
        ModelCache::load(&h.state.model_cache_file()).unwrap(),
        first
    );
    models::list_models(&h.state, false).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    models::list_models(&h.state, true).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn stop_cancels_the_running_turn_for_that_chat_only() {
    let server = MockServer::start().await;
    mock(
        &server,
        "POST",
        "/api/v1/chat/completions",
        sse(PAID_REPLY).set_delay(Duration::from_millis(400)),
    )
    .await;
    let h = Arc::new(signed_in(&server.uri()));
    let a = chats::create_chat(&h.state, "m/a").unwrap();
    let b = chats::create_chat(&h.state, "m/a").unwrap();
    let run = |id: String| {
        let h = h.clone();
        tokio::spawn(async move { send(&h, &id, "Hi", "m/a", vec![]).await })
    };
    let ta = run(a.id.clone());
    let tb = run(b.id.clone());
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.state.turns().len() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    turn::stop(&h.state, &a.id);
    let (ra, ea) = ta.await.unwrap();
    let (rb, eb) = tb.await.unwrap();
    assert_eq!(
        ra.unwrap(),
        TurnResult {
            status: TurnStatus::Interrupted,
            reason: Some("stopped".into()),
            error: None,
        }
    );
    assert_eq!(rb.unwrap(), done());
    assert_eq!(
        ea,
        vec![EngineEvent::Interrupted {
            reason: "stopped".into()
        }]
    );
    assert_eq!(eb.last(), Some(&EngineEvent::Done));
    assert_eq!(
        chats::get_messages(&h.state, &b.id).unwrap()[1].text,
        "Hello"
    );
    // Stopped before any text: only the user's message is kept.
    assert_eq!(chats::get_messages(&h.state, &a.id).unwrap().len(), 1);
}

#[tokio::test]
async fn no_command_return_value_or_event_payload_contains_the_key() {
    let server = MockServer::start().await;
    mock(
        &server,
        "GET",
        "/api/v1/key",
        ResponseTemplate::new(200).set_body_string(KEY_FREE),
    )
    .await;
    mock(
        &server,
        "POST",
        "/api/v1/auth/keys",
        ResponseTemplate::new(200).set_body_json(json!({"key": KEY})),
    )
    .await;
    mock(
        &server,
        "GET",
        "/api/v1/models",
        ResponseTemplate::new(200).set_body_string(MODELS),
    )
    .await;
    mock(&server, "POST", "/api/v1/chat/completions", sse(PAID_REPLY)).await;

    let mut outputs: Vec<String> = Vec::new();
    let mut record = |v: Value| outputs.push(v.to_string());

    // Paths through the keychain-unavailable hold and both sign-in routes.
    let h = harness_with(&server.uri(), Arc::new(NoKeychain));
    record(to(&auth::sign_in_with_key(&h.state, KEY).await));
    record(Value::String(format!("{:?}", *h.state.pending_key())));
    record(to(&auth::auth_status(&h.state)));
    record(to(&auth::resolve_key_storage(
        &h.state,
        StorageChoice::Session,
    )));
    record(to(&auth::sign_in_code_url(&h.state)));
    record(to(&auth::sign_in_with_code(&h.state, "c").await));
    record(to(
        &auth::sign_in_start(&h.state, Duration::from_secs(5)).await
    ));
    auth::sign_in_cancel(&h.state);
    record(to(&auth::key_status(&h.state).await));
    record(to(&models::list_models(&h.state, true).await));
    let chat = chats::create_chat(&h.state, "m/a");
    record(to(&chat));
    let id = chat.unwrap().id;
    let (r, events) = send(&h, &id, "Hi", "m/a", vec![]).await;
    record(to(&r));
    record(to(&events));
    let (r, events) = retry(&h, &id, "m/a").await;
    record(to(&r));
    record(to(&events));
    record(to(&chats::list_chats(&h.state)));
    record(to(&chats::get_messages(&h.state, &id)));
    record(to(&chats::chat_usage(&h.state, &id)));
    record(to(&chats::month_spend(&h.state)));
    record(to(&chats::rename_chat(&h.state, &id, "t")));
    turn::stop(&h.state, &id);
    record(to(&auth::sign_out(&h.state)));
    record(to(&auth::auth_status(&h.state)));
    record(to(&chats::delete_chat(&h.state, &id)));
    wait_for_event(&h.events, "auth://finished").await;
    for (name, payload) in h.events.all() {
        record(json!([name, payload]));
    }

    // 19 command results plus at least one event.
    assert!(outputs.len() >= 20, "{}", outputs.len());
    for out in &outputs {
        assert!(!out.contains(KEY), "key leaked in {out}");
    }
}

fn done() -> TurnResult {
    TurnResult {
        status: TurnStatus::Done,
        reason: None,
        error: None,
    }
}

#[test]
fn turn_result_json_shape() {
    assert_eq!(
        to(&done()),
        json!({"status": "done", "reason": null, "error": null})
    );
    let failed = TurnResult {
        status: TurnStatus::Failed,
        reason: None,
        error: Some(UiError::other("x")),
    };
    assert_eq!(
        to(&failed),
        json!({
            "status": "failed",
            "reason": null,
            "error": {"kind": "other", "message": "x", "retryable": false}
        })
    );
}

fn to<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap()
}

// ---- attachments ----

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn att(name: &str, mime: &str, bytes: &[u8]) -> turn::AttachmentInput {
    turn::AttachmentInput {
        name: name.into(),
        mime: mime.into(),
        data_base64: b64(bytes),
    }
}

fn cache_model(h: &Harness, id: &str, accepts_images: bool) {
    let m = ModelInfo {
        id: id.into(),
        name: id.into(),
        prompt_usd_per_mtok: 0.0,
        completion_usd_per_mtok: 0.0,
        is_free: true,
        supports_tools: true,
        accepts_images,
        context_length: 1000,
    };
    ModelCache::save(&h.state.model_cache_file(), &[m]).unwrap();
}

async fn last_body(server: &MockServer) -> Value {
    let reqs = server.received_requests().await.unwrap();
    reqs.last().unwrap().body_json().unwrap()
}

async fn chat_server() -> MockServer {
    let server = MockServer::start().await;
    mock(&server, "POST", "/api/v1/chat/completions", sse(PAID_REPLY)).await;
    server
}

#[tokio::test]
async fn a_text_attachment_becomes_a_file_part_after_the_users_text() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let a = att("notes.md", "text/markdown", b"# Title\nbody");
    send(&h, &chat.id, "Read this", "m/a", vec![a])
        .await
        .0
        .unwrap();
    let body = last_body(&server).await;
    let content = &body["messages"][0]["content"];
    assert_eq!(content[0]["text"], "Read this");
    assert_eq!(
        content[1]["text"],
        "File: notes.md\n```\n# Title\nbody\n```"
    );
}

#[tokio::test]
async fn a_fence_inside_the_file_gets_a_longer_outer_fence() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let a = att("a.md", "text/markdown", b"x\n```rust\nfn main() {}\n```\ny");
    send(&h, &chat.id, "", "m/a", vec![a]).await.0.unwrap();
    let body = last_body(&server).await;
    let text = body["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("File: a.md\n````\n"), "{text}");
    assert!(text.ends_with("\n````"), "{text}");
}

#[tokio::test]
async fn an_attachment_only_message_sends_no_empty_text_part() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    cache_model(&h, "m/a", true);
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let atts = vec![
        att("n.txt", "text/plain", b"hello"),
        att("p.png", "image/png", &[1, 2, 3]),
    ];
    send(&h, &chat.id, "", "m/a", atts).await.0.unwrap();
    let body = last_body(&server).await;
    let parts = body["messages"][0]["content"].as_array().unwrap().clone();
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!(
        parts[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("File: n.txt")
    );
    assert_eq!(parts[1]["type"], "image_url");
}

#[tokio::test]
async fn an_image_attachment_becomes_an_image_data_url_part() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    cache_model(&h, "m/a", true);
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let a = att("p.png", "image/png", &[137, 80, 78, 71]);
    let data = a.data_base64.clone();
    send(&h, &chat.id, "look", "m/a", vec![a]).await.0.unwrap();
    let body = last_body(&server).await;
    assert_eq!(
        body["messages"][0]["content"][1]["image_url"]["url"],
        format!("data:image/png;base64,{data}")
    );
}

#[tokio::test]
async fn an_image_for_a_model_that_cannot_read_images_is_rejected_and_nothing_is_stored() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    cache_model(&h, "m/a", false);
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let a = att("p.png", "image/png", &[1, 2, 3]);
    let (r, events) = send(&h, &chat.id, "look", "m/a", vec![a]).await;
    let e = r.unwrap_err();
    assert_eq!(e.kind, "other");
    assert_eq!(
        e.message,
        "This model cannot read images. Pick a model that can, or remove the image."
    );
    assert!(!e.retryable);
    assert!(events.is_empty());
    assert!(chats::get_messages(&h.state, &chat.id).unwrap().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_image_for_a_model_missing_from_the_cache_is_allowed() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    cache_model(&h, "m/other", false);
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let a = att("p.png", "image/png", &[1, 2, 3]);
    send(&h, &chat.id, "look", "m/a", vec![a]).await.0.unwrap();
    assert_eq!(chats::get_messages(&h.state, &chat.id).unwrap().len(), 2);
}

#[tokio::test]
async fn limits_are_enforced_in_the_core_with_the_exact_copy() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let too_many: Vec<_> = (0..6)
        .map(|i| att(&format!("{i}.txt"), "text/plain", b"x"))
        .collect();
    let big_text = att("big.txt", "text/plain", &vec![b'a'; 200_001]);
    let big_image = att("big.png", "image/png", &vec![0u8; 5_000_001]);
    let binary = att("blob.bin", "application/octet-stream", &[0xff, 0xfe, 0x00]);
    let ok_text = att("edge.txt", "text/plain", &vec![b'a'; 200_000]);
    let cases: Vec<(Vec<turn::AttachmentInput>, &str)> = vec![
        (too_many, "You can attach up to 5 files."),
        (vec![big_text], "big.txt is too large to attach."),
        (vec![big_image], "big.png is too large to attach."),
        (vec![binary], "blob.bin is not a text file or image."),
    ];
    for (atts, message) in cases {
        let e = send(&h, &chat.id, "x", "m/a", atts).await.0.unwrap_err();
        assert_eq!(
            (e.kind.as_str(), e.message.as_str(), e.retryable),
            ("other", message, false)
        );
    }
    assert!(chats::get_messages(&h.state, &chat.id).unwrap().is_empty());
    send(&h, &chat.id, "x", "m/a", vec![ok_text])
        .await
        .0
        .unwrap();
}

#[tokio::test]
async fn a_retried_turn_carries_stored_attachments_and_drops_images_for_a_blind_model() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    cache_model(&h, "m/a", true);
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let atts = vec![
        att("n.txt", "text/plain", b"hello"),
        att("p.png", "image/png", &[1, 2, 3]),
    ];
    let stored: Vec<StoredAttachment> = atts.into_iter().map(Into::into).collect();
    h.state
        .store
        .append_message(&chat.id, "user", "see", &stored, false)
        .unwrap();

    retry(&h, &chat.id, "m/a").await.0.unwrap();
    let content = last_body(&server).await["messages"][0]["content"].clone();
    assert_eq!(content.as_array().unwrap().len(), 3);
    assert_eq!(content[2]["type"], "image_url");

    cache_model(&h, "m/a", false);
    let reply = h.state.store.get_messages(&chat.id).unwrap().pop().unwrap();
    h.state.store.delete_message(&chat.id, &reply.id).unwrap();
    retry(&h, &chat.id, "m/a").await.0.unwrap();
    let content = last_body(&server).await["messages"][0]["content"].clone();
    let parts = content.as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "see");
    assert!(
        parts[1]["text"]
            .as_str()
            .unwrap()
            .starts_with("File: n.txt")
    );
}
