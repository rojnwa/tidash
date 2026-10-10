//! Tokio backend on its own thread: receives `Action`s, sends `Event`s.

use crate::i18n::tr;
use crate::images::Artwork;
use crate::model::{Action, Event, Load, Redirect};
use crate::playback::{Msg, Playback};
use crate::settings::Settings;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tidal::{Client, ClientConfig, DiskCache, Pkce, TokenStore};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::{AbortHandle, JoinSet};

/// Spawns the backend. Before it handles actions it reads the settings and the stored session
/// and opens the audio output, off the UI thread: all can take a while. Every event sent is
/// followed by `waker()`. Without an audio device the app runs, and play actions report the
/// missing output.
/// It also opens the images cache, set in `images` for the media controls before any track.
pub fn start(
    waker: impl Fn() + Send + Sync + 'static,
    images: Arc<OnceLock<Arc<DiskCache>>>,
) -> (UnboundedSender<Action>, Receiver<Event>) {
    let (action_tx, action_rx) = unbounded_channel();
    let (event_tx, event_rx) = channel();
    let emit = Arc::new(Emitter {
        tx: event_tx,
        waker: Box::new(waker),
    });
    let spawned = std::thread::Builder::new()
        .name("backend".into())
        .spawn(move || {
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
            // The settings file and the store block: read them before the runtime runs on
            // this thread. Logging in first lets the UI leave the login page before the device
            // is open.
            let settings_path = crate::settings::path();
            let settings = settings_path.as_deref().map(Settings::load);
            let settings = settings.unwrap_or_default();
            emit.send(Event::Settings(settings.clone()));
            let audio = settings.audio.clone();
            let (msg_tx, msg_rx) = unbounded_channel();
            let mut backend = Backend::new(
                Arc::clone(&emit),
                msg_tx.clone(),
                settings,
                tidal::default_store(),
                open_api_cache(),
            );
            backend.artwork = Artwork::new(crate::images::open_cache(&images));
            backend.playback = open_player(&msg_tx, audio.output()).map(|p| {
                emit.send(Event::Player(p.clone()));
                Playback::new(p, emit, msg_tx, audio)
            });
            backend.settings_path = settings_path;
            rt.block_on(backend.run(action_rx, msg_rx));
        });
    if let Err(e) = spawned {
        log::error!("backend thread failed: {e}");
    }
    (action_tx, event_rx)
}

/// The per-user cache directory; `None` without a home directory.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "tidash").map(|p| p.cache_dir().to_path_buf())
}

/// The saved API bodies (spec §6); without a cache dir, or if it fails to open, none.
fn open_api_cache() -> Option<Arc<DiskCache>> {
    DiskCache::open(cache_dir()?.join("api"), 64 << 20, "json")
        .inspect_err(|e| log::warn!("no API cache: {e}"))
        .ok()
        .map(Arc::new)
}

/// The audio output on `config`, its events forwarded to `msgs`; `None` without a device.
fn open_player(
    msgs: &UnboundedSender<Msg>,
    config: player::OutputConfig,
) -> Option<player::Player> {
    let (player, events) = player::Player::spawn(config)
        .inspect_err(|e| log::warn!("no audio output: {e}"))
        .ok()?;
    let tx = msgs.clone();
    let forward = move || {
        for ev in events {
            if tx.send(Msg::Player(ev)).is_err() {
                return; // backend gone
            }
        }
    };
    if let Err(e) = std::thread::Builder::new()
        .name("player-events".into())
        .spawn(forward)
    {
        log::error!("player event thread failed: {e}");
    }
    Some(player)
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

/// For a request on `client` that failed: `Error::Auth` means its one token refresh failed as
/// well, so that session is dead and the backend logs out to the login screen. True if so;
/// else the caller reports the error itself.
pub(crate) fn session_expired(
    e: &(dyn std::error::Error + 'static),
    client: &Arc<Client>,
    msgs: &UnboundedSender<Msg>,
) -> bool {
    let Some(auth @ tidal::Error::Auth(_)) = e.downcast_ref() else {
        return false;
    };
    log::warn!("session expired: {e}");
    let expired = Msg::SessionExpired(Arc::clone(client), auth.client_refused());
    let _ = msgs.send(expired); // backend gone = app closing
    true
}

/// A login exchange or a token refresh refused the client ID.
fn client_refused() -> Event {
    Event::Error(tr("TIDAL refused the client ID. Check Settings › Account.").into_owned())
}

struct Backend {
    emit: Arc<Emitter>,
    /// For the tasks it spawns.
    msgs: UnboundedSender<Msg>,
    cfg: ClientConfig,
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    pkce: Option<Pkce>,
    /// The search in flight, one of `tasks`: a newer search aborts it.
    search_task: Option<AbortHandle>,
    client: Option<Arc<Client>>,
    /// The session's page, search and favorites requests. Logout and expiry abort them and
    /// wait, so none of their replies or cache writes come after (`stop_requests`).
    // ponytail: a cache write already inside its `spawn_blocking` still lands (sub-ms window);
    // have `DiskCache::clear` take the write lock and refuse later writes if that ever matters.
    tasks: JoinSet<()>,
    /// The saved API bodies; `None` without a cache dir.
    api_cache: Option<Arc<DiskCache>>,
    artwork: Artwork,
    /// `None` without an audio device.
    playback: Option<Playback>,
    settings: Settings,
    /// Where `settings` are saved; `None` without a config dir (nothing is saved).
    settings_path: Option<PathBuf>,
}

impl Backend {
    /// `store` comes with what its probe read (`tidal::default_store`); the session logs in
    /// from that, on the client `settings` name.
    fn new(
        emit: Arc<Emitter>,
        msgs: UnboundedSender<Msg>,
        settings: Settings,
        (store, stored): (Arc<dyn TokenStore>, tidal::Result<Option<tidal::Session>>),
        api_cache: Option<Arc<DiskCache>>,
    ) -> Self {
        let mut backend = Self {
            emit,
            msgs,
            cfg: settings.client_config(),
            // The token exchange is awaited in the action loop: a stalled server must not hold
            // it. `Client::new` would panic where this fails (no TLS backend).
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("an HTTP client"),
            store,
            pkce: None,
            search_task: None,
            client: None,
            tasks: JoinSet::new(),
            api_cache,
            artwork: Artwork::new(None),
            playback: None,
            settings,
            settings_path: None,
        };
        backend.resume(stored);
        backend
    }

    /// Until the UI drops its sender (app closing); then the current play is reported.
    async fn run(mut self, mut rx: UnboundedReceiver<Action>, mut msgs: UnboundedReceiver<Msg>) {
        // Samples the position for a resume after a player error, and preloads the next track
        // near the end; only ticks while playing.
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        self.load_favorites(); // the stored session's
        loop {
            let playing = self.playback.as_ref().is_some_and(Playback::is_playing);
            tokio::select! {
                biased; // a pending player error goes before a sample of its reset clock
                Some(msg) = msgs.recv() => match msg {
                    Msg::SessionExpired(dead, refused) => self.expire(dead, refused).await,
                    msg => {
                        if let Some(p) = &mut self.playback {
                            p.on_msg(self.client.as_ref(), msg);
                        }
                    }
                },
                action = rx.recv() => match action {
                    Some(action) => self.handle(action).await,
                    None => break,
                },
                _ = tick.tick(), if playing => {
                    if let Some(p) = &mut self.playback {
                        p.tick(self.client.as_ref());
                    }
                }
            }
        }
        if let Some(p) = &mut self.playback {
            p.shutdown(self.client.as_ref()).await;
        }
    }

    /// A stored session logs in without any network call; expiry refreshes lazily.
    fn resume(&mut self, stored: tidal::Result<Option<tidal::Session>>) {
        match stored {
            Ok(Some(session)) => self.log_in(session),
            Ok(None) => {}
            // Left in place: a denied or locked store may read fine next launch.
            Err(e) => {
                log::warn!("stored session unreadable: {e}");
                self.emit.send(Event::CredentialsNotPersistent);
            }
        }
    }

    fn log_in(&mut self, session: tidal::Session) {
        let (cache, user) = (self.api_cache.clone(), session.user_id);
        match Client::new(self.cfg.clone(), session, self.store.clone(), cache) {
            Ok(c) => {
                self.client = Some(Arc::new(c));
                self.emit.send(Event::LoggedIn(user));
            }
            Err(e) => self
                .emit
                .send(Event::Error(format!("{}: {e}", tr("Login failed")))),
        }
    }

    /// Runs `task` for this session: logout and expiry abort it.
    fn spawn(&mut self, task: impl Future<Output = ()> + Send + 'static) -> AbortHandle {
        while self.tasks.try_join_next().is_some() {} // drop the finished ones
        self.tasks.spawn(task)
    }

    /// Logout and expiry: the session's requests in flight, `tasks` and the stream load, are
    /// aborted and waited for, so none of their replies or cache writes come after.
    async fn stop_requests(&mut self) {
        self.tasks.shutdown().await;
        if let Some(p) = &mut self.playback {
            p.abort_load().await;
        }
    }

    /// The favorite ids for the hearts. A failure is logged, not shown: they stay empty.
    fn load_favorites(&mut self) {
        let Some(c) = self.client.clone() else { return };
        let (emit, msgs) = (self.emit.clone(), self.msgs.clone());
        self.spawn(async move {
            match c.favorite_ids().await {
                Ok(ids) => emit.send(Event::Favorites(ids)),
                Err(e) if session_expired(&e, &c, &msgs) => {}
                Err(e) => log::warn!("favorite ids: {e}"),
            }
        });
    }

    /// `dead`'s session is gone (see `session_expired`): back to the login screen, playback
    /// stopped, the stored copy removed if it can be. The queue stays for the next login.
    /// `refused`: by TIDAL refusing the client ID, which is then shown.
    async fn expire(&mut self, dead: Arc<Client>, refused: bool) {
        // Already out, or a request from before a newer login: that session is not dead.
        if !self.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &dead)) {
            return;
        }
        self.client = None;
        self.stop_requests().await;
        dead.detach_store().await; // as on logout
        if let Some(p) = &mut self.playback {
            p.session_lost();
        }
        if let Err(e) = self.with_store(|s| s.clear()).await {
            log::warn!("could not remove the expired session: {e}");
        }
        self.emit.send(Event::SessionExpired);
        if refused {
            self.emit.send(client_refused());
        }
    }

    async fn handle(&mut self, action: Action) {
        match action {
            Action::BeginLogin => {
                let url = self.begin_login();
                // Some launchers (xdg-open) would block the backend until the browser exits.
                if let Err(e) = open::that_detached(&url) {
                    log::warn!("could not open the browser: {e}");
                }
            }
            Action::FinishLogin(Redirect(redirect)) => self.finish_login(&redirect).await,
            Action::Logout => self.log_out().await,
            Action::Search(q) => {
                let Some(c) = self.client.clone() else { return };
                let (emit, msgs) = (self.emit.clone(), self.msgs.clone());
                // A newer search supersedes the in-flight one.
                if let Some(old) = self.search_task.take() {
                    old.abort();
                }
                emit.send(Event::SearchStarted(q.clone()));
                self.search_task = Some(self.spawn(async move {
                    match c.search(&q, 30).await {
                        Ok(results) => emit.send(Event::SearchResults { query: q, results }),
                        Err(e) if session_expired(&e, &c, &msgs) => {}
                        Err(e) => emit.send(Event::Error(format!("{}: {e}", tr("Search failed")))),
                    }
                }));
            }
            Action::Open(route) => {
                if let Some(c) = self.client.clone() {
                    let load = crate::pages::open(c, route, self.emit.clone(), self.msgs.clone());
                    self.spawn(load);
                }
            }
            Action::SetFavorite { item, on } => {
                if let Some(c) = self.client.clone() {
                    let (emit, msgs) = (self.emit.clone(), self.msgs.clone());
                    self.spawn(crate::pages::set_favorite(c, item, on, emit, msgs));
                }
            }
            Action::SetSettings(settings) => self.set_settings(settings).await,
            Action::ListDevices => {
                let emit = self.emit.clone();
                tokio::task::spawn_blocking(move || {
                    let names = fastframe_audio::output_device_names()
                        .inspect_err(|e| log::warn!("could not list the output devices: {e}"))
                        .unwrap_or_default();
                    emit.send(Event::Devices(names));
                });
            }
            Action::LoadLyrics(id) => {
                let fetch = move |c: Arc<Client>| async move { c.lyrics(id).await };
                self.load_for_panel(id, Event::Lyrics, fetch);
            }
            Action::LoadCredits(id) => {
                let fetch = move |c: Arc<Client>| async move { c.track_credits(id).await };
                self.load_for_panel(id, Event::Credits, fetch);
            }
            Action::LoadImage { image, px } => {
                let (base, emit) = (&self.cfg.image_base, self.emit.clone());
                self.artwork.load(&self.http, base, &image, px, emit);
            }
            Action::PlayTracks { .. }
            | Action::Play
            | Action::Pause
            | Action::TogglePlay
            | Action::Next
            | Action::Previous
            | Action::Seek(_)
            | Action::SetVolume(_)
            | Action::PlayNext(_)
            | Action::AddToQueue(_)
            | Action::MoveInQueue { .. }
            | Action::RemoveFromQueue(_)
            | Action::PlayQueueIndex(_)
            | Action::SetShuffle(_)
            | Action::SetRepeat(_) => match &mut self.playback {
                Some(p) => p.handle(self.client.as_ref(), action),
                None if matches!(
                    action,
                    Action::PlayTracks { .. }
                        | Action::Play
                        | Action::TogglePlay
                        | Action::PlayQueueIndex(_)
                ) =>
                {
                    self.emit
                        .send(Event::Error(tr("No audio output device").into_owned()));
                }
                None => {}
            },
        }
    }

    /// Track `id`'s lyrics or credits for the side panel, from TIDAL (a missing answer is not
    /// cached, so a saved copy cannot tell "none" from "never fetched"), sent as `event`. A
    /// failure is `Failed`, shown by the panel only.
    fn load_for_panel<T: Send + 'static, F>(
        &mut self,
        id: u64,
        event: fn(u64, Load<T>) -> Event,
        fetch: impl FnOnce(Arc<Client>) -> F + Send + 'static,
    ) where
        F: Future<Output = tidal::Result<T>> + Send,
    {
        let Some(c) = self.client.clone() else { return };
        let (emit, msgs) = (self.emit.clone(), self.msgs.clone());
        self.spawn(async move {
            let load = match fetch(Arc::clone(&c)).await {
                Ok(data) => Load::Ready(data),
                Err(e) if session_expired(&e, &c, &msgs) => return,
                Err(e) => {
                    log::warn!("track {id} for the side panel: {e}");
                    Load::Failed
                }
            };
            emit.send(event(id, load));
        });
    }

    /// A new login on the current client: its authorize URL, also sent to the UI.
    fn begin_login(&mut self) -> String {
        let pkce = Pkce::new();
        let url = tidal::authorize_url(&self.cfg, &pkce).to_string();
        self.pkce = Some(pkce);
        self.emit.send(Event::LoginUrl(url.clone()));
        url
    }

    async fn log_out(&mut self) {
        let client = self.client.take();
        self.stop_requests().await; // before the cache is cleared
        // What the old session still sends (the play report below, a request in flight)
        // may refresh it, but must not write it back once the store is cleared.
        if let Some(c) = &client {
            c.detach_store().await;
        }
        // The next user must not find this one's pages, even offline.
        if let Some(cache) = self.api_cache.clone() {
            let cleared = tokio::task::spawn_blocking(move || cache.clear()).await;
            if !matches!(cleared, Ok(Ok(()))) {
                log::warn!("could not clear the API cache: {cleared:?}");
            }
        }
        self.pkce = None;
        // The user asked to log out: do so even if the stored copy could not be removed.
        if let Err(e) = self.with_store(|s| s.clear()).await {
            self.emit
                .send(Event::Error(format!("{}: {e}", tr("Logout failed"))));
        }
        self.emit.send(Event::LoggedOut);
        // After LoggedOut, whose model reset the playback snapshots then refill.
        if let Some(p) = &mut self.playback {
            p.reset(client.as_ref());
        }
    }

    /// Saves `settings`, sends them back, and works by them: the audio at once; another client
    /// for the next login, logging out of a session made with the old one.
    async fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        // Awaited, so two quick changes land in order.
        if let Some(path) = self.settings_path.clone() {
            let settings = self.settings.clone();
            let saved = tokio::task::spawn_blocking(move || settings.save(&path)).await;
            if let Err(e) = saved.unwrap_or_else(|e| Err(std::io::Error::other(e))) {
                self.emit.send(Event::Error(format!(
                    "{}: {e}",
                    tr("Settings could not be saved")
                )));
            }
        }
        self.emit.send(Event::Settings(self.settings.clone()));
        if let Some(p) = &mut self.playback {
            p.set_audio(self.settings.audio.clone());
        }
        let new = self.settings.client_config();
        if (&new.client_id, &new.redirect_uri) == (&self.cfg.client_id, &self.cfg.redirect_uri) {
            return;
        }
        (self.cfg.client_id, self.cfg.redirect_uri) = (new.client_id, new.redirect_uri);
        if self.client.is_some() {
            self.log_out().await;
            let why = tr("Logged out: the client ID changed. Log in again.");
            self.emit.send(Event::Error(why.into_owned()));
        }
    }

    async fn finish_login(&mut self, redirect: &str) {
        let Some(pkce) = self.pkce.clone() else {
            self.emit.send(Event::Error(
                tr("Login failed: start with \"Log in with TIDAL\" first").into_owned(),
            ));
            return;
        };
        let session = match tidal::code_from_redirect(redirect, &pkce.state) {
            Ok(code) => tidal::exchange_code(&self.http, &self.cfg, &code, &pkce).await,
            Err(e) => Err(e),
        };
        match session {
            Ok(session) => {
                let saved = session.clone();
                if let Err(e) = self.with_store(move |s| s.save(&saved)).await {
                    self.emit.send(Event::Error(format!(
                        "{}: {e}",
                        tr("Login will not be remembered")
                    )));
                }
                self.pkce = None;
                self.log_in(session);
                self.load_favorites();
            }
            Err(e) if e.client_refused() => self.emit.send(client_refused()),
            Err(e) => self
                .emit
                .send(Event::Error(format!("{}: {e}", tr("Login failed")))),
        }
    }

    /// Store calls block, so they run off the async workers (see `TokenStore`).
    async fn with_store(
        &self,
        call: impl FnOnce(&dyn TokenStore) -> tidal::Result<()> + Send + 'static,
    ) -> tidal::Result<()> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || call(store.as_ref()))
            .await
            .unwrap_or_else(|e| Err(tidal::Error::Store(e.to_string())))
    }
}

#[cfg(test)]
#[path = "backend/tests.rs"]
mod tests;
