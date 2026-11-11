//! Shared app state behind the Tauri commands. Deliberately has no `Debug`
//! impl: nothing here may ever format the key.

use crate::engine::{DirectEngine, Engine};
use crate::secrets::{FileStore, KeyStore, KeychainStore, SecretError, StorageKind};
use crate::skills::library::Library;
use crate::store::Store;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const OPENROUTER_ORIGIN: &str = "https://openrouter.ai";
const KEY_FILE: &str = "key";
const DB_FILE: &str = "chats.db";
const MODEL_CACHE_FILE: &str = "models.json";
/// Under the app data directory: `packs/<id>/` and `yours/`.
const SKILLS_DIR: &str = "skills";

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
    /// The key once read or saved, so the underlying store is touched once
    /// per launch. On macOS every keychain read can raise a password prompt.
    /// It stays inside the Rust core and is dropped on clear and swap.
    remembered: Mutex<Option<String>>,
}

impl SharedKeyStore {
    pub fn new(inner: Arc<dyn KeyStore>) -> Self {
        Self {
            inner: RwLock::new(inner),
            remembered: Mutex::new(None),
        }
    }

    pub fn swap(&self, next: Arc<dyn KeyStore>) {
        *self.inner.write().unwrap_or_else(|e| e.into_inner()) = next;
        *self.remembered() = None;
    }

    fn current(&self) -> Arc<dyn KeyStore> {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn remembered(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.remembered.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl std::fmt::Debug for SharedKeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedKeyStore(..)")
    }
}

impl KeyStore for SharedKeyStore {
    fn get(&self) -> Result<Option<String>, SecretError> {
        if let Some(key) = self.remembered().clone() {
            return Ok(Some(key));
        }
        let key = self.current().get()?;
        self.remembered().clone_from(&key);
        Ok(key)
    }
    fn set(&self, key: &str) -> Result<(), SecretError> {
        self.current().set(key)?;
        *self.remembered() = Some(key.to_string());
        Ok(())
    }
    fn clear(&self) -> Result<(), SecretError> {
        *self.remembered() = None;
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

/// Limits on every request to OpenRouter, so a dropped connection ends in an
/// error instead of waiting forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpTimeouts {
    /// Establishing the connection.
    pub connect: Duration,
    /// The longest silence between two reads; a long stream that keeps
    /// talking is never cut off.
    pub read: Duration,
    /// The whole of a single, non-streaming request.
    pub request: Duration,
}

impl HttpTimeouts {
    pub const PRODUCTION: Self = Self {
        connect: Duration::from_secs(15),
        read: Duration::from_secs(60),
        request: Duration::from_secs(30),
    };
}

/// `stream` carries chat replies; `api` carries the single requests (code
/// exchange, key status, model list) and also has the total-request limit.
pub struct HttpClients {
    pub stream: reqwest::Client,
    pub api: reqwest::Client,
}

impl HttpClients {
    pub fn new(timeouts: HttpTimeouts) -> reqwest::Result<Self> {
        Self::from_builder(reqwest::Client::builder, timeouts)
    }

    /// `base` makes the starting builder for each client.
    pub fn from_builder(
        base: impl Fn() -> reqwest::ClientBuilder,
        t: HttpTimeouts,
    ) -> reqwest::Result<Self> {
        let limited = || base().connect_timeout(t.connect).read_timeout(t.read);
        Ok(Self {
            stream: limited().build()?,
            api: limited().timeout(t.request).build()?,
        })
    }
}

pub struct RunningTurn {
    pub id: u64,
    pub cancel: CancellationToken,
}

pub struct AppState {
    /// For streamed chat replies.
    pub http: reqwest::Client,
    /// For single requests; bounded in total as well.
    pub api_http: reqwest::Client,
    /// `https://openrouter.ai` in production; a mock server in tests.
    pub origin: String,
    pub keys: Arc<SharedKeyStore>,
    pub store: Store,
    pub engine: Arc<dyn Engine>,
    pub data_dir: PathBuf,
    /// Every installed skill: bundled, installed and the user's own.
    pub library: Library,
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
        http: HttpClients,
        origin: String,
        key_store: Arc<dyn KeyStore>,
        store: Store,
        data_dir: PathBuf,
        builtin_skills: PathBuf,
        events: Arc<dyn EventSink>,
    ) -> Self {
        let keys = Arc::new(SharedKeyStore::new(key_store));
        let engine = Arc::new(DirectEngine::new(
            http.stream.clone(),
            api_base(&origin),
            keys.clone(),
        ));
        Self {
            http: http.stream,
            api_http: http.api,
            origin,
            keys,
            store,
            engine,
            library: Library::load(&builtin_skills, &data_dir.join(SKILLS_DIR)),
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
    pub fn open(
        data_dir: PathBuf,
        builtin_skills: PathBuf,
        events: Arc<dyn EventSink>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let store = Store::open(&data_dir.join(DB_FILE)).map_err(|e| e.to_string())?;
        let key_store = startup_key_store(&data_dir);
        let http = HttpClients::new(HttpTimeouts::PRODUCTION).map_err(|e| e.to_string())?;
        Ok(Self::new(
            http,
            OPENROUTER_ORIGIN.to_string(),
            key_store,
            store,
            data_dir,
            builtin_skills,
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

    /// Counts how often the underlying store is read. On macOS every read of
    /// the keychain can raise a password prompt.
    #[derive(Default)]
    struct CountingStore {
        inner: SessionStore,
        reads: std::sync::atomic::AtomicUsize,
    }
    impl KeyStore for CountingStore {
        fn get(&self) -> Result<Option<String>, SecretError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.get()
        }
        fn set(&self, key: &str) -> Result<(), SecretError> {
            self.inner.set(key)
        }
        fn clear(&self) -> Result<(), SecretError> {
            self.inner.clear()
        }
        fn kind(&self) -> StorageKind {
            StorageKind::Keychain
        }
    }

    #[test]
    fn the_stored_key_is_read_once_then_served_from_memory() {
        let store = Arc::new(CountingStore::default());
        store.inner.set("k").unwrap();
        let shared = SharedKeyStore::new(store.clone());
        for _ in 0..5 {
            assert_eq!(shared.get().unwrap(), Some("k".to_string()));
        }
        assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_saved_key_is_served_without_reading_the_store_back() {
        let store = Arc::new(CountingStore::default());
        let shared = SharedKeyStore::new(store.clone());
        shared.set("k").unwrap();
        assert_eq!(shared.get().unwrap(), Some("k".to_string()));
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn clear_and_swap_forget_the_remembered_key() {
        let store = Arc::new(CountingStore::default());
        let shared = SharedKeyStore::new(store.clone());
        shared.set("k").unwrap();
        shared.clear().unwrap();
        assert_eq!(shared.get().unwrap(), None);

        shared.set("k").unwrap();
        shared.swap(Arc::new(SessionStore::default()));
        assert_eq!(shared.get().unwrap(), None);
    }

    #[test]
    fn the_remembered_key_never_shows_in_debug_output() {
        let shared = SharedKeyStore::new(Arc::new(SessionStore::default()));
        shared.set("sk-or-secret").unwrap();
        assert!(!format!("{shared:?}").contains("sk-or-secret"));
    }
}
