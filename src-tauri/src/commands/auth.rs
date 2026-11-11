//! Sign-in, key storage and sign-out.

use crate::auth::callback::{self, CallbackError};
use crate::auth::pkce;
use crate::engine::UiError;
use crate::openrouter::error::ApiError;
use crate::openrouter::key_info::{self, KeyStatus};
use crate::secrets::{FileStore, KeyStore, SecretError, SessionStore, StorageKind};
use crate::state::{AppState, HeldKey, PendingSignIn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const AUTH_FINISHED: &str = "auth://finished";
pub const AUTH_REVOKED: &str = "auth://revoked";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuthStatus {
    pub signed_in: bool,
    pub storage: Option<StorageKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuthUrl {
    pub auth_url: String,
}

/// Where to keep the key when the keychain is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageChoice {
    Session,
    File,
}

fn could_not_save() -> UiError {
    UiError::other("Could not save the key on this computer.")
}

pub fn auth_status(s: &AppState) -> AuthStatus {
    let has_key = matches!(s.keys.get(), Ok(Some(_)));
    let signed_in = has_key && !s.signed_out.load(Ordering::SeqCst);
    AuthStatus {
        signed_in,
        storage: signed_in.then(|| s.keys.kind()),
    }
}

/// OpenRouter rejected the saved key: report signed out and tell the web
/// view once. The key itself is kept until sign-out or a new sign-in.
pub(crate) fn revoke(s: &AppState) {
    if !s.signed_out.swap(true, Ordering::SeqCst) {
        s.emit(AUTH_REVOKED, Value::Null);
    }
}

/// Every API failure made with the saved key passes through here.
pub(crate) fn note(s: &AppState, e: UiError) -> UiError {
    if e.kind == "unauthorized" {
        revoke(s);
    }
    e
}

fn stop_pending_sign_in(s: &AppState) {
    if let Some(old) = s.pending_sign_in().take() {
        old.stop(true);
    }
}

fn begin_sign_in(s: &AppState) -> (pkce::Pkce, String, u64, CancellationToken, Arc<AtomicBool>) {
    let pkce = pkce::generate();
    let state_param = uuid::Uuid::new_v4().simple().to_string();
    let id = s.next_id();
    let cancel = CancellationToken::new();
    let quiet = Arc::new(AtomicBool::new(false));
    let pending = PendingSignIn {
        id,
        verifier: pkce.verifier.clone(),
        cancel: cancel.clone(),
        quiet: quiet.clone(),
    };
    if let Some(old) = s.pending_sign_in().replace(pending) {
        old.stop(true);
    }
    (pkce, state_param, id, cancel, quiet)
}

/// Save a key that OpenRouter accepted. With no keychain service, the key is
/// held in memory until `resolve_key_storage`.
fn save_key(s: &AppState, key: String) -> Result<(), UiError> {
    stop_pending_sign_in(s);
    match s.keys.set(&key) {
        Ok(()) => {
            *s.pending_key() = None;
            s.signed_out.store(false, Ordering::SeqCst);
            Ok(())
        }
        Err(SecretError::Unavailable) => {
            *s.pending_key() = Some(HeldKey(key));
            Err(UiError::keychain_unavailable())
        }
        Err(SecretError::Other(_)) => Err(could_not_save()),
    }
}

async fn complete_with_code(s: &AppState, code: &str, verifier: &str) -> Result<(), UiError> {
    let key = pkce::exchange_code(&s.http, &s.origin, code, verifier)
        .await
        .map_err(|e| match e {
            ApiError::Network(_) => UiError::from(e),
            _ => {
                UiError::other("OpenRouter did not accept this sign-in code. Start sign-in again.")
            }
        })?;
    save_key(s, key)
}

/// Bind the loopback listener and wait for the browser in the background.
/// The caller opens `auth_url` in the browser.
pub async fn sign_in_start(s: &Arc<AppState>, timeout: Duration) -> Result<AuthUrl, UiError> {
    let listener = callback::bind()
        .await
        .map_err(|_| UiError::other("Could not start sign-in on this computer."))?;
    let (pkce, state_param, id, cancel, quiet) = begin_sign_in(s);
    let auth_url = pkce::auth_url(&pkce.challenge, Some(&listener.callback_url), &state_param);
    let s = s.clone();
    tokio::spawn(async move {
        let result = listener.wait_for_code(&state_param, timeout, cancel).await;
        if result == Err(CallbackError::Cancelled) && quiet.load(Ordering::SeqCst) {
            return;
        }
        {
            let mut pending = s.pending_sign_in();
            if pending.as_ref().is_some_and(|p| p.id == id) {
                *pending = None;
            }
        }
        let reason = match result {
            Ok(code) => match complete_with_code(&s, &code, &pkce.verifier).await {
                Ok(()) => None,
                Err(e) if e.kind == "keychain_unavailable" => Some("keychain_unavailable"),
                Err(_) => Some("failed"),
            },
            Err(CallbackError::Denied) => Some("denied"),
            Err(CallbackError::TimedOut) => Some("timed_out"),
            Err(CallbackError::Cancelled) => Some("cancelled"),
            Err(CallbackError::Io(_)) => Some("failed"),
        };
        s.emit(
            AUTH_FINISHED,
            json!({ "ok": reason.is_none(), "reason": reason }),
        );
    });
    Ok(AuthUrl { auth_url })
}

/// A sign-in for the paste-a-code path: no listener, no callback URL.
pub fn sign_in_code_url(s: &AppState) -> AuthUrl {
    let (pkce, state_param, ..) = begin_sign_in(s);
    AuthUrl {
        auth_url: pkce::auth_url(&pkce.challenge, None, &state_param),
    }
}

pub fn sign_in_cancel(s: &AppState) {
    if let Some(p) = s.pending_sign_in().take() {
        p.stop(false);
    }
}

pub async fn sign_in_with_code(s: &AppState, code: &str) -> Result<(), UiError> {
    let verifier = s
        .pending_sign_in()
        .as_ref()
        .map(|p| p.verifier.clone())
        .ok_or_else(|| UiError::other("No sign-in is in progress. Start sign-in again."))?;
    complete_with_code(s, code.trim(), &verifier).await
}

pub async fn sign_in_with_key(s: &AppState, key: &str) -> Result<(), UiError> {
    let key = key.trim();
    // A rejected candidate key says nothing about the saved one, so this
    // failure does not mark the session revoked.
    key_info::key_status(&s.http, &s.api_base(), key)
        .await
        .map_err(UiError::from)?;
    save_key(s, key.to_string())
}

pub fn resolve_key_storage(s: &AppState, choice: StorageChoice) -> Result<(), UiError> {
    let Some(held) = s.pending_key().take() else {
        return Err(UiError::other("There is no key waiting to be saved."));
    };
    let store: Arc<dyn KeyStore> = match choice {
        StorageChoice::Session => Arc::new(SessionStore::default()),
        StorageChoice::File => Arc::new(FileStore::new(s.key_file())),
    };
    if store.set(&held.0).is_err() {
        *s.pending_key() = Some(held);
        return Err(could_not_save());
    }
    s.keys.swap(store);
    s.signed_out.store(false, Ordering::SeqCst);
    Ok(())
}

/// Forget the key and stop everything in flight. Chats are kept.
pub fn sign_out(s: &AppState) {
    s.signed_out.store(true, Ordering::SeqCst);
    let _ = s.keys.clear();
    *s.pending_key() = None;
    stop_pending_sign_in(s);
    for (_, turn) in s.turns().drain() {
        turn.cancel.cancel();
    }
}

pub async fn key_status(s: &AppState) -> Result<KeyStatus, UiError> {
    let key = match s.keys.get() {
        Ok(Some(k)) => k,
        Ok(None) => return Err(ApiError::Unauthorized.into()),
        Err(_) => return Err(UiError::key_unreadable()),
    };
    key_info::key_status(&s.http, &s.api_base(), &key)
        .await
        .map_err(|e| note(s, e.into()))
}
