//! Where the user's API key lives. The key never appears in `Debug` output,
//! logs or error messages.

use crate::brand::KEYCHAIN_SERVICE;
use std::path::PathBuf;
use std::sync::Mutex;

const KEYCHAIN_ACCOUNT: &str = "openrouter";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    Keychain,
    Session,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// No keychain service on this machine.
    Unavailable,
    Other(String),
}

pub trait KeyStore: Send + Sync {
    fn get(&self) -> Result<Option<String>, SecretError>;
    fn set(&self, key: &str) -> Result<(), SecretError>;
    fn clear(&self) -> Result<(), SecretError>;
    fn kind(&self) -> StorageKind;
}

/// OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service).
pub struct KeychainStore;

impl KeychainStore {
    pub fn new() -> Self {
        Self
    }

    fn entry() -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(map_keyring_error)
    }
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for KeychainStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeychainStore")
    }
}

/// Only fixed strings and platform messages are used: some keyring error
/// variants carry the raw secret bytes, which must never be formatted.
fn map_keyring_error(e: keyring::Error) -> SecretError {
    use keyring::Error as E;
    match e {
        E::NoDefaultStore | E::NotSupportedByStore(_) => SecretError::Unavailable,
        E::NoStorageAccess(p) => SecretError::Other(format!("keychain access denied: {p}")),
        E::PlatformFailure(p) => SecretError::Other(format!("keychain failure: {p}")),
        _ => SecretError::Other("keychain error".to_string()),
    }
}

impl KeyStore for KeychainStore {
    fn get(&self) -> Result<Option<String>, SecretError> {
        match Self::entry()?.get_password() {
            Ok(k) => Ok(Some(k)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring_error(e)),
        }
    }

    fn set(&self, key: &str) -> Result<(), SecretError> {
        Self::entry()?.set_password(key).map_err(map_keyring_error)
    }

    fn clear(&self) -> Result<(), SecretError> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring_error(e)),
        }
    }

    fn kind(&self) -> StorageKind {
        StorageKind::Keychain
    }
}

/// Held in memory until the app quits.
#[derive(Default)]
pub struct SessionStore {
    key: Mutex<Option<String>>,
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionStore")
    }
}

impl KeyStore for SessionStore {
    fn get(&self) -> Result<Option<String>, SecretError> {
        Ok(self.key.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn set(&self, key: &str) -> Result<(), SecretError> {
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) = Some(key.to_string());
        Ok(())
    }

    fn clear(&self) -> Result<(), SecretError> {
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    fn kind(&self) -> StorageKind {
        StorageKind::Session
    }
}

/// A file readable by the current user only (mode 0600 on Unix; on Windows it
/// relies on the user-profile folder's default access control).
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore")
            .field("path", &self.path)
            .finish()
    }
}

fn io_err(what: &str, e: std::io::Error) -> SecretError {
    SecretError::Other(format!("{what}: {}", e.kind()))
}

impl KeyStore for FileStore {
    fn get(&self) -> Result<Option<String>, SecretError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) if s.is_empty() => Ok(None),
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("could not read key file", e)),
        }
    }

    fn set(&self, key: &str) -> Result<(), SecretError> {
        use std::io::Write;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_err("could not create key folder", e))?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&self.path)
            .map_err(|e| io_err("could not open key file", e))?;
        // An existing file keeps its old mode, so tighten it explicitly.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io_err("could not restrict key file", e))?;
        }
        file.write_all(key.as_bytes())
            .map_err(|e| io_err("could not write key file", e))
    }

    fn clear(&self) -> Result<(), SecretError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("could not remove key file", e)),
        }
    }

    fn kind(&self) -> StorageKind {
        StorageKind::File
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("beluga-secrets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("key")
    }

    #[test]
    fn session_store_roundtrip() {
        let s = SessionStore::default();
        assert_eq!(s.get().unwrap(), None);
        s.set("sk-test").unwrap();
        assert_eq!(s.get().unwrap(), Some("sk-test".to_string()));
        s.clear().unwrap();
        assert_eq!(s.get().unwrap(), None);
        assert_eq!(s.kind(), StorageKind::Session);
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp_path("mode");
        let s = FileStore::new(p.clone());
        s.set("sk-test").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // Overwriting an existing, looser file tightens it again.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        s.set("sk-test-2").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn file_store_roundtrip_and_clear_removes_file() {
        let p = temp_path("roundtrip");
        let s = FileStore::new(p.clone());
        assert_eq!(s.get().unwrap(), None);
        s.set("sk-test").unwrap();
        assert_eq!(s.get().unwrap(), Some("sk-test".to_string()));
        assert_eq!(s.kind(), StorageKind::File);
        s.clear().unwrap();
        assert!(!p.exists());
        assert_eq!(s.get().unwrap(), None);
        s.clear().unwrap();
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn stores_do_not_leak_the_key_through_debug() {
        let s = SessionStore::default();
        s.set("sk-secret-value").unwrap();
        assert!(!format!("{s:?}").contains("sk-secret-value"));
    }
}
