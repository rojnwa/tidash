//! Tokio backend on its own thread: receives `Action`s, sends `Event`s.

use crate::i18n::tr;
use crate::model::{Action, Event};
use crate::playback::{Msg, Playback};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;
use tidal::{Client, ClientConfig, Pkce, TokenStore};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Opens the audio output, then spawns the backend. Every event sent is followed by `waker()`.
/// Without an audio device (`None`) the app runs, and play actions report the missing output.
pub fn start(
    waker: impl Fn() + Send + Sync + 'static,
) -> (UnboundedSender<Action>, Receiver<Event>, Option<player::Player>) {
    let (action_tx, action_rx) = unbounded_channel();
    let (event_tx, event_rx) = channel();
    let (msg_tx, msg_rx) = unbounded_channel();
    let player = match player::Player::spawn(fastframe_audio::OutputOptions::default()) {
        Ok((player, events)) => {
            let tx = msg_tx.clone();
            let forward = move || {
                for ev in events {
                    if tx.send(Msg::Player(ev)).is_err() {
                        return; // backend gone
                    }
                }
            };
            if let Err(e) = std::thread::Builder::new().name("player-events".into()).spawn(forward) {
                log::error!("player event thread failed: {e}");
            }
            Some(player)
        }
        Err(e) => {
            log::warn!("no audio output: {e}");
            None
        }
    };
    let backend_player = player.clone();
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
        rt.block_on(async move {
            let playback = backend_player.map(|p| Playback::new(p, emit.clone(), msg_tx));
            Backend::new(emit, playback).run(action_rx, msg_rx).await;
        });
    });
    if let Err(e) = spawned {
        log::error!("backend thread failed: {e}");
    }
    (action_tx, event_rx, player)
}

pub(crate) struct Emitter {
    pub(crate) tx: Sender<Event>,
    pub(crate) waker: Box<dyn Fn() + Send + Sync>,
}

impl Emitter {
    pub(crate) fn send(&self, ev: Event) {
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
    client: Option<Arc<Client>>,
    /// `None` without an audio device.
    playback: Option<Playback>,
}

impl Backend {
    fn new(emit: Arc<Emitter>, playback: Option<Playback>) -> Self {
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
            playback,
        }
    }

    /// Until the UI drops its sender (app closing); then the current play is reported.
    async fn run(mut self, mut rx: UnboundedReceiver<Action>, mut msgs: UnboundedReceiver<Msg>) {
        if !self.persistent {
            self.emit.send(Event::CredentialsNotPersistent);
        }
        self.resume();
        // Samples the position for a resume after a player error; only ticks while playing.
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let playing = self.playback.as_ref().is_some_and(Playback::is_playing);
            tokio::select! {
                biased; // a pending player error goes before a sample of its reset clock
                Some(msg) = msgs.recv() => {
                    if let Some(p) = &mut self.playback {
                        p.on_msg(self.client.as_ref(), msg);
                    }
                }
                action = rx.recv() => match action {
                    Some(action) => self.handle(action).await,
                    None => break,
                },
                _ = tick.tick(), if playing => {
                    if let Some(p) = &mut self.playback {
                        p.tick();
                    }
                }
            }
        }
        if let Some(p) = &mut self.playback {
            p.shutdown(self.client.as_ref()).await;
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
                let client = self.client.take();
                self.pkce = None;
                // The user asked to log out: do so even if the stored copy could not be removed.
                if let Err(e) = self.store.clear() {
                    self.emit.send(Event::Error(format!("{}: {e}", tr("Logout failed"))));
                }
                self.emit.send(Event::LoggedOut);
                // After LoggedOut, whose model reset the playback snapshots then refill.
                if let Some(p) = &mut self.playback {
                    p.reset(client.as_ref());
                }
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
            Action::PlayTracks { .. }
            | Action::Play
            | Action::Pause
            | Action::Next
            | Action::Previous
            | Action::Seek(_)
            | Action::SetVolume(_) => match &mut self.playback {
                Some(p) => p.handle(self.client.as_ref(), action),
                None if matches!(action, Action::PlayTracks { .. } | Action::Play) => {
                    self.emit.send(Event::Error(tr("No audio output device").into_owned()));
                }
                None => {}
            },
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
