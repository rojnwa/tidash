#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use tidal::{Client, ClientConfig, MemoryStore, Session};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEARCH: &str = include_str!("fixtures/search.json");
const PAGE1: &str = include_str!("fixtures/album_items_page1.json");
const PAGE2: &str = include_str!("fixtures/album_items_page2.json");

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
