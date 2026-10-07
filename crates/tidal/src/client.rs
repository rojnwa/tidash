//! Authenticated HTTP client: bearer auth, countryCode, single-flight refresh, concurrency cap, retries.

use crate::{ClientConfig, Error, Result, Session, TokenStore, refresh};
use serde::de::DeserializeOwned;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};

const MAX_IN_FLIGHT: usize = 6;
const MAX_RATE_RETRIES: u32 = 2;

pub struct Client {
    http: reqwest::Client,
    cfg: ClientConfig,
    store: Arc<dyn TokenStore>,
    // Never held across an await.
    session: RwLock<Session>,
    /// Single-flight guard for token refresh.
    refresh_lock: Mutex<()>,
    permits: Semaphore,
}

impl Client {
    pub fn new(cfg: ClientConfig, session: Session, store: Arc<dyn TokenStore>) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().build()?,
            cfg,
            store,
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

    pub fn config(&self) -> &ClientConfig {
        &self.cfg
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let mut url = self
            .cfg
            .api_base
            .join(path)
            .map_err(|e| Error::Auth(format!("bad request path: {e}")))?;
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())))
            .append_pair("countryCode", &self.session().country_code);
        Ok(self.send(self.http.get(url)).await?.json().await?)
    }

    /// Sends with auth, refresh-on-401 and 429/5xx retries. A non-cloneable request is sent once, unretried.
    pub(crate) async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let (mut rate_retries, mut server_retried, mut refreshed) = (0, false, false);
        let mut once = Some(req);
        let template = once.as_ref().and_then(|r| r.try_clone());
        loop {
            let token = self.session().access_token;
            let rb = match (&template, once.take()) {
                (Some(t), _) => t.try_clone(),
                (None, r) => r,
            }
            .ok_or_else(|| Error::Auth("request cannot be retried".into()))?;
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
            let retryable = template.is_some();
            let code = status.as_u16();
            let err = status_error(code, &resp.text().await?);
            match code {
                401 if retryable
                    && !refreshed
                    && !matches!(
                        &err,
                        Error::Status {
                            sub_status: Some(4000..=4999),
                            ..
                        }
                    ) =>
                {
                    self.refresh_after(&token).await?;
                    refreshed = true;
                }
                401 if refreshed => {
                    return Err(Error::Auth("still unauthorized after token refresh".into()));
                }
                429 if retryable && rate_retries < MAX_RATE_RETRIES => {
                    rate_retries += 1;
                    tokio::time::sleep(Duration::from_secs(retry_after)).await;
                }
                500..=599 if retryable && !server_retried => {
                    server_retried = true;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                _ => return Err(err),
            }
        }
    }

    /// Refresh unless another task already replaced `failed_token`.
    async fn refresh_after(&self, failed_token: &str) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let old = self.session();
        if old.access_token != failed_token {
            return Ok(());
        }
        let new = refresh(&self.http, &self.cfg, &old).await?;
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = new.clone();
        // Best-effort: TIDAL doesn't rotate refresh tokens, so a failed save loses nothing.
        let _ = self.store.save(&new);
        Ok(())
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
