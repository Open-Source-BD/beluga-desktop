use super::auth::{self, StorageChoice};
use super::turn::{TurnResult, TurnStatus};
use super::{chat_state, chats, compact, export, folder, models, turn};
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
    harness_full(origin, keys, timeouts, None)
}

/// `builtin_skills` defaults to a folder that does not exist.
fn harness_full(
    origin: &str,
    keys: Arc<dyn KeyStore>,
    timeouts: HttpTimeouts,
    builtin_skills: Option<std::path::PathBuf>,
) -> Harness {
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
        builtin_skills.unwrap_or_else(|| dir.path().join("no-bundled-skills")),
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

/// A keychain that exists but refuses to save, as macOS does when the user
/// declines its password prompt or the entry belongs to another build.
struct RefusingKeychain;
impl KeyStore for RefusingKeychain {
    fn get(&self) -> Result<Option<String>, SecretError> {
        Ok(None)
    }
    fn set(&self, _: &str) -> Result<(), SecretError> {
        Err(SecretError::Other("keychain access denied".into()))
    }
    fn clear(&self) -> Result<(), SecretError> {
        Ok(())
    }
    fn kind(&self) -> StorageKind {
        StorageKind::Keychain
    }
}

#[tokio::test]
async fn a_keychain_that_refuses_to_save_offers_the_storage_choice() {
    let server = MockServer::start().await;
    mock(
        &server,
        "GET",
        "/api/v1/key",
        ResponseTemplate::new(200).set_body_string(KEY_FREE),
    )
    .await;
    let h = harness_with(&server.uri(), Arc::new(RefusingKeychain));
    let err = auth::sign_in_with_key(&h.state, KEY).await.unwrap_err();
    assert_eq!(err.kind, "keychain_unavailable");
    assert!(!auth::auth_status(&h.state).signed_in);

    auth::resolve_key_storage(&h.state, StorageChoice::Session).unwrap();
    let status = auth::auth_status(&h.state);
    assert!(status.signed_in);
    assert_eq!(status.storage, Some(StorageKind::Session));
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
    retry_with(h, chat_id, model, false).await
}

async fn retry_with(
    h: &Harness,
    chat_id: &str,
    model: &str,
    regenerate: bool,
) -> (Result<TurnResult, UiError>, Vec<EngineEvent>) {
    let events: Events = Arc::default();
    let sink = events.clone();
    let r = turn::retry_turn(&h.state, chat_id, model, regenerate, &move |e| {
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
    record(to(&chat_state::chat_state(&h.state, &id, "m/a")));
    record(to(&chat_state::activate_skill(&h.state, &id, "p:a", "m/a")));
    record(to(&chat_state::deactivate_skill(&h.state, &id, "p:a")));
    record(to(&chat_state::chat_fullness(&h.state, &id, "m/a", 10)));
    // The folder commands, with a folder set and not.
    record(to(&folder::folder_files(&h.state, &id, "")));
    record(to(&folder::attach_folder_file(&h.state, &id, "a.txt")));
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("a.txt"), "hello").unwrap();
    record(to(&folder::set_chat_folder(
        &h.state,
        &id,
        Some(tree.path().to_str().unwrap().to_string()),
    )));
    let listed = folder::folder_files(&h.state, &id, "a");
    assert_eq!(listed.as_ref().map(|l| l.total), Ok(1));
    record(to(&listed));
    let attached = folder::attach_folder_file(&h.state, &id, "a.txt");
    assert!(attached.is_ok(), "{attached:?}");
    record(to(&attached));
    record(to(&folder::attach_folder_file(&h.state, &id, "../x")));
    record(to(&folder::set_chat_folder(&h.state, &id, None)));
    // The success path, in a harness that has a skill to activate.
    let f = skills_fx(&server.uri(), &[("a", 10)], &[]);
    let skill_chat = chats::create_chat(&f.h.state, "m/a").unwrap().id;
    let activated = chat_state::activate_skill(&f.h.state, &skill_chat, "p:a", "m/a");
    assert!(activated.is_ok(), "{activated:?}");
    record(to(&activated));
    // Edit, regenerate, compact and export, including the exported file.
    let (r, events) = retry_with(&h, &id, "m/a", true).await;
    record(to(&r));
    record(to(&events));
    send(&h, &id, "Again", "m/a", vec![]).await.0.unwrap();
    let compacted = compact::compact_chat(&h.state, &id, "m/a", Some("k")).await;
    assert_eq!(compacted, Ok(done()));
    record(to(&compacted));
    let file = h._dir.path().join("export.md");
    let exported = export::export_chat(&h.state, &id, file.to_str().unwrap());
    assert!(exported.is_ok(), "{exported:?}");
    record(to(&exported));
    record(Value::String(std::fs::read_to_string(&file).unwrap()));
    let first = chats::get_messages(&h.state, &id).unwrap()[0].id.clone();
    record(to(&chats::rows_after(&h.state, &id, &first)));
    record(to(&chats::take_user_message(&h.state, &id, &first)));
    turn::stop(&h.state, &id);
    record(to(&auth::sign_out(&h.state)));
    record(to(&auth::auth_status(&h.state)));
    record(to(&chats::delete_chat(&h.state, &id)));
    wait_for_event(&h.events, "auth://finished").await;
    for (name, payload) in h.events.all() {
        record(json!([name, payload]));
    }

    // 24 command results plus at least one event.
    assert!(outputs.len() >= 25, "{}", outputs.len());
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

// ---- skills ----

fn real_skills_harness() -> Harness {
    harness_full(
        "http://unused.invalid",
        Arc::new(SessionStore::default()),
        HttpTimeouts::PRODUCTION,
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills")),
    )
}

#[test]
fn list_packs_shape_matches_ipc() {
    let h = real_skills_harness();
    let packs = super::skills::list_packs(&h.state).unwrap();
    assert_eq!(packs.len(), 15);
    let v = serde_json::to_value(&packs[0]).unwrap();
    let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "about", "commit", "error", "id", "license", "on", "origin", "repo", "skills", "title"
        ]
    );
    assert_eq!(v["origin"], "built_in");
    let mut skey: Vec<_> = v["skills"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    skey.sort();
    assert_eq!(
        skey,
        [
            "acts_on_computer",
            "available",
            "body_chars",
            "broken",
            "description",
            "id",
            "name",
            "on"
        ]
    );
    let d = super::skills::get_skill(&h.state, "superpowers:brainstorming").unwrap();
    let dv = serde_json::to_value(&d).unwrap();
    let mut dkeys: Vec<_> = dv.as_object().unwrap().keys().cloned().collect();
    dkeys.sort();
    assert_eq!(
        dkeys,
        [
            "body",
            "files",
            "license",
            "pack_id",
            "pack_title",
            "skill",
            "source_url"
        ]
    );
    assert!(
        d.source_url
            .starts_with("https://github.com/obra/superpowers/tree/")
    );
    assert!(d.source_url.ends_with("/skills/brainstorming"));
    let g = super::skills::get_skill(&h.state, "gstack:gstack").unwrap();
    assert!(
        g.source_url
            .starts_with("https://github.com/garrytan/gstack/tree/")
    );
    assert!(!g.source_url.ends_with("/.") && !g.source_url.ends_with('/'));
    assert_eq!(g.source_url.matches('/').count(), 6, "no trailing dir");
}

#[test]
fn get_skill_of_an_unknown_id_is_a_ui_error() {
    let h = real_skills_harness();
    let e = super::skills::get_skill(&h.state, "nope:nope").unwrap_err();
    assert_eq!(
        e.message,
        "This skill is not available. Check it in Skills."
    );
}

#[test]
fn set_skill_on_persists_across_a_reload() {
    let h = real_skills_harness();
    let id = "superpowers:brainstorming";
    let on = |h: &Harness| super::skills::get_skill(&h.state, id).unwrap().skill.on;
    assert!(on(&h), "on by default");
    super::skills::set_skill_on(&h.state, id, false).unwrap();
    h.state.library.reload();
    assert!(!on(&h));
    super::skills::set_pack_on(&h.state, "superpowers", false).unwrap();
    h.state.library.reload();
    let packs = super::skills::list_packs(&h.state).unwrap();
    assert!(!packs[0].on);
    assert!(super::skills::set_skill_on(&h.state, "nope:nope", true).is_err());
    assert!(super::skills::set_pack_on(&h.state, "nope", true).is_err());
}

#[test]
fn skill_file_returns_a_text_attachment() {
    let h = real_skills_harness();
    let packs = super::skills::list_packs(&h.state).unwrap();
    let h2 = h.state.library.skill(&h.state.store, "gstack:gstack");
    assert!(h2.is_some());
    // Find a skill that lists a file.
    let (id, file) = packs
        .iter()
        .flat_map(|p| &p.skills)
        .find_map(|s| {
            let (_, e) = h.state.library.skill(&h.state.store, &s.id)?;
            e.files.first().map(|f| (s.id.clone(), f.clone()))
        })
        .expect("some bundled skill has a file");
    let a = super::skills::skill_file(&h.state, &id, &file).unwrap();
    assert_eq!(a.name, file.rsplit('/').next().unwrap());
    assert_eq!(a.mime, "text/plain");
    assert!(!a.data_base64.is_empty());
    assert!(super::skills::skill_file(&h.state, &id, "../pack.json").is_err());
}

#[test]
fn skills_status_counts_the_real_tree() {
    let h = real_skills_harness();
    let st = super::skills::skills_status(&h.state).unwrap();
    assert_eq!(
        (st.builtin_error, st.packs, st.skills, st.on),
        (None, 15, 795, 21)
    );
    let v = serde_json::to_value(super::skills::skills_status(&h.state).unwrap()).unwrap();
    let mut k: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    assert_eq!(k, ["builtin_error", "on", "packs", "skills"]);
}

#[test]
fn skills_status_reports_a_missing_builtin_tree() {
    let h = harness_with("http://unused.invalid", Arc::new(SessionStore::default()));
    let st = super::skills::skills_status(&h.state).unwrap();
    assert!(st.builtin_error.is_some());
    assert_eq!(st.packs, 0);
}

// ---- running skills ----

use super::chat_state::{MAX_ACTIVE_SKILLS, SKILL_NOTE};
use crate::openrouter::stream::{Part, Role, WireMessage};

struct SkillsFx {
    h: Harness,
    _skills: tempfile::TempDir,
}

/// The SKILL.md text of a fixture skill: frontmatter plus `len` copies of 'x'.
fn skill_md(name: &str, len: usize) -> String {
    format!(
        "---\nname: {name}\ndescription: d\n---\n{}",
        "x".repeat(len)
    )
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// A signed-in harness whose only built-in pack is `p`, with every listed
/// skill on by default; names in `broken` are marked broken.
fn skills_fx(origin: &str, skills: &[(&str, usize)], broken: &[&str]) -> SkillsFx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("skills");
    let pack = root.join("p");
    let mut entries = Vec::new();
    let mut ids = Vec::new();
    for (name, len) in skills {
        let dir = pack.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), skill_md(name, *len)).unwrap();
        let id = format!("p:{name}");
        entries.push(json!({
            "id": id, "pack": "p", "name": name, "description": "d",
            "dir": format!("skills/{name}"), "acts_on_computer": *name == "acts",
            "body_chars": len, "files": [], "license": null,
            "broken": broken.contains(name).then_some("Missing name"),
        }));
        ids.push(id);
    }
    let manifest = json!({
        "id": "p", "title": "P", "repo": "o/p", "commit": "abc", "license": "MIT",
        "license_files": [], "skills": entries, "skipped": [], "checksum": "x",
    });
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("pack.json"), manifest.to_string()).unwrap();
    std::fs::write(
        root.join("default-skills.json"),
        json!({ "on": ids }).to_string(),
    )
    .unwrap();
    let keys = SessionStore::default();
    keys.set(KEY).unwrap();
    let h = harness_full(origin, Arc::new(keys), HttpTimeouts::PRODUCTION, Some(root));
    SkillsFx { h, _skills: t }
}

/// Caches models as `(id, name, context_length)`.
fn cache_models(h: &Harness, models: &[(&str, &str, u64)]) {
    let list: Vec<ModelInfo> = models
        .iter()
        .map(|(id, name, ctx)| ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
            prompt_usd_per_mtok: 0.0,
            completion_usd_per_mtok: 0.0,
            is_free: true,
            supports_tools: true,
            accepts_images: true,
            context_length: *ctx,
        })
        .collect();
    ModelCache::save(&h.state.model_cache_file(), &list).unwrap();
}

/// `(role, first text part)` of every message in a request body.
fn sent(body: &Value) -> Vec<(String, String)> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap().to_string(),
                m["content"][0]["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn sys(text: &str) -> (String, String) {
    ("system".to_string(), text.to_string())
}

fn user(text: &str) -> (String, String) {
    ("user".to_string(), text.to_string())
}

fn skill_msg(name: &str, len: usize) -> (String, String) {
    sys(&format!("Skill: p:{name}\n\n{}", skill_md(name, len)))
}

fn other(message: &str) -> UiError {
    UiError {
        kind: "other".into(),
        message: message.into(),
        retryable: false,
    }
}

fn active_ids(h: &Harness, chat_id: &str) -> Vec<String> {
    h.state.store.active_skills(chat_id).unwrap()
}

fn notes(h: &Harness, chat_id: &str) -> Vec<String> {
    h.state
        .store
        .get_messages(chat_id)
        .unwrap()
        .into_iter()
        .filter(|m| m.kind == "note")
        .map(|m| m.text)
        .collect()
}

const BIG: &str = "m/big";

#[test]
fn skill_note_is_the_specs_fixed_note() {
    assert_eq!(
        SKILL_NOTE,
        "You are running inside a chat app. You cannot read or change files, run commands, \
browse, or start helpers. When the instructions below tell you to do any of those, say what \
you would do and ask the user to do it and paste the result, or work from the files they \
attach. Do not claim to have done something you cannot do."
    );
    assert_eq!(MAX_ACTIVE_SKILLS, 3);
}

#[test]
fn preamble_is_note_then_skills_in_order_then_summary() {
    let text = |m: &WireMessage| match &m.parts[..] {
        [Part::Text(t)] => (m.role, t.clone()),
        other => panic!("{other:?}"),
    };
    let got: Vec<_> = turn::preamble(&[("p:b", "B"), ("p:a", "A")], Some("S"))
        .iter()
        .map(text)
        .collect();
    assert_eq!(
        got,
        vec![
            (Role::System, SKILL_NOTE.to_string()),
            (Role::System, "Skill: p:b\n\nB".to_string()),
            (Role::System, "Skill: p:a\n\nA".to_string()),
            (
                Role::System,
                "Summary of the earlier conversation:\n\nS".to_string()
            ),
        ]
    );
    let only_summary: Vec<_> = turn::preamble(&[], Some("S")).iter().map(text).collect();
    assert_eq!(
        only_summary,
        vec![(
            Role::System,
            "Summary of the earlier conversation:\n\nS".to_string()
        )]
    );
    assert!(turn::preamble(&[], None).is_empty());
}

#[tokio::test]
async fn no_active_skill_sends_the_history_unchanged() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10)], &[]);
    let chat = chats::create_chat(&f.h.state, BIG).unwrap();
    send(&f.h, &chat.id, "Hi", BIG, vec![]).await.0.unwrap();
    assert_eq!(sent(&last_body(&server).await), vec![user("Hi")]);
}

#[tokio::test]
async fn one_active_skill_is_sent_after_the_fixed_note_and_before_the_conversation() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10)], &[]);
    let chat = chats::create_chat(&f.h.state, BIG).unwrap();
    chat_state::activate_skill(&f.h.state, &chat.id, "p:a", BIG).unwrap();
    send(&f.h, &chat.id, "Hi", BIG, vec![]).await.0.unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![sys(SKILL_NOTE), skill_msg("a", 10), user("Hi")]
    );
}

#[tokio::test]
async fn two_skills_keep_activation_order() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10), ("b", 20)], &[]);
    let chat = chats::create_chat(&f.h.state, BIG).unwrap();
    chat_state::activate_skill(&f.h.state, &chat.id, "p:b", BIG).unwrap();
    chat_state::activate_skill(&f.h.state, &chat.id, "p:a", BIG).unwrap();
    send(&f.h, &chat.id, "Hi", BIG, vec![]).await.0.unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![
            sys(SKILL_NOTE),
            skill_msg("b", 20),
            skill_msg("a", 10),
            user("Hi")
        ]
    );
}

#[tokio::test]
async fn notes_are_never_sent() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10)], &[]);
    let chat = chats::create_chat(&f.h.state, BIG).unwrap();
    f.h.state.store.append_note(&chat.id, "A note").unwrap();
    chat_state::activate_skill(&f.h.state, &chat.id, "p:a", BIG).unwrap();
    send(&f.h, &chat.id, "Hi", BIG, vec![]).await.0.unwrap();
    f.h.state.store.append_note(&chat.id, "Later").unwrap();
    send(&f.h, &chat.id, "Again", BIG, vec![]).await.0.unwrap();
    let got = sent(&last_body(&server).await);
    assert_eq!(
        got,
        vec![
            sys(SKILL_NOTE),
            skill_msg("a", 10),
            user("Hi"),
            ("assistant".into(), "Hello".into()),
            user("Again")
        ]
    );
    assert_eq!(notes(&f.h, &chat.id), ["A note", "Using p:a", "Later"]);
}

#[tokio::test]
async fn a_summary_is_sent_as_a_system_message_and_older_messages_are_not() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10)], &[]);
    let store = &f.h.state.store;
    let chat = chats::create_chat(&f.h.state, BIG).unwrap();
    store
        .append_message(&chat.id, "user", "old", &[], false)
        .unwrap();
    store
        .append_message(&chat.id, "assistant", "old reply", &[], false)
        .unwrap();
    let new = store
        .append_message(&chat.id, "user", "new", &[], false)
        .unwrap();
    store
        .set_summary(&chat.id, "They said old.", new.seq)
        .unwrap();
    retry(&f.h, &chat.id, BIG).await.0.unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![
            sys("Summary of the earlier conversation:\n\nThey said old."),
            user("new")
        ]
    );
    // With a skill active the summary follows the skill messages.
    chat_state::activate_skill(&f.h.state, &chat.id, "p:a", BIG).unwrap();
    send(&f.h, &chat.id, "Hi", BIG, vec![]).await.0.unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![
            sys(SKILL_NOTE),
            skill_msg("a", 10),
            sys("Summary of the earlier conversation:\n\nThey said old."),
            user("new"),
            ("assistant".into(), "Hello".into()),
            user("Hi")
        ]
    );
}

#[test]
fn activate_refuses_a_fourth_an_over_long_an_off_a_broken_and_an_unknown_skill() {
    let f = skills_fx(
        "http://unused.invalid",
        &[
            ("a", 10),
            ("b", 10),
            ("c", 10),
            ("d", 10),
            ("long", 60_001),
            ("off", 10),
            ("bad", 10),
        ],
        &["bad"],
    );
    let s = &f.h.state;
    super::skills::set_skill_on(s, "p:off", false).unwrap();
    let chat = chats::create_chat(s, BIG).unwrap();
    let not_available = other("This skill is not available. Check it in Skills.");
    for id in ["p:off", "p:bad", "p:nope"] {
        assert_eq!(
            chat_state::activate_skill(s, &chat.id, id, BIG),
            Err(not_available.clone()),
            "{id}"
        );
    }
    assert_eq!(
        chat_state::activate_skill(s, &chat.id, "p:long", BIG),
        Err(other("This skill is too long to run in a chat."))
    );
    for id in ["p:a", "p:b", "p:c"] {
        chat_state::activate_skill(s, &chat.id, id, BIG).unwrap();
    }
    assert_eq!(
        chat_state::activate_skill(s, &chat.id, "p:d", BIG),
        Err(other(
            "A chat can have three skills active. Remove one first."
        ))
    );
    assert_eq!(active_ids(&f.h, &chat.id), ["p:a", "p:b", "p:c"]);
    assert_eq!(
        notes(&f.h, &chat.id),
        ["Using p:a", "Using p:b", "Using p:c"]
    );
}

#[test]
fn activate_refuses_a_skill_that_does_not_fit_a_small_context_model() {
    let f = skills_fx("http://unused.invalid", &[("a", 2_000)], &[]);
    cache_models(&f.h, &[("m/small", "Small Model", 1_000)]);
    let chat = chats::create_chat(&f.h.state, "m/small").unwrap();
    assert_eq!(
        chat_state::activate_skill(&f.h.state, &chat.id, "p:a", "m/small"),
        Err(other(
            "This skill is too long for Small Model. Pick a model with a larger limit, \
or remove another active skill."
        ))
    );
    assert!(active_ids(&f.h, &chat.id).is_empty());
    assert!(notes(&f.h, &chat.id).is_empty());
}

#[test]
fn activate_twice_adds_one_note() {
    let f = skills_fx("http://unused.invalid", &[("a", 10)], &[]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, BIG).unwrap();
    let first = chat_state::activate_skill(s, &chat.id, "p:a", BIG).unwrap();
    let second = chat_state::activate_skill(s, &chat.id, "p:a", BIG).unwrap();
    assert_eq!(first, second);
    assert_eq!(active_ids(&f.h, &chat.id), ["p:a"]);
    assert_eq!(notes(&f.h, &chat.id), ["Using p:a"]);
}

#[tokio::test]
async fn switching_to_a_small_model_marks_the_last_activated_skill_too_long_and_leaves_it_out_of_the_wire_history()
 {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 1_000), ("b", 1_000)], &[]);
    // Half the small model's context holds the note and `a` exactly, not `b` as well.
    let small = 2 * ((chars(SKILL_NOTE) + chars(&skill_md("a", 1_000))) / 4) as u64;
    cache_models(
        &f.h,
        &[(BIG, "Big", 1_000_000), ("m/small", "Small", small)],
    );
    let s = &f.h.state;
    let chat = chats::create_chat(s, BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:a", BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:b", BIG).unwrap();

    let state = chat_state::chat_state(s, &chat.id, "m/small").unwrap();
    let flags: Vec<_> = state
        .active_skills
        .iter()
        .map(|k| (k.id.as_str(), k.too_long, k.unavailable))
        .collect();
    assert_eq!(flags, [("p:a", false, false), ("p:b", true, false)]);
    let big = chat_state::chat_state(s, &chat.id, BIG).unwrap();
    assert!(big.active_skills.iter().all(|k| !k.too_long));

    send(&f.h, &chat.id, "Hi", "m/small", vec![])
        .await
        .0
        .unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![sys(SKILL_NOTE), skill_msg("a", 1_000), user("Hi")]
    );
    // Both are still active for the next turn on a larger model.
    assert_eq!(active_ids(&f.h, &chat.id), ["p:a", "p:b"]);
}

#[tokio::test]
async fn a_model_missing_from_the_cache_is_treated_as_fitting() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 50_000)], &[]);
    cache_models(&f.h, &[("m/small", "Small", 100)]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, "m/unknown").unwrap();
    let state = chat_state::activate_skill(s, &chat.id, "p:a", "m/unknown").unwrap();
    assert!(!state.active_skills[0].too_long);
    send(&f.h, &chat.id, "Hi", "m/unknown", vec![])
        .await
        .0
        .unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![sys(SKILL_NOTE), skill_msg("a", 50_000), user("Hi")]
    );
}

#[tokio::test]
async fn retry_turn_sends_the_same_skill_messages_as_send_message() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10), ("b", 30)], &[]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:b", BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:a", BIG).unwrap();
    send(&f.h, &chat.id, "Hi", BIG, vec![]).await.0.unwrap();
    let sent_first = last_body(&server).await["messages"].clone();
    let reply = s.store.get_messages(&chat.id).unwrap().pop().unwrap();
    s.store.delete_message(&chat.id, &reply.id).unwrap();
    retry(&f.h, &chat.id, BIG).await.0.unwrap();
    assert_eq!(last_body(&server).await["messages"], sent_first);
}

#[tokio::test]
async fn retry_turn_ignores_a_note_after_the_last_user_message() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    h.state
        .store
        .append_message(&chat.id, "user", "Hi", &[], false)
        .unwrap();
    h.state.store.append_note(&chat.id, "Using p:a").unwrap();
    assert_eq!(retry(&h, &chat.id, "m/a").await.0.unwrap(), done());
    assert_eq!(sent(&last_body(&server).await), vec![user("Hi")]);
    assert_eq!(
        roles_and_texts(&h, &chat.id),
        vec![
            ("user".into(), "Hi".into(), false),
            ("system".into(), "Using p:a".into(), false),
            ("assistant".into(), "Hello".into(), false)
        ]
    );
}

#[tokio::test]
async fn retry_turn_drops_an_interrupted_reply_that_a_note_follows() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let store = &h.state.store;
    store
        .append_message(&chat.id, "user", "Hi", &[], false)
        .unwrap();
    store
        .append_message(&chat.id, "assistant", "Partial", &[], true)
        .unwrap();
    store.append_note(&chat.id, "Using p:a").unwrap();
    assert_eq!(retry(&h, &chat.id, "m/a").await.0.unwrap(), done());
    assert_eq!(sent(&last_body(&server).await), vec![user("Hi")]);
    assert_eq!(
        roles_and_texts(&h, &chat.id),
        vec![
            ("user".into(), "Hi".into(), false),
            ("system".into(), "Using p:a".into(), false),
            ("assistant".into(), "Hello".into(), false)
        ]
    );
}

#[tokio::test]
async fn retry_turn_after_a_completed_reply_and_a_note_has_nothing_to_retry() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    send(&h, &chat.id, "Hi", "m/a", vec![]).await.0.unwrap();
    h.state.store.append_note(&chat.id, "Using p:a").unwrap();
    let before = roles_and_texts(&h, &chat.id);
    assert_eq!(
        retry(&h, &chat.id, "m/a").await.0,
        Err(other("There is nothing to retry in this chat."))
    );
    assert_eq!(roles_and_texts(&h, &chat.id), before);
    // An only-notes chat has nothing to retry either.
    let empty = chats::create_chat(&h.state, "m/a").unwrap();
    h.state.store.append_note(&empty.id, "Using p:a").unwrap();
    assert_eq!(
        retry(&h, &empty.id, "m/a").await.0,
        Err(other("There is nothing to retry in this chat."))
    );
}

#[test]
fn fullness_of_an_empty_chat_with_a_draft_and_with_active_skills() {
    let f = skills_fx("http://unused.invalid", &[("a", 4_000)], &[]);
    cache_models(&f.h, &[("m/4k", "Four", 4_000)]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, "m/4k").unwrap();
    assert_eq!(
        chat_state::chat_fullness(s, &chat.id, "m/4k", 1_000).unwrap(),
        chat_state::Fullness {
            percent: 6,
            context_length: 4_000
        }
    );
    chat_state::activate_skill(s, &chat.id, "p:a", "m/4k").unwrap();
    let with_skill = chars(SKILL_NOTE) + chars(&skill_md("a", 4_000)) + 1_000;
    let expected = (with_skill as f64 / 4.0 / 4_000.0 * 100.0).round() as u32;
    assert!(expected > 6);
    assert_eq!(
        chat_state::chat_fullness(s, &chat.id, "m/4k", 1_000)
            .unwrap()
            .percent,
        expected
    );
    // Not capped at 100.
    let over = chat_state::chat_fullness(s, &chat.id, "m/4k", 20_000).unwrap();
    assert!(over.percent > 100, "{}", over.percent);
    // A model missing from the cache.
    assert_eq!(
        chat_state::chat_fullness(s, &chat.id, "m/unknown", 1_000).unwrap(),
        chat_state::Fullness {
            percent: 0,
            context_length: 0
        }
    );
}

#[test]
fn fullness_counts_the_summary_sendable_text_attachments_and_images_in_characters() {
    let f = skills_fx("http://unused.invalid", &[("a", 400)], &[]);
    cache_models(&f.h, &[("m/1k", "One", 1_000)]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, "m/1k").unwrap();
    chat_state::activate_skill(s, &chat.id, "p:a", "m/1k").unwrap();
    s.store
        .append_message(&chat.id, "user", &"o".repeat(400), &[], false)
        .unwrap();
    let atts: Vec<StoredAttachment> = vec![
        att("n.txt", "text/plain", "é€x".as_bytes()).into(),
        att("p.png", "image/png", &[1, 2, 3]).into(),
    ];
    let kept = s
        .store
        .append_message(&chat.id, "user", "héllo", &atts, false)
        .unwrap();
    s.store
        .set_summary(&chat.id, &"S".repeat(40), kept.seq)
        .unwrap();
    s.store.append_note(&chat.id, &"n".repeat(400)).unwrap();
    let total = chars(SKILL_NOTE) + chars(&skill_md("a", 400)) + 40 + 5 + 3 + 1_000 + 7;
    let expected = (total as f64 / 4.0 / 1_000.0 * 100.0).round() as u32;
    assert_eq!(
        chat_state::chat_fullness(s, &chat.id, "m/1k", 7).unwrap(),
        chat_state::Fullness {
            percent: expected,
            context_length: 1_000
        }
    );
}

#[test]
fn chat_state_reports_skills_folder_and_compaction() {
    let f = skills_fx("http://unused.invalid", &[("acts", 10), ("a", 10)], &[]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, BIG).unwrap();
    let empty = chat_state::chat_state(s, &chat.id, BIG).unwrap();
    assert_eq!(
        to(&empty),
        json!({"active_skills": [], "folder": null, "compacted": false})
    );
    chat_state::activate_skill(s, &chat.id, "p:acts", BIG).unwrap();
    s.store.set_folder(&chat.id, Some("/tmp/work")).unwrap();
    s.store.set_summary(&chat.id, "S", 0).unwrap();
    let st = chat_state::chat_state(s, &chat.id, BIG).unwrap();
    assert_eq!(
        to(&st),
        json!({
            "active_skills": [{
                "id": "p:acts", "name": "acts", "too_long": false,
                "acts_on_computer": true, "unavailable": false
            }],
            "folder": "/tmp/work",
            "compacted": true
        })
    );
}

#[tokio::test]
async fn an_active_skill_that_is_switched_off_is_kept_reported_unavailable_and_not_sent() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10), ("b", 10)], &[]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:a", BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:b", BIG).unwrap();
    super::skills::set_skill_on(s, "p:a", false).unwrap();
    let st = chat_state::chat_state(s, &chat.id, BIG).unwrap();
    let flags: Vec<_> = st
        .active_skills
        .iter()
        .map(|k| (k.id.as_str(), k.too_long, k.unavailable))
        .collect();
    assert_eq!(flags, [("p:a", false, true), ("p:b", false, false)]);
    send(&f.h, &chat.id, "Hi", BIG, vec![]).await.0.unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![sys(SKILL_NOTE), skill_msg("b", 10), user("Hi")]
    );
    super::skills::set_skill_on(s, "p:b", false).unwrap();
    send(&f.h, &chat.id, "Again", BIG, vec![]).await.0.unwrap();
    let got = sent(&last_body(&server).await);
    assert!(got.iter().all(|(role, _)| role != "system"), "{got:?}");
    assert_eq!(active_ids(&f.h, &chat.id), ["p:a", "p:b"]);
}

#[test]
fn deactivate_removes_the_skill_and_adds_no_note() {
    let f = skills_fx("http://unused.invalid", &[("a", 10), ("b", 10)], &[]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:a", BIG).unwrap();
    chat_state::activate_skill(s, &chat.id, "p:b", BIG).unwrap();
    let st = chat_state::deactivate_skill(s, &chat.id, "p:a").unwrap();
    assert_eq!(st.active_skills.len(), 1);
    assert_eq!(st.active_skills[0].id, "p:b");
    assert_eq!(active_ids(&f.h, &chat.id), ["p:b"]);
    assert_eq!(notes(&f.h, &chat.id), ["Using p:a", "Using p:b"]);
    // Deactivating a skill that is not active changes nothing.
    chat_state::deactivate_skill(s, &chat.id, "p:a").unwrap();
    assert_eq!(active_ids(&f.h, &chat.id), ["p:b"]);
}

#[test]
fn the_four_chat_state_commands_reject_an_unknown_chat() {
    let f = skills_fx("http://unused.invalid", &[("a", 10)], &[]);
    let s = &f.h.state;
    let gone = Err(other("That chat no longer exists."));
    assert_eq!(chat_state::chat_state(s, "nope", BIG), gone);
    assert_eq!(chat_state::activate_skill(s, "nope", "p:a", BIG), gone);
    assert_eq!(chat_state::deactivate_skill(s, "nope", "p:a"), gone);
    assert_eq!(
        chat_state::chat_fullness(s, "nope", BIG, 0),
        Err(other("That chat no longer exists."))
    );
}

#[tokio::test]
async fn a_model_reporting_a_context_length_of_0_is_treated_as_fitting() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10)], &[]);
    cache_models(&f.h, &[("m/zero", "Zero", 0)]);
    let s = &f.h.state;
    let chat = chats::create_chat(s, "m/zero").unwrap();
    let st = chat_state::activate_skill(s, &chat.id, "p:a", "m/zero").unwrap();
    assert!(!st.active_skills[0].too_long);
    let st = chat_state::chat_state(s, &chat.id, "m/zero").unwrap();
    assert!(!st.active_skills[0].too_long);
    send(&f.h, &chat.id, "Hi", "m/zero", vec![])
        .await
        .0
        .unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![sys(SKILL_NOTE), skill_msg("a", 10), user("Hi")]
    );
    assert_eq!(
        chat_state::chat_fullness(s, &chat.id, "m/zero", 1_000).unwrap(),
        chat_state::Fullness {
            percent: 0,
            context_length: 0
        }
    );
}

// ---- edit and resend, regenerate, compact, export ----

use crate::state::RunningTurn;
use crate::store::StoredMessage;
use tokio_util::sync::CancellationToken;

const NO_CHAT: &str = "That chat no longer exists.";
const SUMMARY_ASK: &str = "Summarise the conversation below so it can be continued later. \
Keep decisions, file names, code that matters and open questions. Be brief.";

/// Stores `(role, text)` rows in order; the role "note" stores a note.
fn seed(h: &Harness, chat_id: &str, rows: &[(&str, &str)]) -> Vec<StoredMessage> {
    rows.iter()
        .map(|(role, text)| {
            if *role == "note" {
                h.state.store.append_note(chat_id, text)
            } else {
                h.state
                    .store
                    .append_message(chat_id, role, text, &[], false)
            }
            .unwrap()
        })
        .collect()
}

fn texts(h: &Harness, chat_id: &str) -> Vec<String> {
    chats::get_messages(&h.state, chat_id)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect()
}

fn assistant(text: &str) -> (String, String) {
    ("assistant".to_string(), text.to_string())
}

const FOUR: [(&str, &str); 4] = [
    ("user", "Q1"),
    ("assistant", "A1"),
    ("user", "Q2"),
    ("assistant", "A2"),
];

#[test]
fn take_returns_the_message_deletes_it_and_later_rows_and_keeps_earlier_ones() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let store = &h.state.store;
    let one = att("one.txt", "text/plain", b"one");
    let two = att("two.png", "image/png", &[1, 2, 3]);
    store
        .append_message(&chat.id, "user", "First", &[one.into()], false)
        .unwrap();
    store
        .append_message(&chat.id, "assistant", "Reply", &[], false)
        .unwrap();
    let second = store
        .append_message(&chat.id, "user", "Second", &[two.clone().into()], false)
        .unwrap();
    store
        .append_message(&chat.id, "assistant", "Reply 2", &[], false)
        .unwrap();
    store.append_note(&chat.id, "Using p:a").unwrap();

    let taken = chats::take_user_message(&h.state, &chat.id, &second.id).unwrap();
    assert_eq!(
        taken,
        chats::TakenMessage {
            text: "Second".into(),
            attachments: vec![two],
            removed: 2,
        }
    );
    // The shape the message box sends.
    assert_eq!(
        to(&taken),
        json!({
            "text": "Second",
            "attachments": [{"name": "two.png", "mime": "image/png", "data_base64": b64(&[1, 2, 3])}],
            "removed": 2,
        })
    );
    assert_eq!(texts(&h, &chat.id), ["First", "Reply"]);
}

#[test]
fn take_works_for_an_earlier_user_message() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let rows = seed(&h, &chat.id, &FOUR);
    let taken = chats::take_user_message(&h.state, &chat.id, &rows[0].id).unwrap();
    assert_eq!((taken.text.as_str(), taken.removed), ("Q1", 3));
    assert!(taken.attachments.is_empty());
    assert!(texts(&h, &chat.id).is_empty());
}

#[test]
fn take_leaves_active_skills_active() {
    let f = skills_fx("http://unused.invalid", &[("a", 10)], &[]);
    let chat = chats::create_chat(&f.h.state, BIG).unwrap();
    chat_state::activate_skill(&f.h.state, &chat.id, "p:a", BIG).unwrap();
    let rows = seed(&f.h, &chat.id, &[("user", "Hi"), ("assistant", "Hello")]);
    let taken = chats::take_user_message(&f.h.state, &chat.id, &rows[0].id).unwrap();
    assert_eq!(taken.removed, 1);
    assert_eq!(active_ids(&f.h, &chat.id), ["p:a"]);
    assert_eq!(texts(&f.h, &chat.id), ["Using p:a"]);
}

#[test]
fn take_is_refused_while_a_turn_runs_and_for_anything_but_a_user_message_of_that_chat() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let elsewhere = chats::create_chat(&h.state, "m/a").unwrap();
    let rows = seed(
        &h,
        &chat.id,
        &[
            ("user", "Hi"),
            ("assistant", "Hello"),
            ("note", "Using p:a"),
        ],
    );
    let theirs = seed(&h, &elsewhere.id, &[("user", "Elsewhere")]);
    let before = texts(&h, &chat.id);

    h.state.turns().insert(
        chat.id.clone(),
        RunningTurn {
            id: 99,
            cancel: CancellationToken::new(),
        },
    );
    assert_eq!(
        chats::take_user_message(&h.state, &chat.id, &rows[0].id),
        Err(other(
            "Wait for the reply to finish, or stop it, before editing."
        ))
    );
    h.state.turns().remove(&chat.id);

    let cannot = Err(other("That message cannot be edited."));
    for id in [&rows[1].id, &rows[2].id, &theirs[0].id, &"nope".to_string()] {
        assert_eq!(chats::take_user_message(&h.state, &chat.id, id), cannot);
        assert_eq!(
            chats::rows_after(&h.state, &chat.id, id),
            Err(other("That message cannot be edited."))
        );
    }
    assert_eq!(
        chats::take_user_message(&h.state, "nope", &rows[0].id),
        Err(other(NO_CHAT))
    );
    assert_eq!(
        chats::rows_after(&h.state, "nope", &rows[0].id),
        Err(other(NO_CHAT))
    );
    assert_eq!(texts(&h, &chat.id), before);
    assert_eq!(texts(&h, &elsewhere.id), ["Elsewhere"]);
}

#[test]
fn rows_after_counts_messages_and_notes_after_the_message_and_deletes_nothing() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let rows = seed(
        &h,
        &chat.id,
        &[
            ("user", "One"),
            ("assistant", "R1"),
            ("note", "N"),
            ("user", "Two"),
            ("assistant", "R2"),
        ],
    );
    assert_eq!(chats::rows_after(&h.state, &chat.id, &rows[0].id), Ok(4));
    assert_eq!(chats::rows_after(&h.state, &chat.id, &rows[3].id), Ok(1));
    assert_eq!(texts(&h, &chat.id).len(), 5);
}

#[test]
fn take_before_the_compaction_boundary_clears_the_summary_and_at_it_keeps_it() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let store = &h.state.store;
    let rows = seed(
        &h,
        &chat.id,
        &[
            ("user", "U1"),
            ("assistant", "A1"),
            ("user", "U2"),
            ("assistant", "A2"),
            ("user", "U3"),
            ("assistant", "A3"),
        ],
    );
    store.set_summary(&chat.id, "S", rows[4].seq).unwrap();
    chats::take_user_message(&h.state, &chat.id, &rows[4].id).unwrap();
    assert_eq!(
        store.summary(&chat.id).unwrap(),
        Some(("S".to_string(), rows[4].seq))
    );
    chats::take_user_message(&h.state, &chat.id, &rows[2].id).unwrap();
    assert_eq!(store.summary(&chat.id).unwrap(), None);
    let sendable: Vec<String> = store
        .sendable_messages(&chat.id)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(sendable, ["U1", "A1"]);
}

#[tokio::test]
async fn regenerate_replaces_a_completed_reply_and_stores_no_second_user_message() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    send(&h, &chat.id, "Hi", "m/a", vec![]).await.0.unwrap();
    let (r, events) = retry_with(&h, &chat.id, "m/a", true).await;
    assert_eq!(r.unwrap(), done());
    assert_eq!(events.last(), Some(&EngineEvent::Done));
    assert_eq!(
        roles_and_texts(&h, &chat.id),
        vec![
            ("user".into(), "Hi".into(), false),
            ("assistant".into(), "Hello".into(), false)
        ]
    );
    assert_eq!(sent(&last_body(&server).await), vec![user("Hi")]);
    assert_eq!(chats::chat_usage(&h.state, &chat.id).unwrap().requests, 2);
}

#[tokio::test]
async fn regenerate_with_another_model_writes_that_model_onto_the_chat() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    send(&h, &chat.id, "Hi", "m/a", vec![]).await.0.unwrap();
    retry_with(&h, &chat.id, "m/b", true).await.0.unwrap();
    assert_eq!(last_body(&server).await["model"], "m/b");
    assert_eq!(chats::list_chats(&h.state).unwrap()[0].model_id, "m/b");
}

#[tokio::test]
async fn retry_without_regenerate_on_a_completed_reply_still_has_nothing_to_retry() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    send(&h, &chat.id, "Hi", "m/a", vec![]).await.0.unwrap();
    let before = roles_and_texts(&h, &chat.id);
    let nothing = Err(other("There is nothing to retry in this chat."));
    assert_eq!(retry_with(&h, &chat.id, "m/a", false).await.0, nothing);
    assert_eq!(roles_and_texts(&h, &chat.id), before);
    // Regenerating still needs a reply that answers a user message.
    let empty = chats::create_chat(&h.state, "m/a").unwrap();
    assert_eq!(retry_with(&h, &empty.id, "m/a", true).await.0, nothing);
    let lone = chats::create_chat(&h.state, "m/a").unwrap();
    seed(&h, &lone.id, &[("assistant", "Hello")]);
    assert_eq!(retry_with(&h, &lone.id, "m/a", true).await.0, nothing);
    assert_eq!(texts(&h, &lone.id), ["Hello"]);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

async fn compact(
    h: &Harness,
    chat_id: &str,
    model: &str,
    keep: Option<&str>,
) -> Result<TurnResult, UiError> {
    compact::compact_chat(&h.state, chat_id, model, keep).await
}

#[tokio::test]
async fn compact_refuses_a_chat_with_fewer_than_four_messages_and_sends_nothing() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    seed(
        &h,
        &chat.id,
        &[
            ("user", "Q1"),
            ("assistant", "A1"),
            ("note", "N"),
            ("user", "Q2"),
        ],
    );
    let before = texts(&h, &chat.id);
    assert_eq!(
        compact(&h, &chat.id, "m/a", None).await,
        Err(other("This chat is too short to shorten."))
    );
    assert_eq!(compact(&h, "nope", "m/a", None).await, Err(other(NO_CHAT)));
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(h.state.store.summary(&chat.id).unwrap(), None);
    assert_eq!(texts(&h, &chat.id), before);
}

#[tokio::test]
async fn compact_stores_the_summary_and_note_records_usage_and_later_turns_send_the_summary() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    let rows = seed(&h, &chat.id, &FOUR);
    assert_eq!(compact(&h, &chat.id, "m/a", None).await, Ok(done()));
    assert_eq!(
        sent(&last_body(&server).await),
        vec![
            sys(SUMMARY_ASK),
            user("Q1"),
            assistant("A1"),
            user("Q2"),
            assistant("A2")
        ]
    );
    assert_eq!(
        h.state.store.summary(&chat.id).unwrap(),
        Some(("Hello".to_string(), rows[3].seq + 1))
    );
    assert_eq!(notes(&h, &chat.id), ["Summarised above this line"]);
    assert_eq!(
        to(&chats::chat_usage(&h.state, &chat.id).unwrap()),
        json!({"requests": 1, "cost_usd": 0.25})
    );
    assert!(h.state.turns().is_empty());

    send(&h, &chat.id, "Next", "m/a", vec![]).await.0.unwrap();
    let summary = sys("Summary of the earlier conversation:\n\nHello");
    assert_eq!(
        sent(&last_body(&server).await),
        vec![summary.clone(), user("Next")]
    );
    // Compacting again folds the old summary into the new one.
    send(&h, &chat.id, "More", "m/a", vec![]).await.0.unwrap();
    compact(&h, &chat.id, "m/a", None).await.unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![
            sys(SUMMARY_ASK),
            summary,
            user("Next"),
            assistant("Hello"),
            user("More"),
            assistant("Hello")
        ]
    );
}

#[tokio::test]
async fn compact_leaves_skill_messages_out_and_keeps_skills_active() {
    let server = chat_server().await;
    let f = skills_fx(&server.uri(), &[("a", 10)], &[]);
    let chat = chats::create_chat(&f.h.state, BIG).unwrap();
    chat_state::activate_skill(&f.h.state, &chat.id, "p:a", BIG).unwrap();
    seed(&f.h, &chat.id, &FOUR);
    compact(&f.h, &chat.id, BIG, None).await.unwrap();
    let got = sent(&last_body(&server).await);
    assert!(
        got.iter()
            .all(|(_, t)| !t.starts_with("Skill:") && t != SKILL_NOTE),
        "{got:?}"
    );
    assert_eq!(got.len(), 5);
    assert_eq!(got[0], sys(SUMMARY_ASK));
    assert_eq!(active_ids(&f.h, &chat.id), ["p:a"]);
}

#[tokio::test]
async fn compact_includes_the_keep_text() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    seed(&h, &chat.id, &FOUR);
    compact(&h, &chat.id, "m/a", Some("the database decisions"))
        .await
        .unwrap();
    assert_eq!(
        sent(&last_body(&server).await)[0],
        sys(&format!(
            "{SUMMARY_ASK}\n\nMake sure to keep: the database decisions"
        ))
    );
}

#[tokio::test]
async fn a_failed_compaction_changes_nothing() {
    let server = MockServer::start().await;
    mock(
        &server,
        "POST",
        "/api/v1/chat/completions",
        ResponseTemplate::new(500).set_body_string(r#"{"error":{"message":"boom"}}"#),
    )
    .await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    seed(&h, &chat.id, &FOUR);
    let before = texts(&h, &chat.id);
    let r = compact(&h, &chat.id, "m/a", None).await.unwrap();
    assert_eq!(r.status, TurnStatus::Failed);
    assert_eq!(
        r.error.map(|e| e.message),
        Some("OpenRouter returned an error: boom".to_string())
    );
    assert_eq!(h.state.store.summary(&chat.id).unwrap(), None);
    assert_eq!(texts(&h, &chat.id), before);
    assert!(h.state.turns().is_empty());
}

#[tokio::test]
async fn stop_cancels_a_compaction_and_nothing_changes() {
    let server = MockServer::start().await;
    mock(
        &server,
        "POST",
        "/api/v1/chat/completions",
        sse(PAID_REPLY).set_delay(Duration::from_millis(400)),
    )
    .await;
    let h = Arc::new(signed_in(&server.uri()));
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    seed(&h, &chat.id, &FOUR);
    let before = texts(&h, &chat.id);
    let task = {
        let (h, id) = (h.clone(), chat.id.clone());
        tokio::spawn(async move { compact(&h, &id, "m/a", None).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.state.turns().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    turn::stop(&h.state, &chat.id);
    let r = task.await.unwrap().unwrap();
    assert_eq!(
        r,
        TurnResult {
            status: TurnStatus::Interrupted,
            reason: Some("stopped".into()),
            error: None,
        }
    );
    assert_eq!(h.state.store.summary(&chat.id).unwrap(), None);
    assert_eq!(texts(&h, &chat.id), before);
    assert!(h.state.turns().is_empty());
}

#[tokio::test]
async fn an_over_long_conversation_leaves_out_the_oldest_messages_and_says_so() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    // 60% of 100 tokens is 60 tokens, 240 characters.
    cache_models(&h, &[("m/small", "Small", 100)]);
    let long = "x".repeat(400);
    let rows_of = [
        ("user", long.as_str()),
        ("assistant", "A1"),
        ("user", "Q2"),
        ("assistant", "A2"),
        ("user", "Q3"),
    ];
    let chat = chats::create_chat(&h.state, "m/small").unwrap();
    let rows = seed(&h, &chat.id, &rows_of);
    compact(&h, &chat.id, "m/small", None).await.unwrap();
    assert_eq!(
        sent(&last_body(&server).await),
        vec![
            sys(SUMMARY_ASK),
            assistant("A1"),
            user("Q2"),
            assistant("A2"),
            user("Q3")
        ]
    );
    assert_eq!(
        h.state.store.summary(&chat.id).unwrap(),
        Some((
            "The oldest messages were too long to include in the summary.\n\nHello".to_string(),
            rows[4].seq + 1
        ))
    );
    // A model whose context length is unknown is treated as fitting.
    let unknown = chats::create_chat(&h.state, "m/unknown").unwrap();
    seed(&h, &unknown.id, &rows_of);
    compact(&h, &unknown.id, "m/unknown", None).await.unwrap();
    assert_eq!(sent(&last_body(&server).await)[1], user(&long));
    assert_eq!(
        h.state.store.summary(&unknown.id).unwrap().unwrap().0,
        "Hello"
    );
}

#[test]
fn export_writes_title_roles_notes_attachments_and_the_summary() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    chats::rename_chat(&h.state, &chat.id, "My chat").unwrap();
    let store = &h.state.store;
    let atts: Vec<StoredAttachment> = vec![
        att("notes.txt", "text/plain", b"n").into(),
        att("pic.png", "image/png", &[1]).into(),
    ];
    store
        .append_message(&chat.id, "user", "Hi", &atts, false)
        .unwrap();
    seed(
        &h,
        &chat.id,
        &[
            ("assistant", "Hello\n```rust\nfn x() {}\n```"),
            ("note", "Using p:a"),
        ],
    );
    let later = seed(&h, &chat.id, &[("note", "Summarised above this line")]);
    store
        .set_summary(&chat.id, "Short version", later[0].seq)
        .unwrap();
    seed(&h, &chat.id, &[("user", "Later"), ("assistant", "Sure")]);

    let file = h._dir.path().join("out.md");
    assert_eq!(
        export::export_chat(&h.state, &chat.id, file.to_str().unwrap()),
        Ok(())
    );
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        content,
        "# My chat\n\n\
## You\n\nHi\n\nAttachments: notes.txt, pic.png\n\n\
## m/a\n\nHello\n```rust\nfn x() {}\n```\n\n\
> Using p:a\n\n\
## Summary of earlier messages\n\nShort version\n\n\
> Summarised above this line\n\n\
## You\n\nLater\n\n\
## m/a\n\nSure\n"
    );
}

#[test]
fn export_to_an_unwritable_or_relative_path_is_a_ui_error() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    seed(&h, &chat.id, &[("user", "Hi")]);
    let cannot = Err(other("Could not save the file. Choose another location."));
    let missing = h._dir.path().join("missing").join("out.md");
    for path in [
        missing.to_str().unwrap(),
        h._dir.path().to_str().unwrap(),
        "relative-export.md",
    ] {
        assert_eq!(export::export_chat(&h.state, &chat.id, path), cannot);
    }
    assert!(!missing.exists());
    let abs = h._dir.path().join("x.md");
    assert_eq!(
        export::export_chat(&h.state, "nope", abs.to_str().unwrap()),
        Err(other(NO_CHAT))
    );
    assert!(!abs.exists());
}

#[tokio::test]
async fn compact_is_refused_while_a_turn_runs_and_sends_nothing() {
    let server = chat_server().await;
    let h = signed_in(&server.uri());
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    seed(&h, &chat.id, &FOUR);
    let running = CancellationToken::new();
    h.state.turns().insert(
        chat.id.clone(),
        RunningTurn {
            id: 99,
            cancel: running.clone(),
        },
    );
    assert_eq!(
        compact(&h, &chat.id, "m/a", None).await,
        Err(other(
            "Wait for the reply to finish, or stop it, before shortening the chat."
        ))
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(!running.is_cancelled());
    assert_eq!(h.state.turns().get(&chat.id).map(|t| t.id), Some(99));
    assert_eq!(h.state.store.summary(&chat.id).unwrap(), None);
}

#[test]
fn export_refuses_a_path_that_is_not_markdown_and_accepts_any_case_of_md() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    seed(&h, &chat.id, &[("user", "Hi")]);
    let cannot = Err(other("Could not save the file. Choose another location."));
    for name in ["chat.txt", "chat", "chat.md.exe", "chat.markdown"] {
        let path = h._dir.path().join(name);
        assert_eq!(
            export::export_chat(&h.state, &chat.id, path.to_str().unwrap()),
            cannot,
            "{name}"
        );
        assert!(!path.exists(), "{name}");
    }
    let upper = h._dir.path().join("chat.MD");
    assert_eq!(
        export::export_chat(&h.state, &chat.id, upper.to_str().unwrap()),
        Ok(())
    );
    assert!(upper.exists());
}

#[test]
fn export_writes_the_title_on_one_line() {
    let h = signed_in("http://unused.invalid");
    let chat = chats::create_chat(&h.state, "m/a").unwrap();
    chats::rename_chat(&h.state, &chat.id, "Two\nlines\r\n## Injected\rend").unwrap();
    seed(&h, &chat.id, &[("user", "Hi")]);
    let file = h._dir.path().join("t.md");
    export::export_chat(&h.state, &chat.id, file.to_str().unwrap()).unwrap();
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content.lines().next(), Some("# Two lines  ## Injected end"));
    assert_eq!(content, "# Two lines  ## Injected end\n\n## You\n\nHi\n");
}

#[test]
fn registering_only_if_free_refuses_a_busy_chat_and_registers_a_free_one() {
    let h = signed_in("http://unused.invalid");
    let running = CancellationToken::new();
    h.state.turns().insert(
        "busy".to_string(),
        RunningTurn {
            id: 99,
            cancel: running.clone(),
        },
    );
    assert!(turn::register_turn(&h.state, "busy", turn::Register::IfFree).is_none());
    assert!(!running.is_cancelled());
    assert_eq!(h.state.turns().get("busy").map(|t| t.id), Some(99));

    let (id, cancel) = turn::register_turn(&h.state, "free", turn::Register::IfFree).unwrap();
    assert_eq!(h.state.turns().get("free").map(|t| t.id), Some(id));
    assert!(!cancel.is_cancelled());

    // A normal turn still replaces and cancels the one running.
    let (replacing, _) = turn::register_turn(&h.state, "busy", turn::Register::Replace).unwrap();
    assert!(running.is_cancelled());
    assert_eq!(h.state.turns().get("busy").map(|t| t.id), Some(replacing));
}

// ---- folder files ----

fn folder_chat(h: &Harness) -> (String, tempfile::TempDir) {
    let id = chats::create_chat(&h.state, "m/a").unwrap().id;
    let dir = tempfile::tempdir().unwrap();
    folder::set_chat_folder(
        &h.state,
        &id,
        Some(dir.path().to_str().unwrap().to_string()),
    )
    .unwrap();
    (id, dir)
}

fn decoded(a: &turn::AttachmentInput) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(&a.data_base64)
        .unwrap()
}

#[test]
fn set_chat_folder_refuses_a_missing_path_and_a_file() {
    let h = signed_out("http://unused");
    let id = chats::create_chat(&h.state, "m/a").unwrap().id;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "x").unwrap();
    let refused = Err(UiError::other("That folder could not be opened."));
    for bad in [
        dir.path().join("missing").to_str().unwrap().to_string(),
        file.to_str().unwrap().to_string(),
        "relative/dir".to_string(),
    ] {
        assert_eq!(folder::set_chat_folder(&h.state, &id, Some(bad)), refused);
    }
    assert_eq!(h.state.store.folder(&id).unwrap(), None);
    assert_eq!(
        folder::set_chat_folder(&h.state, "nope", None),
        Err(UiError::other("That chat no longer exists."))
    );

    // A folder is stored by its real path, and `None` clears it.
    let ok = folder::set_chat_folder(&h.state, &id, Some(dir.path().to_str().unwrap().into()));
    assert_eq!(ok, Ok(()));
    let real = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(h.state.store.folder(&id).unwrap().as_deref(), real.to_str());
    assert_eq!(folder::set_chat_folder(&h.state, &id, None), Ok(()));
    assert_eq!(h.state.store.folder(&id).unwrap(), None);
}

#[test]
fn folder_files_with_no_folder_returns_an_empty_list() {
    let h = signed_out("http://unused");
    let id = chats::create_chat(&h.state, "m/a").unwrap().id;
    let list = folder::folder_files(&h.state, &id, "x").unwrap();
    assert_eq!((list.entries.len(), list.total, list.cut), (0, 0, false));
    assert_eq!(
        folder::folder_files(&h.state, "nope", ""),
        Err(UiError::other("That chat no longer exists."))
    );
    assert_eq!(
        folder::attach_folder_file(&h.state, &id, "a.txt"),
        Err(UiError::other("Open a folder first."))
    );
}

#[test]
fn folder_files_shows_at_most_50_entries() {
    let h = signed_out("http://unused");
    let (id, dir) = folder_chat(&h);
    for i in 0..60 {
        std::fs::write(dir.path().join(format!("f{i:02}.txt")), "x").unwrap();
    }
    let list = folder::folder_files(&h.state, &id, "").unwrap();
    assert_eq!((list.entries.len(), list.total), (50, 60));
}

#[test]
fn attach_folder_file_applies_the_200_000_byte_text_limit_with_plan_1_copy() {
    let h = signed_out("http://unused");
    let (id, dir) = folder_chat(&h);
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/ok.txt"), "x".repeat(200_000)).unwrap();
    std::fs::write(dir.path().join("sub/big.txt"), "x".repeat(200_001)).unwrap();
    std::fs::write(dir.path().join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
    std::fs::write(dir.path().join("nul.txt"), b"a\0b").unwrap();
    std::fs::write(dir.path().join("big.png"), vec![0u8; 5_000_001]).unwrap();
    std::fs::write(dir.path().join("pic.JPG"), [1u8, 2, 3]).unwrap();

    let ok = folder::attach_folder_file(&h.state, &id, "sub\\ok.txt").unwrap();
    assert_eq!(
        (ok.name.as_str(), ok.mime.as_str()),
        ("sub/ok.txt", "text/plain")
    );
    assert_eq!(decoded(&ok).len(), 200_000);
    assert_eq!(
        folder::attach_folder_file(&h.state, &id, "sub/big.txt"),
        Err(UiError::other("sub/big.txt is too large to attach."))
    );
    assert_eq!(
        folder::attach_folder_file(&h.state, &id, "big.png"),
        Err(UiError::other("big.png is too large to attach."))
    );
    for bad in ["blob.bin", "nul.txt"] {
        assert_eq!(
            folder::attach_folder_file(&h.state, &id, bad),
            Err(UiError::other(&format!(
                "{bad} is not a text file or image."
            )))
        );
    }
    let pic = folder::attach_folder_file(&h.state, &id, "pic.JPG").unwrap();
    assert_eq!(
        (pic.mime.as_str(), decoded(&pic)),
        ("image/jpeg", vec![1, 2, 3])
    );
    assert_eq!(
        folder::attach_folder_file(&h.state, &id, "../outside.txt"),
        Err(UiError::other("That file is outside the folder."))
    );
}

#[test]
fn attach_folder_file_on_a_directory_returns_its_listing() {
    let h = signed_out("http://unused");
    let (id, dir) = folder_chat(&h);
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/a.txt"), "abc").unwrap();
    let sub = folder::attach_folder_file(&h.state, &id, "sub").unwrap();
    assert_eq!(
        (sub.name.as_str(), sub.mime.as_str()),
        ("sub/ listing.txt", "text/plain")
    );
    assert_eq!(decoded(&sub), b"sub/a.txt\t3");
    let root = folder::attach_folder_file(&h.state, &id, "").unwrap();
    let name = dir.path().file_name().unwrap().to_str().unwrap();
    assert_eq!(root.name, format!("{name}/ listing.txt"));
    assert_eq!(decoded(&root), b"sub/a.txt\t3");
}

#[cfg(unix)]
#[test]
fn attach_folder_file_refuses_a_fifo_promptly() {
    let h = signed_out("http://unused");
    let (id, dir) = folder_chat(&h);
    let fifo = dir.path().join("pipe.txt");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if !made.is_ok_and(|s| s.success()) {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let state = h.state.clone();
    std::thread::spawn(move || {
        let _ = tx.send(folder::attach_folder_file(&state, &id, "pipe.txt"));
    });
    let got = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("attach hung on a FIFO");
    assert_eq!(
        got,
        Err(UiError::other("pipe.txt is not a text file or image."))
    );
}
