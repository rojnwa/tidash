//! Tokio backend on its own thread: receives `Action`s, sends `Event`s.

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
        let emit = Emitter { tx: event_tx, waker: Box::new(waker) };
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

struct Backend {
    emit: Emitter,
    cfg: ClientConfig,
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    persistent: bool,
    pkce: Option<Pkce>,
    // Shared with spawned tasks by later tasks.
    client: Option<Arc<Client>>,
}

impl Backend {
    fn new(emit: Emitter) -> Self {
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
            Err(e) => self.emit.send(Event::Error(format!("Login failed: {e}"))),
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
                match self.store.clear() {
                    Ok(()) => self.emit.send(Event::LoggedOut),
                    Err(e) => self.emit.send(Event::Error(format!("Logout failed: {e}"))),
                }
            }
            // ponytail: search, albums, playback and images arrive in tasks 18-20.
            _ => {}
        }
    }

    async fn finish_login(&mut self, redirect: &str) {
        let Some(pkce) = self.pkce.clone() else {
            self.emit.send(Event::Error("Login failed: start with \"Log in with TIDAL\" first".into()));
            return;
        };
        let session = match tidal::code_from_redirect(redirect, &pkce.state) {
            Ok(code) => tidal::exchange_code(&self.http, &self.cfg, &code, &pkce).await,
            Err(e) => Err(e),
        };
        match session {
            Ok(session) => {
                if let Err(e) = self.store.save(&session) {
                    self.emit.send(Event::Error(format!("Login will not be remembered: {e}")));
                }
                self.pkce = None;
                self.log_in(session);
            }
            Err(e) => self.emit.send(Event::Error(format!("Login failed: {e}"))),
        }
    }
}
