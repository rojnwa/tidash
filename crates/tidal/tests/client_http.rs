#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};
use tidal::{Client, ClientConfig, Error, MemoryStore, Session, TokenStore};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
        auth_base: base.parse().unwrap(),
        ..ClientConfig::default()
    };
    let store = Arc::new(MemoryStore::default());
    let client = Client::new(cfg, session(access), store.clone()).unwrap();
    (client, store)
}

fn ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true}))
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
    }
}
