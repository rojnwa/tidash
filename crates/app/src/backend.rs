//! Tokio backend on its own thread: receives `Action`s, sends `Event`s.

use crate::i18n::tr;
use crate::model::{Action, Event};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use tidal::{Client, ClientConfig, Pkce, TokenStore};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Spawns the backend. Every event sent is followed by `waker()`.
pub fn start(
    waker: impl Fn() + Send + Sync + 'static,
) -> (UnboundedSender<Action>, Receiver<Event>) {
    let (action_tx, action_rx) = unbounded_channel();
    let (event_tx, event_rx) = channel();
    let spawned = std::thread::Builder::new().name("backend".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                log::error!("tokio runtime failed: {e}");
                return;
            }
        };
        let emit = Arc::new(Emitter { tx: event_tx, waker: Box::new(waker) });
        rt.block_on(Backend::new(emit).run(action_rx));
    });
    if let Err(e) = spawned {
        log::error!("backend thread failed: {e}");
    }
    (action_tx, event_rx)
}

struct Emitter {
    tx: Sender<Event>,
    waker: Box<dyn Fn() + Send + Sync>,
}

impl Emitter {
    fn send(&self, ev: Event) {
        let _ = self.tx.send(ev); // receiver gone = app closing
        (self.waker)();
    }
}

/// Downloads a JPEG and decodes it off the async workers.
async fn fetch_image(http: &reqwest::Client, url: &str) -> Result<egui::ColorImage, Box<dyn std::error::Error + Send + Sync>> {
    let bytes = http.get(url).send().await?.error_for_status()?.bytes().await?;
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)?.to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        Ok(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
    })
    .await?
}

struct Backend {
    emit: Arc<Emitter>,
    cfg: ClientConfig,
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    persistent: bool,
    pkce: Option<Pkce>,
    search_task: Option<tokio::task::JoinHandle<()>>,
    // Shared with spawned tasks by later tasks.
    client: Option<Arc<Client>>,
}

impl Backend {
    fn new(emit: Arc<Emitter>) -> Self {
        let (mut store, mut persistent) = tidal::default_store();
        if !persistent {
            // A corrupt stored session makes the probe fail and drops us to memory-only,
            // so no later login would persist. Clear it best-effort and probe again.
            if tidal::KeyringStore::new().clear().is_ok() {
                (store, persistent) = tidal::default_store();
            }
        }
        Self {
            emit,
            cfg: ClientConfig::default(),
            http: reqwest::Client::new(),
            store,
            persistent,
            pkce: None,
            search_task: None,
            client: None,
        }
    }

    async fn run(mut self, mut rx: UnboundedReceiver<Action>) {
        if !self.persistent {
            self.emit.send(Event::CredentialsNotPersistent);
        }
        self.resume();
        while let Some(action) = rx.recv().await {
            self.handle(action).await;
        }
    }

    /// A stored session logs in without any network call; expiry refreshes lazily.
    fn resume(&mut self) {
        match self.store.load() {
            Ok(Some(session)) => self.log_in(session),
            Ok(None) => {}
            Err(e) => {
                log::warn!("stored session unusable: {e}");
                let _ = self.store.clear(); // best effort, so the next login persists
            }
        }
    }

    fn log_in(&mut self, session: tidal::Session) {
        match Client::new(self.cfg.clone(), session, self.store.clone()) {
            Ok(c) => {
                self.client = Some(Arc::new(c));
                self.emit.send(Event::LoggedIn);
            }
            Err(e) => self.emit.send(Event::Error(format!("{}: {e}", tr("Login failed")))),
        }
    }

    async fn handle(&mut self, action: Action) {
        match action {
            Action::BeginLogin => {
                let pkce = Pkce::new();
                let url = tidal::authorize_url(&self.cfg, &pkce).to_string();
                self.pkce = Some(pkce);
                if let Err(e) = open::that(&url) {
                    log::warn!("could not open the browser: {e}");
                }
                self.emit.send(Event::LoginUrl(url));
            }
            Action::FinishLogin(redirect) => self.finish_login(&redirect).await,
            Action::Logout => {
                self.client = None;
                self.pkce = None;
                // The user asked to log out: do so even if the stored copy could not be removed.
                if let Err(e) = self.store.clear() {
                    self.emit.send(Event::Error(format!("{}: {e}", tr("Logout failed"))));
                }
                self.emit.send(Event::LoggedOut);
            }
            Action::Search(q) => {
                let Some(c) = self.client.clone() else { return };
                let emit = self.emit.clone();
                // A newer search supersedes the in-flight one.
                if let Some(old) = self.search_task.take() {
                    old.abort();
                }
                emit.send(Event::SearchStarted(q.clone()));
                self.search_task = Some(tokio::spawn(async move {
                    match c.search(&q, 30).await {
                        Ok(results) => emit.send(Event::SearchResults { query: q, results }),
                        Err(e) => emit.send(Event::Error(format!("{}: {e}", tr("Search failed")))),
                    }
                }));
            }
            Action::OpenAlbum(id) => {
                let Some(c) = self.client.clone() else { return };
                let emit = self.emit.clone();
                emit.send(Event::AlbumLoading(id));
                tokio::spawn(async move {
                    match tokio::try_join!(c.album(id), c.album_tracks(id)) {
                        Ok((album, tracks)) => emit.send(Event::Album { album, tracks }),
                        Err(e) => {
                            emit.send(Event::AlbumFailed(id));
                            emit.send(Event::Error(format!("{}: {e}", tr("Could not open album"))));
                        }
                    }
                });
            }
            Action::LoadImage { cover, size } => {
                let url = tidal::image_url(&self.cfg, &cover, size);
                let (http, emit) = (self.http.clone(), self.emit.clone());
                tokio::spawn(async move {
                    match fetch_image(&http, &url).await {
                        Ok(image) => emit.send(Event::Image { cover, size, image }),
                        Err(e) => log::debug!("image {url}: {e}"), // placeholder stays
                    }
                });
            }
            Action::PlayTracks { tracks, start } => {
                log::debug!("play requested: {} tracks from {start}", tracks.len());
            }
            // ponytail: playback transport arrives in tasks 19-20.
            _ => {}
        }
    }

    async fn finish_login(&mut self, redirect: &str) {
        let Some(pkce) = self.pkce.clone() else {
            self.emit.send(Event::Error(tr("Login failed: start with \"Log in with TIDAL\" first").into_owned()));
            return;
        };
        let session = match tidal::code_from_redirect(redirect, &pkce.state) {
            Ok(code) => tidal::exchange_code(&self.http, &self.cfg, &code, &pkce).await,
            Err(e) => Err(e),
        };
        match session {
            Ok(session) => {
                if let Err(e) = self.store.save(&session) {
                    self.emit.send(Event::Error(format!("{}: {e}", tr("Login will not be remembered"))));
                }
                self.pkce = None;
                self.log_in(session);
            }
            Err(e) => self.emit.send(Event::Error(format!("{}: {e}", tr("Login failed")))),
        }
    }
}
