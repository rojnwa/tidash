//! Session persistence: OS credential store with an in-memory fallback.

use crate::{Error, Result, Session};
use keyring_core::Entry;
use std::sync::{Arc, Mutex, Once};

pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<Session>>;
    fn save(&self, s: &Session) -> Result<()>;
    fn clear(&self) -> Result<()>;
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
        Self::with_service("tidalfast")
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
    fn load(&self) -> Result<Option<Session>> {
        let Some(n) = self.count()? else {
            return Ok(None);
        };
        let mut json = String::new();
        for i in 0..n {
            let part = self
                .get(&format!("session.{i}"))?
                .ok_or_else(|| Error::Store("stored session is incomplete".into()))?;
            json.push_str(&part);
        }
        serde_json::from_str(&json)
            .map(Some)
            .map_err(|_| Error::Store("stored session is corrupt".into()))
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
}
#[derive(Default)]
pub struct MemoryStore(Mutex<Option<Session>>);

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
}

/// The OS store if a probe `load()` works (nothing stored counts), else memory. `true` = persistent.
pub fn default_store() -> (Arc<dyn TokenStore>, bool) {
    let k = KeyringStore::new();
    if k.load().is_ok() {
        (Arc::new(k), true)
    } else {
        (Arc::new(MemoryStore::default()), false)
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
        let s = KeyringStore::with_service("tidalfast-test");
        let _ = s.clear();
        let r = std::panic::catch_unwind(|| round_trip(&s));
        let _ = s.clear();
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }
}
