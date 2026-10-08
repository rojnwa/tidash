#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};
use tidal::{Client, ClientConfig, Error, MemoryStore, Session, TokenStore};
use wiremock::matchers::{body_string, header, method, path, query_param};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

const REFRESH: &str = include_str!("fixtures/refresh.json");

type Val = serde_json::Value;

fn session(access: &str) -> Session {
    Session {
        access_token: access.into(),
        refresh_token: "REFRESH".into(),
        expires_at: u64::MAX,
        user_id: 12345,
        country_code: "US".into(),
    }
}

fn setup(server: &MockServer, access: &str) -> (Client, Arc<MemoryStore>) {
    let base = format!("{}/v1/", server.uri());
    let cfg = ClientConfig {
        api_base: base.parse().unwrap(),
        api_v2_base: format!("{}/v2/", server.uri()).parse().unwrap(),
        auth_base: base.parse().unwrap(),
        ..ClientConfig::default()
    };
    let store = Arc::new(MemoryStore::default());
    let client = Client::new(cfg, session(access), store.clone(), None).unwrap();
    (client, store)
}

fn ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true}))
}

/// Matches `m p` carrying the standard params; v2 also needs `platform` and the client version.
fn standard(m: &str, p: &str, v2: bool) -> MockBuilder {
    let b = Mock::given(method(m))
        .and(path(p))
        .and(query_param("countryCode", "US"))
        .and(query_param("locale", "en_US"))
        .and(query_param("deviceType", "DESKTOP"));
    if v2 {
        b.and(query_param("platform", "DESKTOP"))
            .and(header("x-tidal-client-version", "2026.9.30"))
    } else {
        b
    }
}

async fn token_ok(server: &MockServer, times: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/oauth2/token"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(REFRESH, "application/json"))
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn get_adds_country_and_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/things"))
        .and(query_param("countryCode", "US"))
        .and(query_param("limit", "5"))
        .and(header("authorization", "Bearer ACCESS"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    let v: Val = c.get("things", &[("limit", "5".into())]).await.unwrap();
    assert_eq!(v["ok"], true);
}

#[tokio::test]
async fn expired_token_refreshes_once_and_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer OLD"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"status":401,"subStatus":11003})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer ACCESS"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    token_ok(&server, 1).await;
    let (c, store) = setup(&server, "OLD");
    let _: Val = c.get("things", &[]).await.unwrap();
    assert_eq!(store.load().unwrap().unwrap().access_token, "ACCESS");
    assert_eq!(c.session().access_token, "ACCESS");
}

#[tokio::test]
async fn a_detached_client_refreshes_in_memory_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer OLD"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer ACCESS"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    token_ok(&server, 1).await;
    let (c, store) = setup(&server, "OLD");
    c.detach_store().await;
    let _: Val = c.get("things", &[]).await.unwrap();
    assert_eq!(c.session().access_token, "ACCESS");
    assert!(store.load().unwrap().is_none(), "nothing saved");
}

#[tokio::test]
async fn concurrent_401s_share_one_refresh() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer OLD"))
        .respond_with(ResponseTemplate::new(401))
        .expect(5)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer ACCESS"))
        .respond_with(ok())
        .expect(5)
        .mount(&server)
        .await;
    token_ok(&server, 1).await;
    let (c, _) = setup(&server, "OLD");
    let g = || c.get::<Val>("things", &[]);
    let (a, b, d, e, f) = tokio::join!(g(), g(), g(), g(), g());
    for r in [a, b, d, e, f] {
        r.unwrap();
    }
}

#[tokio::test]
async fn playback_substatus_401_does_not_refresh() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(
                serde_json::json!({"status":401,"subStatus":4005,"userMessage":"nope"}),
            ),
        )
        .mount(&server)
        .await;
    token_ok(&server, 0).await;
    let (c, _) = setup(&server, "OLD");
    let r = c.get::<Val>("things", &[]).await;
    assert!(matches!(
        r,
        Err(Error::Status { sub_status: Some(4005), ref message, .. }) if message == "nope"
    ));
}

#[tokio::test]
async fn revoked_refresh_token_is_auth_error_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth2/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            serde_json::json!({"error":"invalid_grant","error_description":"revoked"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "OLD");
    let r = c.get::<Val>("things", &[]).await;
    assert!(matches!(r, Err(Error::Auth(_))), "{r:?}");
}

#[tokio::test]
async fn retry_after_is_honored() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ok())
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    let t = Instant::now();
    let _: Val = c.get("things", &[]).await.unwrap();
    assert!(t.elapsed() >= Duration::from_secs(1));
}

#[tokio::test]
async fn playback_substatus_after_refresh_is_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer OLD"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(serde_json::json!({"subStatus":11003})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer ACCESS"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(serde_json::json!({"subStatus":4005})),
        )
        .mount(&server)
        .await;
    token_ok(&server, 1).await;
    let (c, _) = setup(&server, "OLD");
    let r = c.get::<Val>("things", &[]).await;
    assert!(
        matches!(
            r,
            Err(Error::Status {
                sub_status: Some(4005),
                ..
            })
        ),
        "{r:?}"
    );
}

#[tokio::test]
async fn foreign_origin_path_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ok())
        .expect(0)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    for p in ["http://evil.test/x", "//evil.test/x"] {
        let r = c.get::<Val>(p, &[]).await;
        assert!(
            matches!(r, Err(Error::Status { status: 0, .. })),
            "{p}: {r:?}"
        );
        let r = c.get_v2::<Val>(p, &[]).await;
        assert!(matches!(r, Err(Error::Status { status: 0, .. })), "{p}");
        let r = c.delete(p).await;
        assert!(matches!(r, Err(Error::Status { status: 0, .. })), "{p}");
        let r = c.post_form(p, &[("a", "1".into())]).await;
        assert!(matches!(r, Err(Error::Status { status: 0, .. })), "{p}");
        let r = c.put_v2(p, &[], None).await;
        assert!(matches!(r, Err(Error::Status { status: 0, .. })), "{p}");
    }
}

#[tokio::test]
async fn v1_get_sends_standard_params() {
    let server = MockServer::start().await;
    standard("GET", "/v1/things", false)
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    let _: Val = c.get("things", &[]).await.unwrap();
}

#[tokio::test]
async fn v2_path_with_query_keeps_it() {
    let server = MockServer::start().await;
    standard("GET", "/v2/home/pages/X/view-all", true)
        .and(query_param("itemId", "a"))
        .and(query_param("limit", "50"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    let _: Val = c
        .get_v2("home/pages/X/view-all?itemId=a", &[("limit", "50".into())])
        .await
        .unwrap();
}

#[tokio::test]
async fn v2_get_sends_platform_and_client_version() {
    let server = MockServer::start().await;
    standard("GET", "/v2/home/feed/static", true)
        .and(header("authorization", "Bearer ACCESS"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    let _: Val = c.get_v2("home/feed/static", &[]).await.unwrap();
}

#[tokio::test]
async fn post_form_sends_urlencoded_body() {
    let server = MockServer::start().await;
    standard("POST", "/v1/users/12345/favorites/tracks", false)
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("trackIds=1%2C2&onArtifactNotFound=FAIL"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    c.post_form(
        "users/12345/favorites/tracks",
        &[
            ("trackIds", "1,2".into()),
            ("onArtifactNotFound", "FAIL".into()),
        ],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn delete_hits_path() {
    let server = MockServer::start().await;
    standard("DELETE", "/v1/users/12345/favorites/tracks/1,2", false)
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    c.delete("users/12345/favorites/tracks/1,2").await.unwrap();
}

#[tokio::test]
async fn put_v2_with_query_only() {
    let server = MockServer::start().await;
    standard("PUT", "/v2/favorites/mixes/remove", true)
        .and(query_param("mixIds", "a,b"))
        .and(header("content-type", "application/json"))
        .and(header("content-length", "0"))
        .and(body_string(""))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    c.put_v2("favorites/mixes/remove", &[("mixIds", "a,b".into())], None)
        .await
        .unwrap();
}

#[tokio::test]
async fn put_v2_with_form() {
    let server = MockServer::start().await;
    standard("PUT", "/v2/favorites/mixes/add", true)
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("mixIds=a%2Cb&onArtifactNotFound=FAIL"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    let (c, _) = setup(&server, "ACCESS");
    let form = [
        ("mixIds", "a,b".into()),
        ("onArtifactNotFound", "FAIL".into()),
    ];
    c.put_v2("favorites/mixes/add", &[], Some(&form))
        .await
        .unwrap();
}

#[tokio::test]
async fn write_refreshes_on_401() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/users/12345/favorites/albums"))
        .and(header("authorization", "Bearer OLD"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/users/12345/favorites/albums"))
        .and(header("authorization", "Bearer ACCESS"))
        .and(body_string("albumIds=7"))
        .respond_with(ok())
        .expect(1)
        .mount(&server)
        .await;
    token_ok(&server, 1).await;
    let (c, _) = setup(&server, "OLD");
    c.post_form("users/12345/favorites/albums", &[("albumIds", "7".into())])
        .await
        .unwrap();
}
