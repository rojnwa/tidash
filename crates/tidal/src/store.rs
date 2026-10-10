//! Session persistence: OS credential store with an in-memory fallback.

use crate::{Error, Result, Session};
use keyring_core::Entry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};

/// Calls block (keychain prompts, D-Bus), and the Linux store runs a tokio runtime of its own,
/// which panics on an async runtime thread: call them from plain threads or `spawn_blocking`.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<Session>>;
    fn save(&self, s: &Session) -> Result<()>;
    /// The TIDAL session only: the secrets stay.
    fn clear(&self) -> Result<()>;

    /// An integration's secret by name (`"lastfm"`, `"listenbrainz"`). By default a store
    /// keeps none: it reads none and refuses to store one.
    fn secret(&self, _name: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn set_secret(&self, _name: &str, _value: &str) -> Result<()> {
        Err(Error::Store("this store keeps no secrets".into()))
    }
    fn delete_secret(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}

/// Error text only; keyring errors carry no secret values.
fn store_err(e: impl std::fmt::Display) -> Error {
    Error::Store(e.to_string())
}

/// Registers the platform's native store once per process.
fn init_native_store() -> Result<()> {
    static INIT: Once = Once::new();
    static RESULT: Mutex<Option<String>> = Mutex::new(None);
    INIT.call_once(|| {
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new();
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(target_os = "linux")]
        let store = zbus_secret_service_keyring_store::Store::new();
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        match store {
            Ok(s) => keyring_core::set_default_store(s),
            Err(e) => *RESULT.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string()),
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            *RESULT.lock().unwrap_or_else(|p| p.into_inner()) = Some("no native store".into());
        }
    });
    match &*RESULT.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(e) => Err(Error::Store(e.clone())),
        None => Ok(()),
    }
}

/// Windows caps a secret at 2560 bytes (UTF-16 doubles it), so the session JSON is split into
/// chunks of `CHUNK` chars stored as `session.0`, `session.1`, ...; `session` holds the chunk count.
const CHUNK: usize = 1000;

pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self::with_service("tidash")
    }

    pub(crate) fn with_service(service: &str) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, user: &str) -> Result<Entry> {
        init_native_store()?;
        Entry::new(&self.service, user).map_err(store_err)
    }

    fn get(&self, user: &str) -> Result<Option<String>> {
        match self.entry(user)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_err(e)),
        }
    }

    fn delete(&self, user: &str) -> Result<()> {
        match self.entry(user)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_err(e)),
        }
    }

    fn discard(&self) -> Result<Option<Session>> {
        self.clear()?;
        Ok(None)
    }

    fn count(&self) -> Result<Option<usize>> {
        self.get("session")?
            .map(|n| {
                n.parse()
                    .map_err(|_| Error::Store("stored session is corrupt".into()))
            })
            .transpose()
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore for KeyringStore {
    /// A corrupt or incomplete session is removed and reads as none, so the next login
    /// persists. A store that cannot be read (locked, access denied) is an error, and what it
    /// holds stays for the next try.
    fn load(&self) -> Result<Option<Session>> {
        let Some(count) = self.get("session")? else {
            return Ok(None);
        };
        let Ok(n) = count.parse::<usize>() else {
            return self.discard();
        };
        let mut json = String::new();
        for i in 0..n {
            let Some(part) = self.get(&format!("session.{i}"))? else {
                return self.discard();
            };
            json.push_str(&part);
        }
        match serde_json::from_str(&json) {
            Ok(s) => Ok(Some(s)),
            Err(_) => self.discard(),
        }
    }

    fn save(&self, s: &Session) -> Result<()> {
        let json = serde_json::to_string(s).map_err(store_err)?;
        let chars: Vec<char> = json.chars().collect();
        let parts: Vec<String> = chars.chunks(CHUNK).map(|c| c.iter().collect()).collect();
        let old = self.count().ok().flatten().unwrap_or(0);
        for (i, part) in parts.iter().enumerate() {
            self.entry(&format!("session.{i}"))?
                .set_password(part)
                .map_err(store_err)?;
        }
        self.entry("session")?
            .set_password(&parts.len().to_string())
            .map_err(store_err)?;
        for i in parts.len()..old {
            self.delete(&format!("session.{i}"))?;
        }
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        if let Some(n) = self.count().ok().flatten() {
            for i in 0..n {
                self.delete(&format!("session.{i}"))?;
            }
        }
        self.delete("session")
    }

    /// Kept as user `secret-<name>`, apart from the session's entries.
    fn secret(&self, name: &str) -> Result<Option<String>> {
        self.get(&format!("secret-{name}"))
    }

    fn set_secret(&self, name: &str, value: &str) -> Result<()> {
        self.entry(&format!("secret-{name}"))?
            .set_password(value)
            .map_err(store_err)
    }

    fn delete_secret(&self, name: &str) -> Result<()> {
        self.delete(&format!("secret-{name}"))
    }
}

/// The session, and the secrets by name.
#[derive(Default)]
pub struct MemoryStore(Mutex<Option<Session>>, Mutex<HashMap<String, String>>);

impl TokenStore for MemoryStore {
    fn load(&self) -> Result<Option<Session>> {
        Ok(self.0.lock().unwrap_or_else(|p| p.into_inner()).clone())
    }

    fn save(&self, s: &Session) -> Result<()> {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(s.clone());
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }

    fn secret(&self, name: &str) -> Result<Option<String>> {
        Ok(self.secrets().get(name).cloned())
    }

    fn set_secret(&self, name: &str, value: &str) -> Result<()> {
        self.secrets().insert(name.into(), value.into());
        Ok(())
    }

    fn delete_secret(&self, name: &str) -> Result<()> {
        self.secrets().remove(name);
        Ok(())
    }
}

impl MemoryStore {
    fn secrets(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.1.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The OS store and the session its probe read, so nobody reads it again (each read can raise
/// a macOS keychain prompt). When the OS store cannot be read: memory, and why. Blocks, as
/// [`TokenStore`] calls do.
pub fn default_store() -> (Arc<dyn TokenStore>, Result<Option<Session>>) {
    let k = KeyringStore::new();
    match k.load() {
        Ok(session) => (Arc::new(k), Ok(session)),
        Err(e) => (Arc::new(MemoryStore::default()), Err(e)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sample() -> Session {
        Session {
            access_token: "a".repeat(1200),
            refresh_token: "r".repeat(1200),
            expires_at: 1_800_000_000,
            user_id: 42,
            country_code: "US".into(),
        }
    }

    /// Logging out of TIDAL leaves the integrations connected.
    fn secrets_survive_clear_in(s: &dyn TokenStore) {
        s.save(&sample()).unwrap();
        s.set_secret("listenbrainz", "lb-token").unwrap();
        s.set_secret("lastfm", "sk").unwrap();
        s.clear().unwrap();
        assert!(s.load().unwrap().is_none());
        assert_eq!(
            s.secret("listenbrainz").unwrap().as_deref(),
            Some("lb-token")
        );
        s.set_secret("listenbrainz", "newer").unwrap();
        assert_eq!(s.secret("listenbrainz").unwrap().as_deref(), Some("newer"));
        s.delete_secret("listenbrainz").unwrap();
        s.delete_secret("listenbrainz").unwrap(); // gone already: fine
        assert!(s.secret("listenbrainz").unwrap().is_none());
        assert_eq!(s.secret("lastfm").unwrap().as_deref(), Some("sk"));
        s.delete_secret("lastfm").unwrap();
    }

    #[test]
    fn secrets_survive_clear() {
        secrets_survive_clear_in(&MemoryStore::default());
    }

    #[test]
    #[ignore = "touches the real OS credential store"]
    fn keyring_secrets_survive_clear() {
        let s = KeyringStore::with_service("tidash-test-secrets");
        let r = std::panic::catch_unwind(|| secrets_survive_clear_in(&s));
        let _ = s.clear();
        for name in ["listenbrainz", "lastfm"] {
            let _ = s.delete_secret(name);
        }
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }

    fn round_trip(s: &dyn TokenStore) {
        assert!(s.load().unwrap().is_none());
        s.clear().unwrap();
        s.save(&sample()).unwrap();
        assert!(s.load().unwrap() == Some(sample()));
        let mut newer = sample();
        newer.access_token = "b".repeat(1200);
        s.save(&newer).unwrap();
        assert!(s.load().unwrap() == Some(newer));
        s.clear().unwrap();
        assert!(s.load().unwrap().is_none());
    }

    #[test]
    fn memory_store_round_trip() {
        round_trip(&MemoryStore::default());
    }

    #[test]
    #[ignore = "touches the real OS credential store"]
    fn keyring_store_round_trip() {
        let s = KeyringStore::with_service("tidash-test");
        let _ = s.clear();
        let r = std::panic::catch_unwind(|| round_trip(&s));
        let _ = s.clear();
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    #[ignore = "touches the real OS credential store"]
    fn corrupt_session_reads_as_none_and_is_removed() {
        let s = KeyringStore::with_service("tidash-test-corrupt");
        let corrupt: [&[(&str, &str)]; 3] = [
            &[("session", "two")],
            &[("session", "2"), ("session.0", "{")],
            &[("session", "1"), ("session.0", "{oops")],
        ];
        let r = std::panic::catch_unwind(|| {
            for items in corrupt {
                for (user, value) in items {
                    s.entry(user).unwrap().set_password(value).unwrap();
                }
                assert!(s.load().unwrap().is_none(), "{items:?}");
                assert!(s.get("session").unwrap().is_none(), "{items:?}");
                assert!(s.get("session.0").unwrap().is_none(), "{items:?}");
            }
        });
        for user in ["session", "session.0", "session.1"] {
            let _ = s.delete(user);
        }
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }
}
