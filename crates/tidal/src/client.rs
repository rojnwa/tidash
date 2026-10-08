//! Authenticated HTTP client: v1/v2 requests with the standard params, bearer auth,
//! single-flight refresh, concurrency cap, retries.

use crate::{ClientConfig, Error, Result, Session, TokenStore, refresh};
use reqwest::Method;
use serde::de::DeserializeOwned;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};

const MAX_IN_FLIGHT: usize = 6;
const MAX_RATE_RETRIES: u32 = 2;

pub struct Client {
    http: reqwest::Client,
    cfg: ClientConfig,
    /// Where a refreshed session is saved; `None` once detached. Held through a save.
    store: Arc<std::sync::Mutex<Option<Arc<dyn TokenStore>>>>,
    // Never held across an await.
    session: RwLock<Session>,
    /// Single-flight guard for token refresh.
    refresh_lock: Mutex<()>,
    permits: Semaphore,
}

impl Client {
    pub fn new(cfg: ClientConfig, session: Session, store: Arc<dyn TokenStore>) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(30))
                .build()?,
            cfg,
            store: Arc::new(std::sync::Mutex::new(Some(store))),
            session: RwLock::new(session),
            refresh_lock: Mutex::new(()),
            permits: Semaphore::new(MAX_IN_FLIGHT),
        })
    }

    pub fn session(&self) -> Session {
        self.session
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn config(&self) -> &ClientConfig {
        &self.cfg
    }

    /// Refreshed sessions stay in memory from now on: on logout, a request still in flight
    /// must not write the session back to the store. Waits for a save in progress.
    pub async fn detach_store(&self) {
        let store = Arc::clone(&self.store);
        // A save holds the lock and can block: wait off the async runtime.
        let _ = tokio::task::spawn_blocking(move || {
            *store.lock().unwrap_or_else(|e| e.into_inner()) = None;
        })
        .await;
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let resp = self.request(Method::GET, path, query, None, false).await?;
        Ok(resp.json().await?)
    }

    /// `path` may carry its own query (a module's `viewAll`); `query` is appended to it.
    pub async fn get_v2<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let resp = self.request(Method::GET, path, query, None, true).await?;
        Ok(resp.json().await?)
    }

    /// Writes succeed on any 2xx (TIDAL answers 200 with a body or 204); the body is ignored.
    pub async fn post_form(&self, path: &str, form: &[(&str, String)]) -> Result<()> {
        self.request(Method::POST, path, &[], Some(form), false)
            .await
            .map(drop)
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        self.request(Method::DELETE, path, &[], None, false)
            .await
            .map(drop)
    }

    pub async fn put_v2(
        &self,
        path: &str,
        query: &[(&str, String)],
        form: Option<&[(&str, String)]>,
    ) -> Result<()> {
        self.request(Method::PUT, path, query, form, true)
            .await
            .map(drop)
    }

    /// Every API request: `path` (its own query kept) joined onto the v1 or v2 base, refused
    /// off the base's origin, then `query` and the standard params, the v2 header and `form`.
    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        form: Option<&[(&str, String)]>,
        v2: bool,
    ) -> Result<reqwest::Response> {
        let cfg = &self.cfg;
        let base = if v2 { &cfg.api_v2_base } else { &cfg.api_base };
        let mut url = base
            .join(path)
            .ok()
            .filter(|u| u.origin() == base.origin())
            .ok_or_else(invalid_path)?;
        {
            let mut q = url.query_pairs_mut();
            q.extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())))
                .append_pair("countryCode", &self.session().country_code)
                .append_pair("locale", &cfg.locale)
                .append_pair("deviceType", &cfg.device_type);
            if v2 {
                q.append_pair("platform", &cfg.platform);
            }
        }
        self.send(|_| {
            let mut rb = self.http.request(method.clone(), url.clone());
            if v2 {
                rb = rb.header("x-tidal-client-version", &cfg.client_version);
            }
            if let Some(form) = form {
                rb = rb.form(form);
            }
            Ok(rb)
        })
        .await
    }

    /// Sends `req(token)` with `token` as bearer, with refresh-on-401 and 429/5xx retries. Each
    /// try is built anew, so a retry carries the current token wherever the request has it.
    pub(crate) async fn send(
        &self,
        req: impl Fn(&str) -> Result<reqwest::RequestBuilder>,
    ) -> Result<reqwest::Response> {
        let (mut rate_retries, mut server_retried, mut refreshed) = (0, false, false);
        loop {
            let token = self.session().access_token;
            let rb = req(&token)?;
            let resp = {
                let _permit = self.permits.acquire().await;
                rb.bearer_auth(&token).send().await?
            };
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(2)
                .min(30);
            let code = status.as_u16();
            let err = status_error(code, &resp.text().await?);
            let playback = matches!(
                &err,
                Error::Status {
                    sub_status: Some(4000..=4999),
                    ..
                }
            );
            match code {
                401 if playback => return Err(err),
                401 if !refreshed => {
                    self.refresh_after(&token).await?;
                    refreshed = true;
                }
                401 => {
                    return Err(Error::Auth("still unauthorized after token refresh".into()));
                }
                429 if rate_retries < MAX_RATE_RETRIES => {
                    rate_retries += 1;
                    tokio::time::sleep(Duration::from_secs(retry_after)).await;
                }
                500..=599 if !server_retried => {
                    server_retried = true;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                _ => return Err(err),
            }
        }
    }

    /// Refresh unless another task already replaced `failed_token`.
    pub(crate) async fn refresh_after(&self, failed_token: &str) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let old = self.session();
        if old.access_token != failed_token {
            return Ok(());
        }
        let new = refresh(&self.http, &self.cfg, &old).await?;
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = new.clone();
        // Best-effort, off the async runtime (keyring may block). A failed save can leave the old refresh
        // token stored; fine, the in-memory session keeps working and the next refresh retries the save.
        let (store, saved) = (Arc::clone(&self.store), new);
        let _ = tokio::task::spawn_blocking(move || {
            match &*store.lock().unwrap_or_else(|e| e.into_inner()) {
                Some(s) => s.save(&saved),
                None => Ok(()), // detached
            }
        })
        .await;
        Ok(())
    }
}

fn invalid_path() -> Error {
    Error::Status {
        status: 0,
        sub_status: None,
        message: "invalid API path".into(),
    }
}

/// `subStatus` may arrive as an integer or a whole float (4005.0).
fn status_error(status: u16, body: &str) -> Error {
    let json: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let sub_status = json
        .as_ref()
        .and_then(|j| j.get("subStatus"))
        .and_then(|v| {
            v.as_u64().or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
        })
        .and_then(|n| u32::try_from(n).ok());
    let message = json
        .as_ref()
        .and_then(|j| j.get("userMessage"))
        .and_then(|m| m.as_str())
        .map_or_else(|| body.chars().take(200).collect(), str::to_owned);
    Error::Status {
        status,
        sub_status,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::status_error;
    use crate::Error;

    #[test]
    fn sub_status_parses_int_and_whole_float() {
        for body in [r#"{"subStatus":4005}"#, r#"{"subStatus":4005.0}"#] {
            assert!(matches!(
                status_error(401, body),
                Error::Status {
                    sub_status: Some(4005),
                    ..
                }
            ));
        }
        assert!(matches!(
            status_error(401, r#"{"subStatus":4005.5}"#),
            Error::Status {
                sub_status: None,
                ..
            }
        ));
    }
}
