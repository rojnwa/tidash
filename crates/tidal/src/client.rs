//! Authenticated HTTP client: v1/v2 requests with the standard params, bearer auth,
//! single-flight refresh, concurrency cap, retries, and GETs through the disk cache.

use crate::{ClientConfig, DiskCache, Entry, Error, Result, Session, TokenStore, refresh};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};

const MAX_IN_FLIGHT: usize = 6;
const MAX_RATE_RETRIES: u32 = 2;

/// Clones share the session, store and cache.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
    /// GETs answer from the cache and never send (see [`Client::cache_only`]).
    cache_only: bool,
}

struct Inner {
    http: reqwest::Client,
    cfg: ClientConfig,
    /// Where a refreshed session is saved; `None` once detached. Held through a save.
    store: Arc<std::sync::Mutex<Option<Arc<dyn TokenStore>>>>,
    // Never held across an await.
    session: RwLock<Session>,
    /// Single-flight guard for token refresh.
    refresh_lock: Mutex<()>,
    permits: Semaphore,
    /// The app's offline copy of GET bodies, whatever their `cache-control`; always revalidated.
    cache: Option<Arc<DiskCache>>,
}

impl Client {
    pub fn new(
        cfg: ClientConfig,
        session: Session,
        store: Arc<dyn TokenStore>,
        cache: Option<Arc<DiskCache>>,
    ) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                http: reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .read_timeout(Duration::from_secs(30))
                    .build()?,
                cfg,
                store: Arc::new(std::sync::Mutex::new(Some(store))),
                session: RwLock::new(session),
                refresh_lock: Mutex::new(()),
                permits: Semaphore::new(MAX_IN_FLIGHT),
                cache,
            }),
            cache_only: false,
        })
    }

    /// Same session and cache; its GETs return the cached copy or [`Error::NotCached`] and
    /// never touch the network. Writes still send.
    pub fn cache_only(&self) -> Client {
        Self {
            inner: Arc::clone(&self.inner),
            cache_only: true,
        }
    }

    pub fn session(&self) -> Session {
        self.inner
            .session
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    pub fn config(&self) -> &ClientConfig {
        &self.inner.cfg
    }

    /// Refreshed sessions stay in memory from now on: on logout, a request still in flight
    /// must not write the session back to the store. Waits for a save in progress.
    pub async fn detach_store(&self) {
        let store = Arc::clone(&self.inner.store);
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
        self.get_json(path, query, false, true).await
    }

    /// `path` may carry its own query (a module's `viewAll`); `query` is appended to it.
    pub async fn get_v2<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        self.get_json(path, query, true, true).await
    }

    /// A v1 GET that never reads or writes the cache, for answers that must not be kept
    /// (stream info's short-lived signed URLs). Cache-only, it is [`Error::NotCached`].
    pub(crate) async fn get_uncached<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        self.get_json(path, query, false, false).await
    }

    /// Writes succeed on any 2xx (TIDAL answers 200 with a body or 204); the body is ignored.
    pub async fn post_form(&self, path: &str, form: &[(&str, String)]) -> Result<()> {
        self.write(Method::POST, path, &[], Some(form), false).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        self.write(Method::DELETE, path, &[], None, false).await
    }

    pub async fn put_v2(
        &self,
        path: &str,
        query: &[(&str, String)],
        form: Option<&[(&str, String)]>,
    ) -> Result<()> {
        self.write(Method::PUT, path, query, form, true).await
    }

    /// A GET through the cache (if `use_cache`), keyed by user and full URL (never the token).
    /// A cached ETag goes out as `If-None-Match` and a 304 serves the cached body; a 200
    /// replaces it.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        v2: bool,
        use_cache: bool,
    ) -> Result<T> {
        let url = self.url(path, query, v2)?;
        let key = format!("{} {url}", self.session().user_id);
        let cache = self.inner.cache.clone().filter(|_| use_cache);
        let cached = match &cache {
            Some(c) => {
                let (c, key) = (Arc::clone(c), key.clone());
                tokio::task::spawn_blocking(move || c.read(&key))
                    .await
                    .ok()
                    .flatten()
            }
            None => None,
        };
        // A copy that no longer parses (torn by a crash, an older shape) counts as a miss.
        let cached =
            cached.and_then(|e| Some((e.etag, serde_json::from_slice::<T>(&e.body).ok()?)));
        if self.cache_only {
            return cached.map(|(_, v)| v).ok_or(Error::NotCached);
        }
        let etag = cached.as_ref().and_then(|(etag, _)| etag.as_deref());
        let resp = self.request(Method::GET, &url, None, v2, etag).await?;
        let status = resp.status();
        if status == StatusCode::NOT_MODIFIED
            && let Some((_, v)) = cached
        {
            return Ok(v); // `read` marked it used
        }
        let etag = resp
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = resp.bytes().await?.to_vec();
        let value = serde_json::from_slice(&body)?;
        if status == StatusCode::OK
            && let Some(c) = cache
        {
            // Best-effort: a failed write leaves the old copy, none, or this body under the old
            // ETag (revalidated with a 200, never a 304). Writes of one key never interleave.
            let entry = Entry { etag, body };
            let _ = tokio::task::spawn_blocking(move || c.write(&key, &entry)).await;
        }
        Ok(value)
    }

    async fn write(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        form: Option<&[(&str, String)]>,
        v2: bool,
    ) -> Result<()> {
        let url = self.url(path, query, v2)?;
        self.request(method, &url, form, v2, None).await.map(drop)
    }

    /// `path` (its own query kept) joined onto the v1 or v2 base, refused off the base's
    /// origin, then `query` and the standard params.
    fn url(&self, path: &str, query: &[(&str, String)], v2: bool) -> Result<Url> {
        let cfg = self.config();
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
        Ok(url)
    }

    /// Every API request: `url` from [`Client::url`], the v2 header, `form`, `If-None-Match`.
    async fn request(
        &self,
        method: Method,
        url: &Url,
        form: Option<&[(&str, String)]>,
        v2: bool,
        etag: Option<&str>,
    ) -> Result<reqwest::Response> {
        self.send(|_| {
            let mut rb = self.http().request(method.clone(), url.clone());
            if v2 {
                rb = rb.header("x-tidal-client-version", &self.config().client_version);
            }
            match form {
                Some(form) => rb = rb.form(form),
                // As the official client sends it. hyper leaves `Content-Length` off an empty
                // body, and some front ends answer 411 without one.
                None if method == Method::PUT => {
                    rb = rb
                        .header(CONTENT_TYPE, "application/json")
                        .header(CONTENT_LENGTH, "0");
                }
                None => {}
            }
            if let Some(etag) = etag {
                rb = rb.header(IF_NONE_MATCH, etag);
            }
            Ok(rb)
        })
        .await
    }

    /// Sends `req(token)` with `token` as bearer, with refresh-on-401 and 429/5xx retries. Each
    /// try is built anew, so a retry carries the current token wherever the request has it.
    /// A 304 (only ever an answer to `If-None-Match`) returns like a success.
    pub(crate) async fn send(
        &self,
        req: impl Fn(&str) -> Result<reqwest::RequestBuilder>,
    ) -> Result<reqwest::Response> {
        let (mut rate_retries, mut server_retried, mut refreshed) = (0, false, false);
        loop {
            let token = self.session().access_token;
            let rb = req(&token)?;
            let resp = {
                let _permit = self.inner.permits.acquire().await;
                rb.bearer_auth(&token).send().await?
            };
            let status = resp.status();
            if status.is_success() || status == StatusCode::NOT_MODIFIED {
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
        let _guard = self.inner.refresh_lock.lock().await;
        let old = self.session();
        if old.access_token != failed_token {
            return Ok(());
        }
        let new = refresh(self.http(), self.config(), &old).await?;
        *self
            .inner
            .session
            .write()
            .unwrap_or_else(|e| e.into_inner()) = new.clone();
        // Best-effort, off the async runtime (keyring may block). A failed save can leave the old refresh
        // token stored; fine, the in-memory session keeps working and the next refresh retries the save.
        let (store, saved) = (Arc::clone(&self.inner.store), new);
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
