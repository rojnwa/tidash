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
    let b = Backend::new(emit, msg_tx, Settings::default(), (store, stored), None);
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
    while let Ok(Msg::SessionExpired(dead, refused)) = msgs.try_recv() {
        rt.block_on(b.expire(dead, refused));
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
                Some(dead) if !logout => b.expire(dead, false).await,
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

/// Track 7, of album 1.
fn track() -> tidal::Track {
    tidal::Track {
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
        ..Default::default()
    }
}

/// Its `clear` takes a while, as a keychain prompt can: logout waits on it after it has
/// cleared the cache.
struct SlowClear;

impl TokenStore for SlowClear {
    fn load(&self) -> tidal::Result<Option<tidal::Session>> {
        panic!("the stored session was read again");
    }
    fn save(&self, _: &tidal::Session) -> tidal::Result<()> {
        Ok(())
    }
    fn clear(&self) -> tidal::Result<()> {
        std::thread::sleep(Duration::from_millis(400));
        Ok(())
    }
}

/// Ruling R24: the stream load, the one session request outside `tasks`, stops before logout
/// clears the cache. Its reply lands while logout still waits on the store, and writes nothing
/// to the cache nor reports back.
#[test]
fn a_stream_load_in_flight_at_logout_stops_before_the_cache_clear() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        let server = MockServer::start().await;
        let info = r#"{"trackId":7,"audioQuality":"LOSSLESS",
            "manifestMimeType":"application/dash+xml","manifest":""}"#;
        let slow = ResponseTemplate::new(200)
            .set_body_raw(info, "application/json")
            .set_delay(Duration::from_millis(100));
        let mock = Mock::given(method("GET")).and(path("/v1/tracks/7/playbackinfo"));
        mock.respond_with(slow).mount(&server).await;
        let dir = std::env::temp_dir().join("tidash-backend-load");
        let _ = std::fs::remove_dir_all(&dir);
        let cache = DiskCache::open(dir.clone(), 1 << 20, "json").expect("a cache");
        let (mut b, _events, mut msgs) = new_backend(Arc::new(SlowClear), Ok(None));
        b.api_cache = Some(Arc::new(cache));
        b.cfg = ClientConfig {
            api_base: format!("{}/v1/", server.uri()).parse().expect("a URL"),
            ..ClientConfig::default()
        };
        b.log_in(session("a"));
        let (output, _, _renderer) = player::Player::spawn_manual();
        let audio = Default::default();
        b.playback = Some(Playback::new(output, b.emit.clone(), b.msgs.clone(), audio));
        let tracks = vec![track()];
        b.handle(Action::PlayTracks { tracks, start: 0 }).await;
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
            .expect("the stream request is in flight");
        b.handle(Action::Logout).await; // the reply lands meanwhile
        let left = std::fs::read_dir(&dir).expect("the cache dir").count();
        assert_eq!(left, 0, "files in the cache");
        assert!(msgs.try_recv().is_err(), "the load reported back");
    });
}

/// No lyrics is a 404: ready, with none. Another failure is `Failed`, which the panel shows:
/// no `Error` event.
#[test]
fn lyrics_fetch_404_is_ready_none() {
    use crate::model::Load;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        let server = MockServer::start().await;
        for (at, status) in [("/v1/tracks/7/lyrics", 404), ("/v1/tracks/7/credits", 403)] {
            let mock = Mock::given(method("GET")).and(path(at));
            let reply = ResponseTemplate::new(status);
            mock.respond_with(reply).expect(1).mount(&server).await;
        }
        let (mut b, events, _msgs) = new_backend(Arc::new(NoReads), Ok(None));
        b.cfg = ClientConfig {
            api_base: format!("{}/v1/", server.uri()).parse().expect("a URL"),
            ..ClientConfig::default()
        };
        b.log_in(session("a"));
        for action in [Action::LoadLyrics(7), Action::LoadCredits(7)] {
            b.handle(action).await;
            while b.tasks.join_next().await.is_some() {} // one at a time: the events' order
        }
        let events: Vec<_> = events.try_iter().skip(1).collect(); // past `LoggedIn`
        assert!(
            matches!(
                &events[..],
                [
                    Event::Lyrics(7, Load::Ready(None)),
                    Event::Credits(7, Load::Failed)
                ]
            ),
            "{events:?}"
        );
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
    let mut p = Playback::new(output, b.emit.clone(), b.msgs.clone(), Default::default());
    let tracks = vec![track()];
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
        let (msgs, settings) = (unbounded_channel().0, Settings::default());
        let mut backend = Backend::new(emit, msgs, settings, (store.clone(), Ok(None)), None);
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

/// The change is saved (creating the config dir) and the model told.
#[test]
fn set_settings_saves_and_announces() {
    let dir = std::env::temp_dir().join(format!("tidash-backend-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("config").join("settings.json");
    let (mut b, events, _msgs) = new_backend(Arc::new(NoReads), Ok(None));
    b.settings_path = Some(file.clone());
    let mut settings = Settings::default();
    settings.audio.exclusive = true;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(b.handle(Action::SetSettings(settings.clone())));
    let saved = std::fs::read_to_string(&file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(saved.contains(r#""exclusive": true"#), "{saved}");
    let events: Vec<_> = events.try_iter().collect();
    assert!(
        matches!(&events[..], [Event::Settings(s)] if *s == settings),
        "{events:?}"
    );
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

fn errors(events: &Receiver<Event>) -> Vec<String> {
    let error = |e| match e {
        Event::Error(e) => Some(e),
        _ => None,
    };
    events.try_iter().filter_map(error).collect()
}

/// A failed flush arms one retry, however many fail before it runs. A refused token clears
/// the user, and is shown once.
#[test]
fn scrobbler_outcomes_arm_one_retry_and_ask_once() {
    let (mut b, events, _msgs) = new_backend(Arc::new(NoReads), Ok(None));
    b.settings.integrations.listenbrainz_user = Some("rob".into());
    runtime().block_on(async {
        let retry = || Outcome {
            retry: true,
            refused: vec![],
        };
        b.on_msg(Msg::Scrobbled(retry())).await;
        let armed = b.scrobble_retry_at.expect("armed");
        b.on_msg(Msg::Scrobbled(retry())).await;
        assert_eq!(b.scrobble_retry_at, Some(armed), "one at a time");
        for _ in 0..2 {
            let refused = vec![Service::ListenBrainz];
            let out = Outcome {
                retry: false,
                refused,
            };
            b.on_msg(Msg::Scrobbled(out)).await;
        }
    });
    assert_eq!(b.settings.integrations.listenbrainz_user, None);
    let refused = "ListenBrainz refused the token. Connect again in Settings.";
    assert_eq!(errors(&events), [refused]);
}

/// The next `Msg::Connected` the backend's tasks send.
async fn next_connected(msgs: &mut UnboundedReceiver<Msg>) -> Msg {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(5), msgs.recv()).await;
        let msg = next.expect("in time").expect("a message");
        if matches!(msg, Msg::Connected(..)) {
            return msg;
        }
    }
}

/// Connect checks the token and keeps it; the settings name the user, and an edit the UI made
/// before it heard does not undo that. Disconnect forgets both.
#[test]
fn connect_and_disconnect_set_the_user() {
    runtime().block_on(async {
        let server = MockServer::start().await;
        let valid = r#"{"code":200,"message":"Token valid.","valid":true,"user_name":"rob"}"#;
        Mock::given(method("GET"))
            .and(path("/1/validate-token"))
            .and(header("authorization", "Token good"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(valid, "application/json"))
            .mount(&server)
            .await;
        let store = Arc::new(tidal::MemoryStore::default());
        let (mut b, events, mut msgs) = new_backend(store.clone(), Ok(None));
        let urls = Urls {
            listenbrainz: format!("{}/", server.uri()).parse().expect("a URL"),
            ..Urls::default()
        };
        b.scrobbler = Scrobbler::new(b.http.clone(), store.clone(), None, urls, &[]);
        let secret = || store.secret("listenbrainz").expect("read");
        let token = Secret("good".into());
        b.handle(Action::ConnectListenBrainz(token)).await;
        b.on_msg(next_connected(&mut msgs).await).await;
        assert_eq!(
            b.settings.integrations.listenbrainz_user.as_deref(),
            Some("rob")
        );
        assert_eq!(secret().as_deref(), Some("good"));
        b.handle(Action::SetSettings(Settings::default())).await;
        assert_eq!(
            b.settings.integrations.listenbrainz_user.as_deref(),
            Some("rob")
        );
        b.handle(Action::DisconnectListenBrainz).await;
        b.on_msg(next_connected(&mut msgs).await).await;
        assert_eq!(b.settings.integrations.listenbrainz_user, None);
        assert_eq!(secret(), None);
        let shown = |e: Event| match e {
            Event::Settings(s) => Some(s.integrations.listenbrainz_user),
            _ => None,
        };
        let users: Vec<_> = events.try_iter().filter_map(shown).collect();
        assert_eq!(users, [Some("rob".into()), Some("rob".into()), None]);
    });
}

const CLIENT_ID_CHANGED: &str = "Logged out: the client ID changed. Log in again.";
const CLIENT_REFUSED: &str = "TIDAL refused the client ID. Check Settings › Account.";

/// Another client ID ends the session it logged in with: logged out, told why once. Other
/// changes, and a blank ID (the built-in one), leave it alone.
#[test]
fn changing_the_client_id_logs_out() {
    let store = Arc::new(tidal::MemoryStore::default());
    store.save(&session("a")).expect("saved");
    let (mut b, events, _msgs) = new_backend(store.clone(), Ok(Some(session("a"))));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let mut settings = Settings::default();
    settings.theme = crate::settings::Theme::Light;
    settings.account.client_id = Some("  ".into());
    rt.block_on(b.handle(Action::SetSettings(settings.clone())));
    assert!(b.client.is_some(), "still logged in");
    settings.account.client_id = Some("other".into());
    rt.block_on(b.handle(Action::SetSettings(settings.clone())));
    assert!(b.client.is_none());
    assert_eq!(b.cfg.client_id, "other");
    assert_eq!(store.load().expect("read"), None, "the session is removed");
    let events: Vec<_> = events.try_iter().collect();
    let out = events.iter().filter(|e| matches!(e, Event::LoggedOut));
    assert_eq!(out.count(), 1, "{events:?}");
    let errors: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Error(e) => Some(e.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(errors, [CLIENT_ID_CHANGED]);
    assert!(
        matches!(events.last(), Some(Event::Error(_))),
        "after LoggedOut: {events:?}"
    );
}

/// Logged out there is nothing to log out of: the override is saved, and the next login's
/// authorize URL carries it. (`begin_login` is `BeginLogin` without opening the browser.)
#[test]
fn client_id_set_while_logged_out_is_used_by_the_next_login() {
    let (mut b, events, _msgs) = new_backend(Arc::new(NoReads), Ok(None));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let mut settings = Settings::default();
    settings.account.client_id = Some("mine".into());
    settings.account.redirect_uri = Some("https://example.test/cb".into());
    rt.block_on(b.handle(Action::SetSettings(settings)));
    let url = b.begin_login();
    let events: Vec<_> = events.try_iter().collect();
    assert!(
        matches!(&events[..], [Event::Settings(_), Event::LoginUrl(u)] if *u == url),
        "{events:?}"
    );
    let url = reqwest::Url::parse(&url).expect("a URL");
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], "mine");
    assert_eq!(query["redirect_uri"], "https://example.test/cb");
}

/// The token endpoint refuses the client ID, on the login exchange and on a refresh (which
/// first ends the session): the error points to the setting.
#[test]
fn invalid_client_points_to_the_setting() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        let server = MockServer::start().await;
        let refused = ResponseTemplate::new(400)
            .set_body_raw(r#"{"error":"invalid_client"}"#, "application/json");
        let token = Mock::given(method("POST")).and(path("/oauth2/token"));
        token.respond_with(refused).expect(2).mount(&server).await;
        let search = Mock::given(method("GET")).and(path("/v1/search"));
        search
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let (mut b, events, mut msgs) = new_backend(Arc::new(NoReads), Ok(None));
        b.cfg = ClientConfig {
            api_base: format!("{}/v1/", server.uri()).parse().expect("a URL"),
            auth_base: format!("{}/", server.uri()).parse().expect("a URL"),
            ..ClientConfig::default()
        };
        b.pkce = Some(tidal::Pkce::from_parts("v", "st"));
        let redirect = Redirect("https://x.test/cb?code=c&state=st".into());
        b.handle(Action::FinishLogin(redirect)).await;
        b.log_in(session("a"));
        b.handle(Action::Search("q".into())).await;
        let expired = tokio::time::timeout(Duration::from_secs(5), msgs.recv()).await;
        let Ok(Some(Msg::SessionExpired(dead, refused))) = expired else {
            panic!("no expiry");
        };
        b.expire(dead, refused).await;
        let events: Vec<_> = events.try_iter().collect();
        let shown: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Error(e) => Some(e.as_str()),
                Event::SessionExpired => Some("expired"),
                _ => None,
            })
            .collect();
        assert_eq!(shown, [CLIENT_REFUSED, "expired", CLIENT_REFUSED]);
    });
}
