use super::*;

/// A failed flush arms one retry, however many fail before it runs. A refused token clears
/// the user, and is shown once.
#[test]
fn scrobbler_outcomes_arm_one_retry_and_ask_once() {
    let (mut b, events, _msgs) = new_backend(Arc::new(NoReads), Ok(None));
    b.settings.integrations.listenbrainz_user = Some("rob".into());
    b.settings.integrations.lastfm_user = Some("ann".into());
    runtime().block_on(async {
        let retry = || Outcome {
            retry: true,
            refused: vec![],
        };
        b.on_msg(Msg::Scrobbled(retry())).await;
        let armed = b.scrobble_retry_at.expect("armed");
        b.on_msg(Msg::Scrobbled(retry())).await;
        assert_eq!(b.scrobble_retry_at, Some(armed), "one at a time");
        for service in [Service::ListenBrainz, Service::LastFm] {
            for _ in 0..2 {
                let refused = vec![service];
                let out = Outcome {
                    retry: false,
                    refused,
                };
                b.on_msg(Msg::Scrobbled(out)).await;
            }
        }
    });
    assert_eq!(b.settings.integrations, Default::default());
    let refused = [
        "ListenBrainz refused the token. Connect again in Settings.",
        "Last.fm needs to be connected again in Settings.",
    ];
    assert_eq!(errors(&events), refused);
}

/// Sends to the Last.fm mock at `server` with test keys, through `store`, on an HTTP client
/// without a pool timer: with time paused, a pending one would move the clock while a request
/// waits.
fn lastfm_at(b: &mut Backend, server: &MockServer, store: Arc<tidal::MemoryStore>) {
    let apis = Apis {
        lastfm: format!("{}/2.0/", server.uri()).parse().expect("a URL"),
        lastfm_keys: Some(crate::scrobble::Keys {
            key: "KEY",
            secret: "SECRET",
        }),
        ..Apis::default()
    };
    let http = reqwest::Client::builder().pool_idle_timeout(None).build();
    let http = http.expect("a client");
    b.scrobbler = Scrobbler::new(http, store, None, apis, &[]);
}

/// A Last.fm call of `method`.
fn lastfm_call(m: &str) -> wiremock::MockBuilder {
    let named = wiremock::matchers::body_string_contains(format!("method={m}"));
    Mock::given(method("POST")).and(path("/2.0/")).and(named)
}

fn json(status: u16, body: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body.to_owned(), "application/json")
}

fn paused_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .expect("a runtime")
}

/// Whether Settings shows the wait, as each event leaves it; and the Last.fm user.
fn waits_and_users(events: &Receiver<Event>) -> Vec<String> {
    let shown = |e| match e {
        Event::LastfmWaiting(w) => Some(format!("waiting {w}")),
        Event::Settings(s) => Some(format!("user {:?}", s.integrations.lastfm_user)),
        _ => None,
    };
    events.try_iter().filter_map(shown).collect()
}

/// Connect opens the page the user allows Tidash on, then asks every 3 s until they have:
/// the session key is kept, the settings name the user, and the wait ends after that.
#[test]
fn connect_polls_until_authorized() {
    paused_runtime().block_on(async {
        let server = MockServer::start().await;
        let token = json(200, r#"{"token":"TOKEN"}"#);
        lastfm_call("auth.getToken")
            .respond_with(token)
            .mount(&server)
            .await;
        let not_yet = json(403, r#"{"error":14,"message":"Unauthorized Token"}"#);
        lastfm_call("auth.getSession")
            .respond_with(not_yet)
            .up_to_n_times(2)
            .mount(&server)
            .await;
        let session = json(
            200,
            r#"{"session":{"name":"ann","key":"SK","subscriber":0}}"#,
        );
        lastfm_call("auth.getSession")
            .respond_with(session)
            .mount(&server)
            .await;
        let store = Arc::new(tidal::MemoryStore::default());
        let (mut b, events, mut msgs) = new_backend(store.clone(), Ok(None));
        lastfm_at(&mut b, &server, store.clone());
        let opened = Arc::new(std::sync::Mutex::new(Vec::new()));
        let opener = Arc::clone(&opened);
        let start = tokio::time::Instant::now();
        b.connect_lastfm(move |url| opener.lock().expect("opened").push(url));
        let wait = b.lastfm_connect.take().expect("waiting");
        wait.await.expect("done");
        assert_eq!(
            start.elapsed(),
            Duration::from_secs(9),
            "three asks, 3 s apart"
        );
        b.on_msg(next_connected(&mut msgs).await).await;
        let page = "https://www.last.fm/api/auth/?api_key=KEY&token=TOKEN";
        assert_eq!(*opened.lock().expect("opened"), [page]);
        assert_eq!(store.secret("lastfm").expect("read").as_deref(), Some("SK"));
        let user = b.settings.integrations.lastfm_user.as_deref();
        assert_eq!(user, Some("ann"));
        let shown = ["waiting true", r#"user Some("ann")"#, "waiting false"];
        assert_eq!(waits_and_users(&events), shown);
        let asks = server.received_requests().await.expect("recorded").len();
        assert_eq!(asks, 4, "a token, then three asks");
    });
}

/// Cancel stops the asking, and the wait; nothing connects.
#[test]
fn cancel_stops_the_poll() {
    paused_runtime().block_on(async {
        let server = MockServer::start().await;
        let token = json(200, r#"{"token":"TOKEN"}"#);
        lastfm_call("auth.getToken")
            .respond_with(token)
            .mount(&server)
            .await;
        let not_yet = json(403, r#"{"error":14,"message":"Unauthorized Token"}"#);
        let asking = lastfm_call("auth.getSession").respond_with(not_yet);
        asking.mount(&server).await;
        let store = Arc::new(tidal::MemoryStore::default());
        let (mut b, events, mut msgs) = new_backend(store.clone(), Ok(None));
        lastfm_at(&mut b, &server, store.clone());
        b.connect_lastfm(|_| {});
        let server = &server;
        let asks = || async move { server.received_requests().await.map_or(0, |r| r.len()) };
        let waiting = |b: &Backend| b.lastfm_connect.as_ref().is_some_and(|w| !w.is_finished());
        while asks().await < 3 && waiting(&b) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(waiting(&b), "asking still");
        b.handle(Action::CancelLastfm).await;
        let asked = asks().await;
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(asks().await, asked, "no more asks");
        assert_eq!(waits_and_users(&events), ["waiting true", "waiting false"]);
        assert!(msgs.try_recv().is_err(), "nothing connects");
        assert_eq!(store.secret("lastfm").expect("read"), None);
    });
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
        let apis = Apis {
            listenbrainz: format!("{}/", server.uri()).parse().expect("a URL"),
            ..Apis::default()
        };
        b.scrobbler = Scrobbler::new(b.http.clone(), store.clone(), None, apis, &[]);
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

/// None lost (Review Focus 3): a play queued as the UI closes, its scrobbler task not yet run,
/// is in the queue file the next start flushes.
#[test]
fn a_scrobble_at_quit_is_saved() {
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("tidash-backend-quit-{pid}"));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("scrobbles.json");
    // ListenBrainz connected without a readable token: queued, nothing sent.
    let store = Arc::new(tidal::MemoryStore::default());
    let (mut b, _events, msgs) = new_backend(store.clone(), Ok(None));
    let (queue, on) = (Some(file.clone()), [Service::ListenBrainz]);
    b.scrobbler = Scrobbler::new(b.http.clone(), store, queue, Apis::default(), &on);
    let listen = crate::scrobble::tests::listen(7, 1_700_000_100);
    let _ = b.msgs.send(Msg::Scrobble(listen));
    let (action_tx, action_rx) = unbounded_channel();
    drop(action_tx); // the UI closed
    runtime().block_on(b.run(action_rx, msgs));
    let saved = std::fs::read_to_string(&file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(saved.contains("1700000100"), "{saved:?}");
}
