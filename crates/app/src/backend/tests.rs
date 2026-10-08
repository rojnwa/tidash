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

/// The next user must not find this one's pages, even offline: logout clears the saved
/// ones. A page or a search in flight when the session ends, by logout or expiry (ruling
/// R13), neither refills the cache nor reaches the model.
#[test]
fn session_end_stops_requests_in_flight() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    for logout in [true, false] {
        rt.block_on(async {
            let server = MockServer::start().await;
            let page = r#"{"title":"A's","rows":[]}"#;
            for (at, body) in [("/v1/pages/explore", page), ("/v1/search", "{}")] {
                let slow = ResponseTemplate::new(200)
                    .set_body_raw(body, "application/json")
                    .set_delay(Duration::from_millis(300));
                let mock = Mock::given(method("GET")).and(path(at));
                mock.respond_with(slow).mount(&server).await;
            }
            let dir = std::env::temp_dir().join(format!("tidash-backend-end-{logout}"));
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
            b.handle(Action::Search("q".into())).await;
            let sent = async {
                while server.received_requests().await.is_none_or(|r| r.len() < 2) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(5), sent)
                .await
                .expect("both requests are in flight");
            match b.client.clone() {
                Some(dead) if !logout => b.expire(dead).await,
                _ => b.handle(Action::Logout).await,
            }
            tokio::time::sleep(Duration::from_millis(600)).await; // past their replies
            let left = std::fs::read_dir(&dir).expect("the cache dir").count();
            // Expiry keeps the saved page for the next login.
            assert_eq!(left, usize::from(!logout), "cache files, logout: {logout}");
            let reply = |e: &Event| matches!(e, Event::Page { .. } | Event::SearchResults { .. });
            let late: Vec<_> = events.try_iter().filter(reply).collect();
            assert!(late.is_empty(), "logout: {logout}, {late:?}");
        });
    }
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
