//! The page loader: a route's saved copy at once, then the fresh one (spec §6, §9); and the
//! favorite writes, which make a collection tab stale.

use crate::backend::{Emitter, session_expired};
use crate::i18n::tr;
use crate::model::{CollectionTab, Event, PageData, Route};
use crate::playback::Msg;
use std::sync::Arc;
use tidal::{Client, Favorite, Item};
use tokio::sync::mpsc::UnboundedSender;

/// `route`'s page from `c` (a `cache_only` client reads the saved copy).
pub async fn fetch(c: &Client, route: &Route) -> tidal::Result<PageData> {
    fn items<T>(v: Vec<T>, kind: fn(T) -> Item) -> PageData {
        PageData::Items(v.into_iter().map(kind).collect())
    }
    Ok(match route {
        Route::Home => PageData::Shelves(c.home().await?),
        Route::Page(path) => PageData::Shelves(c.page(path).await?),
        Route::ViewAll(path) => PageData::Shelves(c.view_all(path).await?),
        Route::Album(id) => {
            let (album, tracks) = tokio::try_join!(c.album(*id), c.album_tracks(*id))?;
            PageData::Album { album, tracks }
        }
        Route::Artist(id) => PageData::Artist(c.artist_page(*id).await?),
        Route::Playlist(uuid) => {
            // Task 7 keeps the positions and the ETag for editing.
            let (playlist, items) = tokio::try_join!(c.playlist(uuid), c.playlist_items(uuid))?;
            PageData::Playlist {
                playlist,
                tracks: items.tracks,
            }
        }
        Route::Mix(id) => {
            let (mix, tracks) = c.mix(id).await?;
            PageData::Mix { mix, tracks }
        }
        Route::Collection(tab) => match tab {
            CollectionTab::Tracks => PageData::Tracks(c.favorite_tracks().await?),
            CollectionTab::Albums => items(c.favorite_albums().await?, Item::Album),
            CollectionTab::Artists => items(c.favorite_artists().await?, Item::Artist),
            CollectionTab::Playlists => items(c.favorite_playlists().await?, Item::Playlist),
            CollectionTab::Mixes => items(c.favorite_mixes().await?, Item::Mix),
        },
        // No page; `Model::needs_load` never asks for one.
        Route::Login | Route::Search | Route::Settings => PageData::Items(Vec::new()),
    })
}

/// Loads `route`: the saved copy at once, then the fresh one. After a saved copy, a refresh
/// that cannot reach TIDAL only marks the app offline and one TIDAL refuses keeps the copy;
/// with nothing shown the page fails. The backend spawns it, to abort it on logout.
pub async fn open(c: Arc<Client>, route: Route, emit: Arc<Emitter>, msgs: UnboundedSender<Msg>) {
    let saved = fetch(&c.cache_only(), &route).await.ok();
    let shown = saved.is_some();
    if let Some(data) = saved {
        let route = route.clone();
        emit.send(Event::Page { route, data });
    }
    match fetch(&c, &route).await {
        Ok(data) => {
            emit.send(Event::Page { route, data });
            emit.send(Event::Offline(false));
        }
        Err(e) if session_expired(&e, &c, &msgs) => {}
        Err(tidal::Error::Http(e)) if shown => {
            log::info!("showing the saved {route:?}: {e}");
            emit.send(Event::Offline(true));
        }
        Err(e) if shown => emit.send(Event::Error(format!(
            "{}: {e}",
            tr("Could not refresh this page")
        ))),
        Err(e) => {
            emit.send(Event::PageFailed(route));
            emit.send(Event::Error(format!(
                "{}: {e}",
                tr("Could not load the page")
            )));
        }
    }
}

/// Writes a favorite the UI shows already (`Event::FavoriteSet`), the item's only write under
/// way. Done, its collection tab is stale; failed, the heart goes back and one error says why.
/// The backend spawns it.
pub async fn set_favorite(
    c: Arc<Client>,
    item: Favorite,
    on: bool,
    emit: Arc<Emitter>,
    msgs: UnboundedSender<Msg>,
) {
    match c.set_favorite(&item, on).await {
        Ok(()) => emit.send(Event::FavoriteSaved { item }),
        Err(e) if session_expired(&e, &c, &msgs) => {}
        Err(e) => {
            emit.send(Event::FavoriteFailed { item, on });
            emit.send(Event::Error(format!(
                "{}: {e}",
                tr("Could not update favorites")
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use std::sync::mpsc::channel;
    use tidal::{ClientConfig, DiskCache, MemoryStore, Session};
    use tokio::sync::mpsc::unbounded_channel;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const EXPLORE: &str = include_str!("../../tidal/tests/fixtures/explore.json");

    fn explore() -> Route {
        Route::Page("pages/explore".into())
    }

    /// An empty API cache of its own.
    fn cache(name: &str) -> Arc<DiskCache> {
        let dir = std::env::temp_dir().join("tidash-pages").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        Arc::new(DiskCache::open(dir, 1 << 20, "json").expect("a cache"))
    }

    /// A client of the server at `base`, on `cache`.
    fn client(base: &str, cache: &Arc<DiskCache>) -> Arc<Client> {
        let url = |p: &str| format!("{base}/{p}").parse().expect("a URL");
        let cfg = ClientConfig {
            api_base: url("v1/"),
            api_v2_base: url("v2/"),
            auth_base: url("v1/"),
            ..ClientConfig::default()
        };
        let session = Session {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: u64::MAX,
            user_id: 1,
            country_code: "US".into(),
        };
        let store = Arc::new(MemoryStore::default());
        Arc::new(Client::new(cfg, session, store, Some(Arc::clone(cache))).expect("a client"))
    }

    /// Serves the explore fixture: `times` requests, or every one with `None`.
    async fn serve_explore(server: &MockServer, times: Option<u64>) {
        let mock = Mock::given(method("GET"))
            .and(path("/v1/pages/explore"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(EXPLORE, "application/json"));
        match times {
            Some(n) => mock.up_to_n_times(n).mount(server).await,
            None => mock.mount(server).await,
        }
    }

    /// Stops `server`; returns once its port refuses connections.
    fn stop(server: MockServer) {
        let addr = *server.address();
        drop(server);
        let closed = (0..1000).any(|_| std::net::TcpStream::connect(addr).is_err());
        assert!(closed, "{addr} still answers");
    }

    /// The events `task` sends, and whether it found the session dead.
    async fn events_of<F: Future<Output = ()>>(
        task: impl FnOnce(Arc<Emitter>, UnboundedSender<Msg>) -> F,
    ) -> (Vec<Event>, bool) {
        let (tx, events) = channel();
        let emit = Arc::new(Emitter {
            tx,
            waker: Box::new(|| {}),
        });
        let (msgs, mut msg_rx) = unbounded_channel();
        task(emit, msgs).await;
        let expired = matches!(msg_rx.try_recv(), Ok(Msg::SessionExpired(..)));
        (events.try_iter().collect(), expired)
    }

    /// The events of loading the explore page with `c`, and whether it found the session dead.
    async fn load_explore(c: Arc<Client>) -> (Vec<Event>, bool) {
        events_of(|emit, msgs| open(c, explore(), emit, msgs)).await
    }

    /// A model that shows `item`'s heart flipped to `on`, after `events` of its write.
    fn after_write(item: &Favorite, on: bool, events: Vec<Event>) -> Model {
        let mut m = Model::default();
        m.apply(Event::LoggedIn(1));
        m.apply(Event::Page {
            route: Route::Collection(CollectionTab::Tracks),
            data: PageData::Tracks(vec![]),
        });
        m.apply(Event::FavoriteSet {
            item: item.clone(),
            on,
        });
        for ev in events {
            m.apply(ev);
        }
        m
    }

    /// The heart shows the change already: a refused write puts it back and says so once.
    #[tokio::test]
    async fn failed_write_reports_one_error_and_reverts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/users/1/favorites/tracks"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let (c, item) = (
            client(&server.uri(), &cache("fav-fail")),
            Favorite::Track(7),
        );
        let write = |emit, msgs| set_favorite(c, item.clone(), true, emit, msgs);
        let (events, expired) = events_of(write).await;
        assert!(!expired);
        assert!(
            matches!(
                &events[..],
                [
                    Event::FavoriteFailed {
                        item: Favorite::Track(7),
                        on: true
                    },
                    Event::Error(_)
                ]
            ),
            "{events:?}"
        );
        let m = after_write(&item, true, events);
        assert!(!m.favorites.contains(&item), "reverted");
        assert!(m.pending_favorites.is_empty());
        assert_eq!(m.errors.len(), 1);
    }

    /// The next visit to the collection fetches it again, with the change.
    #[tokio::test]
    async fn successful_write_marks_collection_stale() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/v1/users/1/favorites/tracks/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let (c, item) = (client(&server.uri(), &cache("fav-ok")), Favorite::Track(7));
        let write = |emit, msgs| set_favorite(c, item.clone(), false, emit, msgs);
        let (events, _) = events_of(write).await;
        assert!(
            matches!(&events[..], [Event::FavoriteSaved { item: i }] if *i == item),
            "{events:?}"
        );
        let m = after_write(&item, false, events);
        let tracks = Route::Collection(CollectionTab::Tracks);
        assert!(m.needs_load(&tracks, std::time::Instant::now()));
        assert!(!m.favorites.contains(&item));
        assert!(m.pending_favorites.is_empty());
        assert!(m.errors.is_empty());
    }

    fn title(ev: &Event) -> Option<&str> {
        match ev {
            Event::Page {
                data: PageData::Shelves(p),
                ..
            } => Some(&p.title),
            _ => None,
        }
    }

    #[tokio::test]
    async fn fresh_page_after_cached_page() {
        let server = MockServer::start().await;
        serve_explore(&server, Some(1)).await;
        Mock::given(method("GET"))
            .and(path("/v1/pages/explore"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"title":"Fresh","rows":[]}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let c = client(&server.uri(), &cache("fresh"));
        let (first, _) = load_explore(c.clone()).await;
        let titles: Vec<_> = first.iter().map(title).collect();
        assert_eq!(
            titles,
            [Some("Explore"), None],
            "nothing saved yet: {first:?}"
        );
        let (events, _) = load_explore(c).await;
        let titles: Vec<_> = events.iter().map(title).collect();
        assert_eq!(titles, [Some("Explore"), Some("Fresh"), None], "{events:?}");
        assert!(matches!(events[2], Event::Offline(false)));
    }

    #[tokio::test]
    async fn network_error_with_cache_is_offline_not_error() {
        let server = MockServer::builder().start().await; // not pooled: stopping closes it
        serve_explore(&server, None).await;
        let (base, saved) = (server.uri(), cache("offline"));
        load_explore(client(&base, &saved)).await;
        stop(server);
        let (events, expired) = load_explore(client(&base, &saved)).await;
        assert!(
            matches!(events[..], [Event::Page { .. }, Event::Offline(true)]),
            "{events:?}"
        );
        assert!(!expired);
    }

    /// Online, but the refresh is refused (a deleted playlist's 404): not "offline".
    #[tokio::test]
    async fn refused_refresh_with_cache_keeps_the_page_and_reports_once() {
        let server = MockServer::start().await;
        serve_explore(&server, Some(1)).await; // then 404s
        let c = client(&server.uri(), &cache("refused"));
        load_explore(c.clone()).await;
        let (events, expired) = load_explore(c).await;
        assert!(
            matches!(events[..], [Event::Page { .. }, Event::Error(_)]),
            "{events:?}"
        );
        assert!(!expired);
    }

    /// One error, which leaves out the request's URL (its host and query).
    #[tokio::test]
    async fn network_error_without_cache_fails_once() {
        let server = MockServer::builder().start().await;
        let base = server.uri();
        stop(server);
        let (events, expired) = load_explore(client(&base, &cache("failed"))).await;
        let [Event::PageFailed(_), Event::Error(e)] = &events[..] else {
            panic!("{events:?}");
        };
        assert!(!e.contains(&base) && !e.contains("countryCode"), "{e}");
        assert!(!expired);
    }

    #[tokio::test]
    async fn auth_error_expires_session() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth2/token"))
            .respond_with(ResponseTemplate::new(400).set_body_raw(
                r#"{"error":"invalid_grant","error_description":"revoked"}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let (events, expired) = load_explore(client(&server.uri(), &cache("auth"))).await;
        assert!(expired);
        assert!(events.is_empty(), "the expiry says it all: {events:?}");
    }
}
