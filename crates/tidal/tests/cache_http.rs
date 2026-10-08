#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use tidal::{Client, ClientConfig, DiskCache, Error, MemoryStore, Quality, Session};
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type Val = serde_json::Value;

const LOSSLESS: &str = include_str!("fixtures/playbackinfo_lossless.json");

fn dir(name: &str) -> PathBuf {
    std::env::temp_dir().join("tidal-cache-http").join(name)
}

fn cache(name: &str) -> Arc<DiskCache> {
    let _ = std::fs::remove_dir_all(dir(name));
    Arc::new(DiskCache::open(dir(name), 1 << 20, "json").unwrap())
}

fn client(server: &MockServer, user_id: u64, country: &str, c: Option<Arc<DiskCache>>) -> Client {
    let cfg = ClientConfig {
        api_base: format!("{}/v1/", server.uri()).parse().unwrap(),
        api_v2_base: format!("{}/v2/", server.uri()).parse().unwrap(),
        auth_base: format!("{}/v1/", server.uri()).parse().unwrap(),
        ..ClientConfig::default()
    };
    let session = Session {
        access_token: "ACCESS".into(),
        refresh_token: "REFRESH".into(),
        expires_at: u64::MAX,
        user_id,
        country_code: country.into(),
    };
    Client::new(cfg, session, Arc::new(MemoryStore::default()), c).unwrap()
}

async fn serve(server: &MockServer, p: &str, times: u64) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"p": p})))
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn get_fills_the_cache_and_cache_only_reads_it() {
    let server = MockServer::start().await;
    serve(&server, "/v1/pages/home", 1).await;
    serve(&server, "/v2/home/feed/static", 1).await;
    let c = client(&server, 1, "US", Some(cache("fills")));
    let v1: Val = c.get("pages/home", &[]).await.unwrap();
    let v2: Val = c.get_v2("home/feed/static", &[]).await.unwrap();
    let offline = c.cache_only();
    assert_eq!(offline.get::<Val>("pages/home", &[]).await.unwrap(), v1);
    assert_eq!(
        offline
            .get_v2::<Val>("home/feed/static", &[])
            .await
            .unwrap(),
        v2
    );
}

#[tokio::test]
async fn cache_only_miss_is_not_cached() {
    let server = MockServer::start().await;
    serve(&server, "/v1/pages/home", 0).await;
    let c = client(&server, 1, "US", Some(cache("miss"))).cache_only();
    let r = c.get::<Val>("pages/home", &[]).await;
    assert!(matches!(r, Err(Error::NotCached)), "{r:?}");
}

#[tokio::test]
async fn etag_sends_if_none_match_and_304_uses_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/playlists/p"))
        .and(header("if-none-match", "\"a\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/playlists/p"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"a\"")
                .set_body_json(serde_json::json!({"v": 1})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server, 1, "US", Some(cache("etag")));
    let first: Val = c.get("playlists/p", &[]).await.unwrap();
    let second: Val = c.get("playlists/p", &[]).await.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn key_includes_user_and_country() {
    let server = MockServer::start().await;
    serve(&server, "/v1/pages/home", 1).await;
    let shared = cache("key");
    let _: Val = client(&server, 1, "US", Some(shared.clone()))
        .get("pages/home", &[])
        .await
        .unwrap();
    for (user, country) in [(2, "US"), (1, "DE")] {
        let other = client(&server, user, country, Some(shared.clone())).cache_only();
        let r = other.get::<Val>("pages/home", &[]).await;
        assert!(matches!(r, Err(Error::NotCached)), "{user} {country}");
    }
}

#[tokio::test]
async fn no_cache_configured_behaves_as_before() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header_exists("if-none-match"))
        .respond_with(ResponseTemplate::new(304))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/playlists/p"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"a\"")
                .set_body_json(serde_json::json!({"v": 1})),
        )
        .expect(2)
        .mount(&server)
        .await;
    let c = client(&server, 1, "US", None);
    for _ in 0..2 {
        let _: Val = c.get("playlists/p", &[]).await.unwrap();
    }
    let r = c.cache_only().get::<Val>("playlists/p", &[]).await;
    assert!(matches!(r, Err(Error::NotCached)), "{r:?}");
}

/// Stream info carries short-lived signed segment URLs: it never goes to the cache (R24).
#[tokio::test]
async fn stream_info_is_never_cached() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tracks/1/playbackinfo"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(LOSSLESS, "application/json"))
        .mount(&server)
        .await;
    let c = client(&server, 1, "US", Some(cache("stream")));
    c.stream_info(1, Quality::Lossless).await.unwrap();
    assert_eq!(std::fs::read_dir(dir("stream")).unwrap().count(), 0);
}
