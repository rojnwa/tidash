#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;
use tidal::{Artist, Client, ClientConfig, Contributor, MemoryStore, Mixes, Session, Track};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEARCH: &str = include_str!("fixtures/search.json");
const PAGE1: &str = include_str!("fixtures/album_items_page1.json");
const PAGE2: &str = include_str!("fixtures/album_items_page2.json");
const TRACK: &str = include_str!("fixtures/track.json");

fn setup(server: &MockServer) -> Client {
    let base = format!("{}/v1/", server.uri());
    let cfg = ClientConfig {
        api_base: base.parse().unwrap(),
        auth_base: base.parse().unwrap(),
        ..ClientConfig::default()
    };
    let session = Session {
        access_token: "ACCESS".into(),
        refresh_token: "REFRESH".into(),
        expires_at: u64::MAX,
        user_id: 1,
        country_code: "US".into(),
    };
    Client::new(cfg, session, Arc::new(MemoryStore::default()), None).unwrap()
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

#[tokio::test]
async fn search_parses_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/search"))
        .and(query_param("types", "ARTISTS,ALBUMS,PLAYLISTS,TRACKS"))
        .respond_with(json(SEARCH))
        .mount(&server)
        .await;
    let r = setup(&server).search("radiohead", 5).await.unwrap();
    assert!(!r.albums.is_empty() && !r.tracks.is_empty());
    assert!(r.tracks[0].id > 0 && !r.tracks[0].title.is_empty());
    assert_eq!(r.playlists.len(), 1);
    assert_eq!(r.playlists[0].uuid, "edb69697-fc65-4580-b938-55f0f4bd7c54");
}

#[tokio::test]
async fn search_query_is_url_encoded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/search"))
        .and(query_param("query", "AC/DC & Björk"))
        .respond_with(json("{}"))
        .expect(1)
        .mount(&server)
        .await;
    let r = setup(&server).search("AC/DC & Björk", 5).await.unwrap();
    assert!(r.artists.is_empty() && r.albums.is_empty() && r.playlists.is_empty());
    assert!(r.tracks.is_empty());
}

#[tokio::test]
async fn album_tracks_paginates_until_total() {
    let server = MockServer::start().await;
    for (off, body) in [("0", PAGE1), ("50", PAGE2)] {
        Mock::given(method("GET"))
            .and(path("/v1/albums/283039787/items"))
            .and(query_param("offset", off))
            .and(query_param("limit", "50"))
            .respond_with(json(body))
            .expect(1)
            .mount(&server)
            .await;
    }
    let total = serde_json::from_str::<serde_json::Value>(PAGE1).unwrap()["totalNumberOfItems"]
        .as_u64()
        .unwrap() as usize;
    let tracks = setup(&server).album_tracks(283039787).await.unwrap();
    assert_eq!(tracks.len(), total);
    let keys: Vec<_> = tracks
        .iter()
        .map(|t| (t.volume_number, t.track_number))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[tokio::test]
async fn album_parses_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/albums/283039787"))
        .respond_with(json(include_str!("fixtures/album.json")))
        .mount(&server)
        .await;
    let a = setup(&server).album(283039787).await.unwrap();
    assert_eq!(a.number_of_tracks, 63);
    assert_eq!(a.artists.len(), 3);
}

#[tokio::test]
async fn lyrics_from_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tracks/1550546/lyrics"))
        .respond_with(json(include_str!("fixtures/lyrics.json")))
        .mount(&server)
        .await;
    let l = setup(&server).lyrics(1550546).await.unwrap().unwrap();
    assert_eq!(l.lines.len(), 6);
    assert_eq!(l.lines[0].at, Duration::from_millis(30_370));
    assert_eq!(l.lines[0].text, "One more time");
    assert_eq!(l.lines[1].text, "");
    assert!(l.text.starts_with("One more time\n\n"));
    assert!(!l.rtl);
    assert_eq!(l.provider.as_deref(), Some("MUSIXMATCH"));
}

#[tokio::test]
async fn lyrics_404_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tracks/20115564/lyrics"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_raw(include_str!("fixtures/lyrics_404.json"), "application/json"),
        )
        .mount(&server)
        .await;
    assert_eq!(setup(&server).lyrics(20115564).await.unwrap(), None);
}

#[tokio::test]
async fn track_credits_bare_array() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tracks/20115564/credits"))
        .and(query_param("limit", "100"))
        .and(query_param("includeContributors", "true"))
        .respond_with(json(include_str!("fixtures/track_credits.json")))
        .mount(&server)
        .await;
    let c = setup(&server).track_credits(20115564).await.unwrap();
    assert_eq!(c.len(), 3);
    assert_eq!(c[0].role, "Producer");
    assert_eq!(
        c[0].contributors[0],
        Contributor {
            name: "Thomas Bangalter".into(),
            id: Some(3538385)
        }
    );
    assert_eq!(c[1].contributors.len(), 3);
}

#[test]
fn mix_ids_parse() {
    let t: Track = serde_json::from_str(TRACK).unwrap();
    assert_eq!(
        t.mixes.track.as_deref(),
        Some("001a520287326eaf8639f1b722a74f")
    );
    let page: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/artist_page.json")).unwrap();
    let a: Artist = serde_json::from_value(page["item"]["data"].clone()).unwrap();
    assert_eq!(
        a.mixes.artist.as_deref(),
        Some("0002737adb8d077f4591c4cdb7ba60")
    );
    for raw in [
        r#"{"id":1,"title":"t","album":{"id":2,"title":"a"},"mixes":{}}"#,
        r#"{"id":1,"title":"t","album":{"id":2,"title":"a"},"mixes":null}"#,
        r#"{"id":1,"title":"t","album":{"id":2,"title":"a"}}"#,
    ] {
        let t: Track = serde_json::from_str(raw).unwrap();
        assert_eq!(t.mixes, Mixes::default());
    }
}

#[tokio::test]
async fn track_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tracks/20115564"))
        .respond_with(json(TRACK))
        .expect(1)
        .mount(&server)
        .await;
    let t = setup(&server).track(20115564).await.unwrap();
    assert_eq!((t.id, t.title.as_str()), (20115564, "Get Lucky"));
    assert_eq!(t.album.id, 20115556);
    assert_eq!(t.artists.len(), 3);
}
