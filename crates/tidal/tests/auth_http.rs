#![allow(clippy::unwrap_used)]

use std::time::{SystemTime, UNIX_EPOCH};
use tidal::{ClientConfig, Error, Pkce, Session, exchange_code, refresh};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = include_str!("fixtures/token.json");
const REFRESH: &str = include_str!("fixtures/refresh.json");

fn cfg(server: &MockServer) -> ClientConfig {
    ClientConfig {
        auth_base: format!("{}/v1/", server.uri()).parse().unwrap(),
        ..ClientConfig::default()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

#[tokio::test]
async fn exchange_posts_form_and_parses_session() {
    let server = MockServer::start().await;
    let cfg = cfg(&server);
    let pkce = Pkce::from_parts("verif-123", "st");
    Mock::given(method("POST"))
        .and(path("/v1/oauth2/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=abc"))
        .and(body_string_contains("code_verifier=verif-123"))
        .and(body_string_contains(format!("client_id={}", cfg.client_id)))
        .and(body_string_contains("scope=r_usr%2Bw_usr%2Bw_sub"))
        .and(body_string_contains("client_unique_key=0000000000000000"))
        .respond_with(json(TOKEN))
        .expect(1)
        .mount(&server)
        .await;
    let s = exchange_code(&reqwest::Client::new(), &cfg, "abc", &pkce)
        .await
        .unwrap();
    assert_eq!(s.access_token, "ACCESS");
    assert_eq!(s.refresh_token, "REFRESH");
    assert_eq!(s.user_id, 12345);
    assert_eq!(s.country_code, "US");
    assert!(s.expires_at.abs_diff(now() + 14400) <= 5);
}

#[tokio::test]
async fn refresh_keeps_old_refresh_token_and_user_when_absent() {
    let server = MockServer::start().await;
    let cfg = cfg(&server);
    Mock::given(method("POST"))
        .and(path("/v1/oauth2/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=OLDREF"))
        .respond_with(json(REFRESH))
        .expect(1)
        .mount(&server)
        .await;
    let old = Session {
        access_token: "OLDACC".into(),
        refresh_token: "OLDREF".into(),
        expires_at: 0,
        user_id: 777,
        country_code: "DE".into(),
    };
    let s = refresh(&reqwest::Client::new(), &cfg, &old).await.unwrap();
    // S1: refresh sends client_id and no scope.
    let reqs = server.received_requests().await.unwrap();
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("client_id="));
    assert!(!body.contains("scope"));
    assert_eq!(s.access_token, "ACCESS");
    assert_eq!(s.refresh_token, "OLDREF");
    // The refresh fixture carries user data, so it wins; absent-user fallback is covered below.
    assert_eq!(s.user_id, 12345);
    assert_eq!(s.country_code, "US");
    assert!(s.expires_at > now());
}

#[tokio::test]
async fn refresh_without_user_keeps_old_user() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth2/token"))
        .respond_with(json(r#"{"access_token":"NEW","expires_in":60}"#))
        .mount(&server)
        .await;
    let old = Session {
        access_token: "A".into(),
        refresh_token: "R".into(),
        expires_at: 0,
        user_id: 777,
        country_code: "DE".into(),
    };
    let s = refresh(&reqwest::Client::new(), &cfg(&server), &old)
        .await
        .unwrap();
    assert_eq!(
        (s.refresh_token.as_str(), s.user_id, s.country_code.as_str()),
        ("R", 777, "DE")
    );
}

#[tokio::test]
async fn invalid_grant_is_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth2/token"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_raw(r#"{"error":"invalid_grant"}"#, "application/json"),
        )
        .mount(&server)
        .await;
    let r = exchange_code(&reqwest::Client::new(), &cfg(&server), "x", &Pkce::new()).await;
    assert!(matches!(r, Err(Error::Auth(m)) if m.contains("invalid_grant")));
}
