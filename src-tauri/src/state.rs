//! Shared app state behind the Tauri commands. Deliberately has no `Debug`
//! impl: nothing here may ever format the key.

use crate::engine::{DirectEngine, Engine};
use crate::secrets::{FileStore, KeyStore, KeychainStore, SecretError, StorageKind};
use crate::store::Store;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use tokio_util::sync::CancellationToken;

pub const OPENROUTER_ORIGIN: &str = "https://openrouter.ai";
const KEY_FILE: &str = "key";
const DB_FILE: &str = "chats.db";
const MODEL_CACHE_FILE: &str = "models.json";

/// Delivers named events to the web view.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

impl<R: tauri::Runtime> EventSink for tauri::AppHandle<R> {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = tauri::Emitter::emit(self, event, payload);
    }
}

/// The one handle every reader of the key goes through. Swapping the inner
/// store (after a `resolve_key_storage` choice) takes effect everywhere.
pub struct SharedKeyStore {
    inner: RwLock<Arc<dyn KeyStore>>,
}

impl SharedKeyStore {
    pub fn new(inner: Arc<dyn KeyStore>) -> Self {
        Self {
            inner: RwLock::new(inner),
        }
    }

    pub fn swap(&self, next: Arc<dyn KeyStore>) {
        *self.inner.write().unwrap_or_else(|e| e.into_inner()) = next;
    }

    fn current(&self) -> Arc<dyn KeyStore> {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl KeyStore for SharedKeyStore {
    fn get(&self) -> Result<Option<String>, SecretError> {
        self.current().get()
    }
    fn set(&self, key: &str) -> Result<(), SecretError> {
        self.current().set(key)
    }
    fn clear(&self) -> Result<(), SecretError> {
        self.current().clear()
    }
    fn kind(&self) -> StorageKind {
        self.current().kind()
    }
}

/// A key held in memory; its `Debug` output never shows the value.
pub struct HeldKey(pub String);

impl std::fmt::Debug for HeldKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeldKey(..)")
    }
}

/// One sign-in in progress. Serves both the loopback callback and a code the
/// user pastes, which is exchanged with the same verifier.
pub struct PendingSignIn {
    pub id: u64,
    pub verifier: String,
    /// Stops the loopback listener, if one is running.
    pub cancel: CancellationToken,
    /// Set before cancelling when no `auth://finished` should follow.
    pub quiet: Arc<AtomicBool>,
}

impl PendingSignIn {
    pub fn stop(&self, quiet: bool) {
        self.quiet.store(quiet, Ordering::SeqCst);
        self.cancel.cancel();
    }
}

pub struct RunningTurn {
    pub id: u64,
    pub cancel: CancellationToken,
}

pub struct AppState {
    pub http: reqwest::Client,
    /// `https://openrouter.ai` in production; a mock server in tests.
    pub origin: String,
    pub keys: Arc<SharedKeyStore>,
    pub store: Store,
    pub engine: Arc<dyn Engine>,
    pub data_dir: PathBuf,
    pub events: Arc<dyn EventSink>,
    pub turns: Mutex<HashMap<String, RunningTurn>>,
    pub pending_sign_in: Mutex<Option<PendingSignIn>>,
    /// A validated key the keychain could not take, awaiting `resolve_key_storage`.
    pub pending_key: Mutex<Option<HeldKey>>,
    /// Set on sign-out or when OpenRouter rejects the key; cleared by a new sign-in.
    pub signed_out: AtomicBool,
    next_id: AtomicU64,
}

impl AppState {
    pub fn new(
        http: reqwest::Client,
        origin: String,
        key_store: Arc<dyn KeyStore>,
        store: Store,
        data_dir: PathBuf,
        events: Arc<dyn EventSink>,
    ) -> Self {
        let keys = Arc::new(SharedKeyStore::new(key_store));
        let engine = Arc::new(DirectEngine::new(
            http.clone(),
            api_base(&origin),
            keys.clone(),
        ));
        Self {
            http,
            origin,
            keys,
            store,
            engine,
            data_dir,
            events,
            turns: Mutex::new(HashMap::new()),
            pending_sign_in: Mutex::new(None),
            pending_key: Mutex::new(None),
            signed_out: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        }
    }

    /// Production state: the database in the app data directory, and the key
    /// in the keychain unless an earlier "file" choice left a key file there.
    pub fn open(data_dir: PathBuf, events: Arc<dyn EventSink>) -> Result<Self, String> {
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let store = Store::open(&data_dir.join(DB_FILE)).map_err(|e| e.to_string())?;
        let key_store = startup_key_store(&data_dir);
        Ok(Self::new(
            reqwest::Client::new(),
            OPENROUTER_ORIGIN.to_string(),
            key_store,
            store,
            data_dir,
            events,
        ))
    }

    pub fn api_base(&self) -> String {
        api_base(&self.origin)
    }

    pub fn key_file(&self) -> PathBuf {
        key_file(&self.data_dir)
    }

    pub fn model_cache_file(&self) -> PathBuf {
        self.data_dir.join(MODEL_CACHE_FILE)
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn emit(&self, event: &str, payload: serde_json::Value) {
        self.events.emit(event, payload);
    }

    pub fn turns(&self) -> MutexGuard<'_, HashMap<String, RunningTurn>> {
        lock(&self.turns)
    }

    pub fn pending_sign_in(&self) -> MutexGuard<'_, Option<PendingSignIn>> {
        lock(&self.pending_sign_in)
    }

    pub fn pending_key(&self) -> MutexGuard<'_, Option<HeldKey>> {
        lock(&self.pending_key)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn api_base(origin: &str) -> String {
    format!("{}/api/v1", origin.trim_end_matches('/'))
}

fn key_file(data_dir: &Path) -> PathBuf {
    data_dir.join(KEY_FILE)
}

pub fn startup_key_store(data_dir: &Path) -> Arc<dyn KeyStore> {
    let file = key_file(data_dir);
    if file.exists() {
        Arc::new(FileStore::new(file))
    } else {
        Arc::new(KeychainStore::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SessionStore;

    #[test]
    fn startup_uses_the_key_file_when_one_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(startup_key_store(dir.path()).kind(), StorageKind::Keychain);
        std::fs::write(dir.path().join("key"), "k").unwrap();
        assert_eq!(startup_key_store(dir.path()).kind(), StorageKind::File);
    }

    #[test]
    fn swap_changes_what_every_holder_reads() {
        let shared = Arc::new(SharedKeyStore::new(Arc::new(SessionStore::default())));
        let as_dyn: Arc<dyn KeyStore> = shared.clone();
        let next = SessionStore::default();
        next.set("k2").unwrap();
        shared.swap(Arc::new(next));
        assert_eq!(as_dyn.get().unwrap(), Some("k2".to_string()));
    }
}
