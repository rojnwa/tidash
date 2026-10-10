//! Scrobbling: the rule, the plays it sends, and the queue that keeps the ones not yet taken
//! for a later flush. One `Scrobbler` on the backend runtime sends to every connected service.

mod lastfm;
mod listenbrainz;

pub use lastfm::Keys;

use crate::backend::store_call;
use crate::i18n::tr;
use crate::settings::write_atomic;
use listenbrainz::Kind;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tidal::TokenStore;

/// A play counts once it has played this long, or half the track if that is less.
const AT_MOST: Duration = Duration::from_secs(240);
/// Plays kept for later; past this many the oldest go.
const MAX_QUEUED: usize = 500;

/// Whether a play of a `duration_s` track, `played` so far (pauses excluded), is a listen:
/// tracks over 30 s, once half of them or 4 minutes played, whichever comes first.
pub fn due(duration_s: u32, played: Duration) -> bool {
    duration_s > 30 && played >= (Duration::from_secs(duration_s.into()) / 2).min(AT_MOST)
}

/// One play, as the services take it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listen {
    pub track_id: u64,
    pub title: String,
    /// The first artist.
    pub artist: String,
    pub album: String,
    pub duration_s: u32,
    pub track_number: u32,
    /// When the play started, in unix seconds.
    pub started_at: u64,
}

impl Listen {
    pub fn of(t: &tidal::Track, started_at: u64) -> Listen {
        Listen {
            track_id: t.id,
            title: t.title.clone(),
            artist: t
                .artists
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default(),
            album: t.album.title.clone(),
            duration_s: t.duration,
            track_number: t.track_number,
            started_at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Service {
    LastFm,
    ListenBrainz,
}

/// Every service, in the order a play goes out to them.
const SERVICES: [Service; 2] = [Service::LastFm, Service::ListenBrainz];

impl Service {
    /// Its secret's name in the credential store.
    fn secret(self) -> &'static str {
        match self {
            Service::LastFm => "lastfm",
            Service::ListenBrainz => "listenbrainz",
        }
    }

    /// The most listens one send takes.
    fn batch(self) -> usize {
        match self {
            Service::LastFm => lastfm::MAX_SCROBBLES,
            Service::ListenBrainz => listenbrainz::MAX_LISTENS,
        }
    }
}

/// The services' API roots, and Last.fm's keys.
#[derive(Clone)]
pub struct Apis {
    pub listenbrainz: Url,
    pub lastfm: Url,
    /// `None`: Last.fm is never connected (`Keys::compiled`).
    pub lastfm_keys: Option<Keys>,
}

impl Default for Apis {
    fn default() -> Self {
        let url = |s: &str| Url::parse(s).expect("valid constant URL");
        Apis {
            listenbrainz: url("https://api.listenbrainz.org/"),
            lastfm: url("https://ws.audioscrobbler.com/2.0/"),
            lastfm_keys: Keys::compiled(),
        }
    }
}

/// What a service made of a send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Done,
    /// Not now (offline, rate-limited, the service down): kept for a later flush.
    Retry,
    /// The token is no good: kept until the user connects again.
    Reconnect,
    /// Refused as malformed: dropped, never sent again.
    Drop,
}

/// What a send leaves the backend to do.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Plays wait in the queue for a later flush.
    pub retry: bool,
    /// These services refused their token, which is forgotten now.
    pub refused: Vec<Service>,
}

/// A queued play for one service. The queue holds no secrets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    service: Service,
    listen: Listen,
}

/// Sends "now playing" and listens to every connected service; a clone shares it all.
#[derive(Clone)]
pub struct Scrobbler(Arc<Inner>);

struct Inner {
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    apis: Apis,
    /// Where the queue is saved; `None`: it lives in memory only.
    path: Option<PathBuf>,
    /// The plays not yet taken, oldest first. Held across a flush, so adds and flushes go one
    /// at a time, in order.
    queue: tokio::sync::Mutex<Vec<Entry>>,
    /// The services the settings show connected, each with its token as last read or set
    /// (`None` inside: not read yet, or none stored; read again). A service missing is not
    /// connected: nothing is queued or sent for it, and its token is never read. A token found
    /// is kept, as each read of the OS store can raise a keychain prompt.
    tokens: Mutex<HashMap<Service, Option<String>>>,
}

/// `scrobbles.json` in the per-user data dir; `None` without a home directory.
pub fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "tidash").map(|p| p.data_dir().join("scrobbles.json"))
}

/// The queue saved at `path`; empty when there is none or it cannot be read. Blocks.
fn load(path: &Path) -> Vec<Entry> {
    let read = std::fs::read(path).and_then(|b| Ok(serde_json::from_slice(&b)?));
    read.unwrap_or_else(|e| {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("scrobble queue at {} unreadable: {e}", path.display());
        }
        Vec::new()
    })
}

impl Scrobbler {
    /// Sends to the `connected` services, those the settings name a user for; Last.fm only
    /// with its keys. Reads the queue saved at `queue`, if any. Blocks.
    pub fn new(
        http: reqwest::Client,
        store: Arc<dyn TokenStore>,
        queue: Option<PathBuf>,
        apis: Apis,
        connected: &[Service],
    ) -> Scrobbler {
        let saved = queue.as_deref().map(load).unwrap_or_default();
        let usable = |s: &&Service| **s != Service::LastFm || apis.lastfm_keys.is_some();
        let tokens = connected
            .iter()
            .filter(usable)
            .map(|&s| (s, None))
            .collect();
        Scrobbler(Arc::new(Inner {
            http,
            store,
            apis,
            path: queue,
            queue: tokio::sync::Mutex::new(saved),
            tokens: Mutex::new(tokens),
        }))
    }

    /// "Now playing" to every connected service: never queued nor retried.
    pub async fn now_playing(&self, l: Listen) -> Outcome {
        let mut out = Outcome::default();
        for service in SERVICES {
            let Some(token) = self.token(service).await else {
                continue;
            };
            let sent = self.send(service, &token, Kind::PlayingNow, &[&l]).await;
            if sent == Verdict::Reconnect && self.forget(service, &token).await {
                out.refused.push(service);
            }
        }
        out
    }

    /// Queues `l` for every connected service, saves the queue, and flushes it. A service
    /// whose token cannot be read yet gets it queued all the same.
    pub async fn scrobble(&self, l: Listen) -> Outcome {
        let mut queue = self.0.queue.lock().await;
        let on: Vec<Service> = {
            let tokens = self.tokens();
            SERVICES
                .into_iter()
                .filter(|s| tokens.contains_key(s))
                .collect()
        };
        if !on.is_empty() {
            let entries = on.into_iter().map(|service| Entry {
                service,
                listen: l.clone(),
            });
            queue.extend(entries);
            let over = queue.len().saturating_sub(MAX_QUEUED);
            queue.drain(..over);
            self.save(&queue).await;
        }
        self.flush_queue(&mut queue).await
    }

    /// Sends what is queued, oldest first; a service stops at its first failure worth a retry.
    pub async fn flush(&self) -> Outcome {
        let mut queue = self.0.queue.lock().await;
        self.flush_queue(&mut queue).await
    }

    async fn flush_queue(&self, queue: &mut Vec<Entry>) -> Outcome {
        let mut out = Outcome::default();
        for service in SERVICES {
            loop {
                let of_service = queue.iter().filter(|e| e.service == service);
                let batch: Vec<&Listen> = of_service
                    .map(|e| &e.listen)
                    .take(service.batch())
                    .collect();
                if batch.is_empty() {
                    break;
                }
                // Not connected, or no token readable yet: kept for later.
                let Some(token) = self.token(service).await else {
                    break;
                };
                let n = batch.len();
                let kind = if n == 1 { Kind::Single } else { Kind::Import };
                match self.send(service, &token, kind, &batch).await {
                    // ponytail: a refused batch goes whole; ListenBrainz refuses a batch for
                    // one bad listen, so send a refused import singly if bad data ever shows
                    // up. Last.fm takes a batch and ignores the bad ones: those go too.
                    Verdict::Done | Verdict::Drop => {
                        let mut left = n;
                        queue.retain(|e| {
                            let sent = e.service == service && left > 0;
                            left -= usize::from(sent);
                            !sent
                        });
                        self.save(queue).await;
                    }
                    Verdict::Retry => {
                        out.retry = true;
                        break;
                    }
                    Verdict::Reconnect => {
                        if self.forget(service, &token).await {
                            out.refused.push(service);
                        }
                        break;
                    }
                }
            }
        }
        out
    }

    /// Sends `listens` to `service` with `token`, as `kind`.
    async fn send(
        &self,
        service: Service,
        token: &str,
        kind: Kind,
        listens: &[&Listen],
    ) -> Verdict {
        let (http, apis) = (&self.0.http, &self.0.apis);
        match (service, apis.lastfm_keys) {
            (Service::ListenBrainz, _) => {
                listenbrainz::submit(http, &apis.listenbrainz, token, kind, listens).await
            }
            (Service::LastFm, Some(keys)) => {
                lastfm::submit(http, &apis.lastfm, keys, token, kind, listens).await
            }
            // Never: not connected without keys (`new`). Kept.
            (Service::LastFm, None) => Verdict::Retry,
        }
    }

    /// Last.fm's desktop auth (`lastfm::authorize`, which hands `open` the page where the user
    /// allows Tidash), then keeps the session key: whose it is, or the error to show.
    pub async fn connect_lastfm(&self, open: impl FnOnce(String) + Send) -> Result<String, String> {
        let (http, apis) = (&self.0.http, &self.0.apis);
        // Settings offers no Connect without keys.
        let Some(keys) = apis.lastfm_keys else {
            return Err(tr("Last.fm could not be reached. Try again later.").into_owned());
        };
        let session = lastfm::authorize(http, &apis.lastfm, keys, open).await?;
        match self.set_token(Service::LastFm, Some(session.key)).await {
            Ok(()) => Ok(session.name),
            Err(e) => Err(format!("{}: {e}", tr("The token could not be stored"))),
        }
    }

    /// Checks `token` with ListenBrainz and keeps it: whose it is, or the error to show.
    pub async fn connect_listenbrainz(&self, token: String) -> Result<String, String> {
        let (http, base) = (&self.0.http, &self.0.apis.listenbrainz);
        match listenbrainz::validate(http, base, &token).await {
            Ok(Some(user)) => match self.set_token(Service::ListenBrainz, Some(token)).await {
                Ok(()) => Ok(user),
                Err(e) => Err(format!("{}: {e}", tr("The token could not be stored"))),
            },
            Ok(None) => Err(tr("ListenBrainz did not accept that token.").into_owned()),
            Err(e) => {
                log::warn!("ListenBrainz token check: {e}");
                let why = tr("ListenBrainz could not be reached. Try again later.");
                Err(why.into_owned())
            }
        }
    }

    /// Forgets `service`'s token, and the plays queued for it.
    pub async fn disconnect(&self, service: Service) -> tidal::Result<()> {
        self.set_token(service, None).await?;
        let mut queue = self.0.queue.lock().await;
        let before = queue.len();
        queue.retain(|e| e.service != service);
        if queue.len() < before {
            self.save(&queue).await;
        }
        Ok(())
    }

    fn tokens(&self) -> MutexGuard<'_, HashMap<Service, Option<String>>> {
        self.0.tokens.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A connected `service`'s token, read from the store until one is found; `None`: not
    /// connected, or no token readable yet.
    async fn token(&self, service: Service) -> Option<String> {
        let cached = self.tokens().get(&service).cloned();
        match cached {
            None => return None,
            Some(Some(token)) => return Some(token),
            Some(None) => {}
        }
        match store_call(&self.0.store, move |s| s.secret(service.secret())).await {
            Ok(read) => {
                let mut tokens = self.tokens();
                // Disconnected meanwhile: none. Connected anew: that token wins.
                let slot = tokens.get_mut(&service)?;
                if slot.is_none() {
                    *slot = read;
                }
                slot.clone()
            }
            Err(e) => {
                log::warn!("the {service:?} token is unreadable: {e}");
                None
            }
        }
    }

    /// Connects `service` with `token`, or disconnects it (`None`): in the store, then here.
    async fn set_token(&self, service: Service, token: Option<String>) -> tidal::Result<()> {
        let value = token.clone();
        store_call(&self.0.store, move |s| match value {
            Some(v) => s.set_secret(service.secret(), &v),
            None => s.delete_secret(service.secret()),
        })
        .await?;
        match token {
            Some(t) => self.tokens().insert(service, Some(t)),
            None => self.tokens().remove(&service),
        };
        Ok(())
    }

    /// `service` refused `token`: disconnected and the token forgotten, unless a connect
    /// replaced it meanwhile. True if so.
    async fn forget(&self, service: Service, token: &str) -> bool {
        {
            let mut tokens = self.tokens();
            if tokens.get(&service).cloned().flatten().as_deref() != Some(token) {
                return false;
            }
            tokens.remove(&service);
        }
        let deleted = store_call(&self.0.store, move |s| s.delete_secret(service.secret()));
        if let Err(e) = deleted.await {
            log::warn!("could not remove the refused {service:?} token: {e}");
        }
        true
    }

    /// Saves `queue` where it lives, as the settings are saved. A failure is logged: the
    /// plays stay in memory.
    async fn save(&self, queue: &[Entry]) {
        let Some(path) = self.0.path.clone() else {
            return;
        };
        let saved = match serde_json::to_vec(queue) {
            Ok(bytes) => tokio::task::spawn_blocking(move || write_atomic(&path, &bytes))
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e))),
            Err(e) => Err(e.into()),
        };
        if let Err(e) = saved {
            log::warn!("could not save the scrobble queue: {e}");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tidal::MemoryStore;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(crate) fn listen(track_id: u64, started_at: u64) -> Listen {
        Listen {
            track_id,
            title: format!("Track {track_id}"),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_s: 200,
            track_number: 3,
            started_at,
        }
    }

    #[test]
    fn due_rule() {
        let s = Duration::from_secs;
        assert!(!due(30, s(30)) && !due(30, s(3600)), "30 s: never");
        assert!(!due(31, s(15)) && due(31, s(16)));
        assert!(!due(200, s(99)) && due(200, s(100)), "half");
        assert!(!due(600, s(239)) && due(600, s(240)), "4 minutes");
    }

    pub(crate) fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    }

    /// An empty dir of its own for this test's queue file.
    fn temp_dir(name: &str) -> PathBuf {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("tidash-scrobble-{name}-{pid}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A store holding ListenBrainz token "tok".
    fn connected() -> Arc<MemoryStore> {
        let store = Arc::new(MemoryStore::default());
        store.set_secret("listenbrainz", "tok").expect("stored");
        store
    }

    /// The settings name a ListenBrainz user.
    const LB: &[Service] = &[Service::ListenBrainz];

    /// A scrobbler sending to `server`, its queue in `dir` if any, to the services `on`.
    fn scrobbler(
        server: &MockServer,
        dir: Option<&Path>,
        store: Arc<MemoryStore>,
        on: &[Service],
    ) -> Scrobbler {
        let apis = Apis {
            listenbrainz: format!("{}/", server.uri()).parse().expect("a URL"),
            ..Apis::default()
        };
        let queue = dir.map(|d| d.join("scrobbles.json"));
        Scrobbler::new(reqwest::Client::new(), store, queue, apis, on)
    }

    /// When each listen saved in `dir`'s queue started.
    fn saved(dir: &Path) -> Vec<u64> {
        let queue = load(&dir.join("scrobbles.json"));
        queue.iter().map(|e| e.listen.started_at).collect()
    }

    /// Each submission `server` took: its listen type, and when each of its listens started.
    async fn submitted(server: &MockServer) -> Vec<(String, Vec<u64>)> {
        let requests = server.received_requests().await.expect("recorded");
        let submits = requests
            .iter()
            .filter(|r| r.url.path() == "/1/submit-listens");
        submits
            .map(|r| {
                let body: serde_json::Value = serde_json::from_slice(&r.body).expect("JSON");
                let kind = body["listen_type"].as_str().unwrap_or_default().to_owned();
                let listens = body["payload"].as_array().cloned().unwrap_or_default();
                let at = listens.iter().filter_map(|l| l["listened_at"].as_u64());
                (kind, at.collect())
            })
            .collect()
    }

    fn submit(token: &str) -> wiremock::MockBuilder {
        Mock::given(method("POST"))
            .and(path("/1/submit-listens"))
            .and(header("authorization", format!("Token {token}")))
    }

    fn ok() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(r#"{"status":"ok"}"#, "application/json")
    }

    #[test]
    fn failed_scrobbles_persist_and_flush_in_order() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let down = ResponseTemplate::new(503);
            submit("tok")
                .respond_with(down)
                .up_to_n_times(2)
                .mount(&server)
                .await;
            submit("tok").respond_with(ok()).mount(&server).await;
            let dir = temp_dir("order");
            let store = connected();
            let s = scrobbler(&server, Some(&dir), store.clone(), LB);
            for at in [1_700_000_100, 1_700_000_200] {
                let out = s.scrobble(listen(at, at)).await;
                assert_eq!(
                    out,
                    Outcome {
                        retry: true,
                        refused: vec![]
                    },
                    "kept"
                );
            }
            assert_eq!(saved(&dir), [1_700_000_100, 1_700_000_200], "saved between");
            // A restart reads them back; the next flush sends both at once, oldest first.
            let s = scrobbler(&server, Some(&dir), store, LB);
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(saved(&dir), Vec::<u64>::new(), "taken: none left");
            let both = vec![1_700_000_100, 1_700_000_200];
            let want = [
                ("single".to_owned(), vec![1_700_000_100]),
                ("import".to_owned(), both.clone()),
                ("import".to_owned(), both),
            ];
            assert_eq!(submitted(&server).await, want);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn invalid_token_keeps_the_queue_and_asks_once() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let refused = ResponseTemplate::new(401);
            submit("tok").respond_with(refused).mount(&server).await;
            submit("new").respond_with(ok()).mount(&server).await;
            let valid = r#"{"code":200,"message":"Token valid.","valid":true,"user_name":"rob"}"#;
            Mock::given(method("GET"))
                .and(path("/1/validate-token"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(valid, "application/json"))
                .mount(&server)
                .await;
            let dir = temp_dir("refused");
            let store = connected();
            let s = scrobbler(&server, Some(&dir), store.clone(), LB);
            let out = s.scrobble(listen(1, 1_700_000_100)).await;
            let want = Outcome {
                retry: false,
                refused: vec![Service::ListenBrainz],
            };
            assert_eq!(out, want);
            assert_eq!(saved(&dir), [1_700_000_100], "the queue stays");
            let token = store.secret("listenbrainz").expect("read");
            assert_eq!(token, None, "the token is forgotten");
            // Not connected now: asked once; nothing else is sent, nor queued.
            assert_eq!(
                s.scrobble(listen(2, 1_700_000_200)).await,
                Outcome::default()
            );
            assert_eq!(s.now_playing(listen(3, 0)).await, Outcome::default());
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(saved(&dir), [1_700_000_100]);
            assert_eq!(submitted(&server).await.len(), 1);
            // Connected again: what waited goes out.
            let user = s.connect_listenbrainz("new".into()).await;
            assert_eq!(user.as_deref(), Ok("rob"));
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(saved(&dir), Vec::<u64>::new());
            let last = submitted(&server).await.pop();
            assert_eq!(last, Some(("single".into(), vec![1_700_000_100])));
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn connect_validates_and_stores_the_token() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let answers = [
                (
                    "good",
                    r#"{"code":200,"message":"Token valid.","valid":true,"user_name":"rob"}"#,
                ),
                (
                    "bad",
                    r#"{"code":200,"message":"Token invalid.","valid":false}"#,
                ),
            ];
            for (token, body) in answers {
                Mock::given(method("GET"))
                    .and(path("/1/validate-token"))
                    .and(header("authorization", format!("Token {token}")))
                    .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
                    .mount(&server)
                    .await;
            }
            let down = ResponseTemplate::new(503);
            submit("good").respond_with(down).mount(&server).await;
            let dir = temp_dir("connect");
            let store = Arc::new(MemoryStore::default());
            let s = scrobbler(&server, Some(&dir), store.clone(), &[]);
            let refused = s.connect_listenbrainz("bad".into()).await;
            let why = "ListenBrainz did not accept that token.";
            assert_eq!(refused, Err(why.to_owned()));
            assert_eq!(store.secret("listenbrainz").expect("read"), None);
            let user = s.connect_listenbrainz("good".into()).await;
            assert_eq!(user.as_deref(), Ok("rob"));
            let kept = store.secret("listenbrainz").expect("read");
            assert_eq!(kept.as_deref(), Some("good"));
            // Sent with it; kept while the service is down; dropped with the token.
            assert!(s.scrobble(listen(1, 1_700_000_100)).await.retry);
            assert_eq!(submitted(&server).await.len(), 1);
            s.disconnect(Service::ListenBrainz)
                .await
                .expect("disconnected");
            assert_eq!(store.secret("listenbrainz").expect("read"), None);
            assert_eq!(saved(&dir), Vec::<u64>::new());
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// The settings name no user (lost, or never connected), but a token is still stored:
    /// Settings offers Connect, so nothing goes out and nothing is kept.
    #[test]
    fn a_token_without_a_user_sends_nothing() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            submit("tok").respond_with(ok()).mount(&server).await;
            let dir = temp_dir("no-user");
            let s = scrobbler(&server, Some(&dir), connected(), &[]);
            assert_eq!(s.now_playing(listen(1, 0)).await, Outcome::default());
            let out = s.scrobble(listen(1, 1_700_000_100)).await;
            assert_eq!(out, Outcome::default());
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(saved(&dir), Vec::<u64>::new(), "nothing queued");
            assert_eq!(submitted(&server).await.len(), 0, "nothing sent");
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// The settings name a user, but the token is gone or unreadable: the listen waits in the
    /// queue, and goes out once the token reads.
    #[test]
    fn a_user_without_a_token_still_queues() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            submit("tok").respond_with(ok()).mount(&server).await;
            let dir = temp_dir("no-token");
            let store = Arc::new(MemoryStore::default());
            let s = scrobbler(&server, Some(&dir), store.clone(), LB);
            assert_eq!(s.now_playing(listen(1, 0)).await, Outcome::default());
            let out = s.scrobble(listen(1, 1_700_000_100)).await;
            assert_eq!(out, Outcome::default());
            assert_eq!(saved(&dir), [1_700_000_100], "queued");
            assert_eq!(submitted(&server).await.len(), 0, "nothing sent");
            store.set_secret("listenbrainz", "tok").expect("stored");
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(saved(&dir), Vec::<u64>::new());
            let sent = submitted(&server).await;
            assert_eq!(sent, [("single".to_owned(), vec![1_700_000_100])]);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
