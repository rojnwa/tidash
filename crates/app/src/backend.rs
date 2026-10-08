//! Tokio backend on its own thread: receives `Action`s, sends `Event`s.

use crate::i18n::tr;
use crate::images::Artwork;
use crate::model::{Action, Event, Redirect};
use crate::playback::{Msg, Playback};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tidal::{Client, ClientConfig, DiskCache, Pkce, TokenStore};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;

/// Spawns the backend. Before it handles actions it opens the audio output and reads the
/// stored session, off the UI thread: both can take a while. Every event sent is followed by
/// `waker()`. Without an audio device the app runs, and play actions report the missing output.
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
            // The store blocks: read it before the runtime runs on this thread. Logging in
            // first lets the UI leave the login page before the device is open.
            let (msg_tx, msg_rx) = unbounded_channel();
            let mut backend = Backend::new(
                Arc::clone(&emit),
                msg_tx.clone(),
                tidal::default_store(),
                open_api_cache(),
            );
            backend.artwork = Artwork::new(crate::images::open_cache(&images));
            backend.playback = open_player(&msg_tx).map(|p| {
                emit.send(Event::Player(p.clone()));
                Playback::new(p, emit, msg_tx)
            });
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

/// The audio output, its events forwarded to `msgs`; `None` without a device.
fn open_player(msgs: &UnboundedSender<Msg>) -> Option<player::Player> {
    let (player, events) = player::Player::spawn(fastframe_audio::OutputOptions::default())
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
    let dead = matches!(e.downcast_ref(), Some(tidal::Error::Auth(_)));
    if dead {
        log::warn!("session expired: {e}");
        let _ = msgs.send(Msg::SessionExpired(Arc::clone(client))); // backend gone = app closing
    }
    dead
}

struct Backend {
    emit: Arc<Emitter>,
    /// For the tasks it spawns.
    msgs: UnboundedSender<Msg>,
    cfg: ClientConfig,
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    pkce: Option<Pkce>,
    search_task: Option<tokio::task::JoinHandle<()>>,
    client: Option<Arc<Client>>,
    /// The session's page and favorites requests. Logout and expiry abort them and wait, so
    /// none of their replies or cache writes come after.
    // ponytail: a cache write already inside its `spawn_blocking` still lands (sub-ms window);
    // have `DiskCache::clear` take the write lock and refuse later writes if that ever matters.
    tasks: JoinSet<()>,
    /// The saved API bodies; `None` without a cache dir.
    api_cache: Option<Arc<DiskCache>>,
    artwork: Artwork,
    /// `None` without an audio device.
    playback: Option<Playback>,
}

impl Backend {
    /// `store` comes with what its probe read (`tidal::default_store`); the session logs in
    /// from that.
    fn new(
        emit: Arc<Emitter>,
        msgs: UnboundedSender<Msg>,
        (store, stored): (Arc<dyn TokenStore>, tidal::Result<Option<tidal::Session>>),
        api_cache: Option<Arc<DiskCache>>,
    ) -> Self {
        let mut backend = Self {
            emit,
            msgs,
            cfg: ClientConfig::default(),
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
        };
        backend.resume(stored);
        backend
    }

    /// Until the UI drops its sender (app closing); then the current play is reported.
    async fn run(mut self, mut rx: UnboundedReceiver<Action>, mut msgs: UnboundedReceiver<Msg>) {
        // Samples the position for a resume after a player error; only ticks while playing.
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        self.load_favorites(); // the stored session's
        loop {
            let playing = self.playback.as_ref().is_some_and(Playback::is_playing);
            tokio::select! {
                biased; // a pending player error goes before a sample of its reset clock
                Some(msg) = msgs.recv() => match msg {
                    Msg::SessionExpired(dead) => self.expire(dead).await,
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
    fn spawn(&mut self, task: impl Future<Output = ()> + Send + 'static) {
        while self.tasks.try_join_next().is_some() {} // drop the finished ones
        self.tasks.spawn(task);
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
    async fn expire(&mut self, dead: Arc<Client>) {
        // Already out, or a request from before a newer login: that session is not dead.
        if !self.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &dead)) {
            return;
        }
        self.client = None;
        self.tasks.shutdown().await;
        dead.detach_store().await; // as on logout
        if let Some(p) = &mut self.playback {
            p.session_lost();
        }
        if let Err(e) = self.with_store(|s| s.clear()).await {
            log::warn!("could not remove the expired session: {e}");
        }
        self.emit.send(Event::SessionExpired);
    }

    async fn handle(&mut self, action: Action) {
        match action {
            Action::BeginLogin => {
                let pkce = Pkce::new();
                let url = tidal::authorize_url(&self.cfg, &pkce).to_string();
                self.pkce = Some(pkce);
                self.emit.send(Event::LoginUrl(url.clone()));
                // Some launchers (xdg-open) would block the backend until the browser exits.
                if let Err(e) = open::that_detached(&url) {
                    log::warn!("could not open the browser: {e}");
                }
            }
            Action::FinishLogin(Redirect(redirect)) => self.finish_login(&redirect).await,
            Action::Logout => {
                if let Some(t) = self.search_task.take() {
                    t.abort();
                }
                let client = self.client.take();
                self.tasks.shutdown().await; // before the cache is cleared
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
            Action::Search(q) => {
                let Some(c) = self.client.clone() else { return };
                let (emit, msgs) = (self.emit.clone(), self.msgs.clone());
                // A newer search supersedes the in-flight one.
                if let Some(old) = self.search_task.take() {
                    old.abort();
                }
                emit.send(Event::SearchStarted(q.clone()));
                self.search_task = Some(tokio::spawn(async move {
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
mod tests {
    use super::*;
    use crate::model::Route;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Records saves and clears; the session is read only at startup.
    #[derive(Default)]
    struct Calls(std::sync::Mutex<Vec<&'static str>>);

    impl Calls {
        fn push(&self, call: &'static str) -> tidal::Result<()> {
            self.0.lock().expect("calls").push(call);
            Ok(())
        }

        fn get(&self) -> Vec<&'static str> {
            self.0.lock().expect("calls").clone()
        }
    }

    impl TokenStore for Calls {
        fn load(&self) -> tidal::Result<Option<tidal::Session>> {
            panic!("the stored session was read again");
        }
        fn save(&self, _: &tidal::Session) -> tidal::Result<()> {
            self.push("save")
        }
        fn clear(&self) -> tidal::Result<()> {
            self.push("clear")
        }
    }

    fn session(access_token: &str) -> tidal::Session {
        tidal::Session {
            access_token: access_token.into(),
            refresh_token: "r".into(),
            expires_at: 0,
            user_id: 1,
            country_code: "US".into(),
        }
    }

    /// Every read of the OS store can raise a macOS keychain prompt.
    struct NoReads;

    impl TokenStore for NoReads {
        fn load(&self) -> tidal::Result<Option<tidal::Session>> {
            panic!("the stored session was read again");
        }
        fn save(&self, _: &tidal::Session) -> tidal::Result<()> {
            Ok(())
        }
        fn clear(&self) -> tidal::Result<()> {
            Ok(())
        }
    }

    /// Blocks on a runtime of its own, as the Linux store (zbus) does: that panics on an
    /// async runtime thread.
    struct OwnRuntime;

    impl OwnRuntime {
        fn block() {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a runtime")
                .block_on(async {});
        }
    }

    impl TokenStore for OwnRuntime {
        fn load(&self) -> tidal::Result<Option<tidal::Session>> {
            Self::block();
            Ok(None)
        }
        fn save(&self, _: &tidal::Session) -> tidal::Result<()> {
            Self::block();
            Ok(())
        }
        fn clear(&self) -> tidal::Result<()> {
            Self::block();
            Ok(())
        }
    }

    /// A backend on `store`, whose startup probe read `stored`; its events and messages.
    fn new_backend(
        store: Arc<dyn TokenStore>,
        stored: tidal::Result<Option<tidal::Session>>,
    ) -> (Backend, Receiver<Event>, UnboundedReceiver<Msg>) {
        let (tx, events) = channel();
        let emit = Arc::new(Emitter {
            tx,
            waker: Box::new(|| {}),
        });
        let (msg_tx, msgs) = unbounded_channel();
        let b = Backend::new(emit, msg_tx, (store, stored), None);
        (b, events, msgs)
    }

    /// The events of a run with `store`, whose startup probe read `stored`, that handles
    /// `actions` and ends.
    fn run(
        store: Arc<dyn TokenStore>,
        stored: tidal::Result<Option<tidal::Session>>,
        actions: Vec<Action>,
    ) -> Vec<Event> {
        let (backend, events, msgs) = new_backend(store, stored);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let (action_tx, action_rx) = unbounded_channel();
        for a in actions {
            let _ = action_tx.send(a);
        }
        // Sender dropped: `run` handles what was sent and returns.
        drop(action_tx);
        rt.block_on(backend.run(action_rx, msgs));
        events.try_iter().collect()
    }

    fn startup(stored: tidal::Result<Option<tidal::Session>>) -> Vec<Event> {
        run(Arc::new(NoReads), stored, vec![])
    }

    /// Hands `b` the expiries in `msgs`, as its loop does.
    fn deliver(b: &mut Backend, msgs: &mut UnboundedReceiver<Msg>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        while let Ok(Msg::SessionExpired(dead)) = msgs.try_recv() {
            rt.block_on(b.expire(dead));
        }
    }

    fn revoked() -> tidal::Error {
        tidal::Error::Auth("invalid_grant: revoked".into())
    }

    /// The next user must not find this one's pages, even offline: the saved ones go, and a
    /// page request still in flight at logout neither refills the cache nor reaches the model.
    #[test]
    fn logout_clears_api_cache() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        rt.block_on(async {
            let server = MockServer::start().await;
            let slow = ResponseTemplate::new(200)
                .set_body_raw(r#"{"title":"A's","rows":[]}"#, "application/json")
                .set_delay(Duration::from_millis(300));
            Mock::given(method("GET"))
                .and(path("/v1/pages/explore"))
                .respond_with(slow)
                .mount(&server)
                .await;
            let dir = std::env::temp_dir().join("tidash-backend-logout");
            let _ = std::fs::remove_dir_all(&dir);
            let cache = DiskCache::open(dir.clone(), 1 << 20, "json").expect("a cache");
            let saved = tidal::Entry {
                etag: None,
                body: b"{}".to_vec(),
            };
            cache.write("k", &saved).expect("written");
            let (mut b, events, _msgs) = new_backend(Arc::new(NoReads), Ok(None));
            b.api_cache = Some(Arc::new(cache));
            b.cfg = ClientConfig {
                api_base: format!("{}/v1/", server.uri()).parse().expect("a URL"),
                ..ClientConfig::default()
            };
            b.log_in(session("a"));
            b.handle(Action::Open(Route::Page("pages/explore".into())))
                .await;
            let sent = async {
                while server
                    .received_requests()
                    .await
                    .is_none_or(|r| r.is_empty())
                {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(5), sent)
                .await
                .expect("the request is in flight");
            b.handle(Action::Logout).await;
            tokio::time::sleep(Duration::from_millis(600)).await; // past its reply
            let left = std::fs::read_dir(&dir).expect("the cache dir").count();
            assert_eq!(left, 0, "files left in the cache");
            let pages = events
                .try_iter()
                .filter(|e| matches!(e, Event::Page { .. }));
            assert_eq!(pages.count(), 0, "a page reached the model");
        });
    }

    #[test]
    fn logout_clears_the_store_off_the_runtime() {
        let events = run(Arc::new(OwnRuntime), Ok(None), vec![Action::Logout]);
        assert!(matches!(events[..], [Event::LoggedOut]), "{events:?}");
    }

    /// Once, however many requests found it dead; what was playing stops, the queue stays.
    #[test]
    fn an_expired_session_is_removed_and_shown_once() {
        let store = Arc::new(Calls::default());
        let (mut b, events, mut msgs) = new_backend(store.clone(), Ok(Some(session("a"))));
        let (output, _, _renderer) = player::Player::spawn_manual();
        let mut p = Playback::new(output, b.emit.clone(), b.msgs.clone());
        let track = tidal::Track {
            id: 7,
            title: "t".into(),
            duration: 200,
            track_number: 1,
            volume_number: 1,
            artists: vec![],
            album: tidal::AlbumRef {
                id: 1,
                title: "a".into(),
                cover: None,
            },
            stream_ready: true,
        };
        let tracks = vec![track];
        p.handle(None, Action::PlayTracks { tracks, start: 0 });
        let playing = player::PlayerEvent::State(player::PlayState::Playing);
        p.on_msg(None, Msg::Player(playing));
        b.playback = Some(p);
        let dead = b.client.clone().expect("logged in");
        for _ in 0..2 {
            assert!(session_expired(&revoked(), &dead, &b.msgs));
        }
        deliver(&mut b, &mut msgs);
        assert!(b.client.is_none());
        assert_eq!(store.get(), ["clear"]);
        let mut model = crate::model::Model::default();
        let mut expired = 0;
        for ev in events.try_iter() {
            expired += usize::from(matches!(ev, Event::SessionExpired));
            model.apply(ev);
        }
        assert_eq!(expired, 1);
        assert!(!model.logged_in);
        assert_eq!(model.playback.state, Some(player::PlayState::Stopped));
        assert_eq!(model.playback.track.map(|t| t.id), Some(7));
        assert_eq!(model.queue.tracks.len(), 1, "the queue stays");
    }

    /// A request issued on the old session that fails only after a new login says nothing
    /// about the new session.
    #[test]
    fn an_old_sessions_late_expiry_leaves_a_new_login_alone() {
        let store = Arc::new(Calls::default());
        let (mut b, events, mut msgs) = new_backend(store.clone(), Ok(Some(session("old"))));
        let old = b.client.clone().expect("logged in");
        assert!(session_expired(&revoked(), &old, &b.msgs)); // the first to find it dead
        deliver(&mut b, &mut msgs);
        b.log_in(session("new"));
        let new = b.client.clone().expect("logged in again");
        assert!(session_expired(&revoked(), &old, &b.msgs)); // a straggler
        deliver(&mut b, &mut msgs);
        assert!(
            b.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &new)),
            "still logged in"
        );
        let events: Vec<_> = events.try_iter().collect();
        assert!(
            matches!(
                events[..],
                [
                    Event::LoggedIn(1),
                    Event::SessionExpired,
                    Event::LoggedIn(1)
                ]
            ),
            "{events:?}"
        );
        assert_eq!(store.get(), ["clear"], "the new session is not cleared");
    }

    /// A play report that goes out after Logout (from a task still holding the old client)
    /// with a stale token refreshes it, but must not write it back to the store.
    #[test]
    fn a_report_refresh_after_logout_is_not_saved() {
        // JWT-shaped: the report carries the token's claims.
        const OLD: &str = "eyJhbGciOiJub25lIn0.eyJ1aWQiOjEsImNpZCI6Miwic2lkIjoicyJ9.old";
        const NEW: &str = "eyJhbGciOiJub25lIn0.eyJ1aWQiOjEsImNpZCI6Miwic2lkIjoicyJ9.new";
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        rt.block_on(async {
            let server = MockServer::start().await;
            for (token, status) in [(OLD, 401), (NEW, 200)] {
                Mock::given(method("POST"))
                    .and(path("/events"))
                    .and(header("authorization", format!("Bearer {token}")))
                    .respond_with(ResponseTemplate::new(status))
                    .expect(1)
                    .mount(&server)
                    .await;
            }
            Mock::given(method("POST"))
                .and(path("/oauth2/token"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    format!(r#"{{"access_token":"{NEW}","expires_in":14400}}"#),
                    "application/json",
                ))
                .expect(1)
                .mount(&server)
                .await;
            let store = Arc::new(Calls::default());
            let (tx, _events) = channel();
            let emit = Arc::new(Emitter {
                tx,
                waker: Box::new(|| {}),
            });
            let mut backend =
                Backend::new(emit, unbounded_channel().0, (store.clone(), Ok(None)), None);
            backend.cfg = ClientConfig {
                events_url: format!("{}/events", server.uri()).parse().expect("a URL"),
                auth_base: format!("{}/", server.uri()).parse().expect("a URL"),
                ..ClientConfig::default()
            };
            backend.log_in(session(OLD));
            let old = backend.client.clone().expect("logged in");
            backend.handle(Action::Logout).await;
            let report = tidal::PlaybackReport {
                track_id: 1,
                quality: "LOSSLESS".into(),
                streaming_session_id: "p".into(),
                started_at_ms: 0,
                ended_at_ms: 60_000,
                start_position_s: 0.0,
                end_position_s: 60.0,
                source_type: "ALBUM".into(),
                source_id: "1".into(),
            };
            old.report_playback(&report).await.expect("reported");
            assert_eq!(store.get(), ["clear"]);
        });
    }

    #[test]
    fn stored_session_logs_in_without_a_second_read() {
        let events = startup(Ok(Some(session("a"))));
        assert!(matches!(events[..], [Event::LoggedIn(1)]), "{events:?}");
    }

    #[test]
    fn unreadable_store_warns_and_stays_logged_out() {
        let events = startup(Err(tidal::Error::Store("denied".into())));
        assert!(
            matches!(events[..], [Event::CredentialsNotPersistent]),
            "{events:?}"
        );
    }
}
