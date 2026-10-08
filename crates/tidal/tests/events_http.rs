#![allow(clippy::unwrap_used)]

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use std::collections::HashMap;
use std::sync::Arc;
use tidal::{Client, ClientConfig, Error, MemoryStore, PlaybackReport, Session, new_session_id};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn jwt(claims: &str) -> String {
    format!(
        "{}.{}.x",
        B64.encode(r#"{"alg":"none"}"#),
        B64.encode(claims)
    )
}

fn setup(server: &MockServer, token: String) -> Client {
    let cfg = ClientConfig {
        events_url: format!("{}/api/event-batch", server.uri()).parse().unwrap(),
        auth_base: format!("{}/v1/", server.uri()).parse().unwrap(),
        ..ClientConfig::default()
    };
    let session = Session {
        access_token: token,
        refresh_token: "R".into(),
        expires_at: u64::MAX,
        user_id: 12345,
        country_code: "US".into(),
    };
    Client::new(cfg, session, Arc::new(MemoryStore::default()), None).unwrap()
}

fn report() -> PlaybackReport {
    PlaybackReport {
        track_id: 777,
        quality: "LOSSLESS".into(),
        streaming_session_id: "play-1".into(),
        started_at_ms: 1_000,
        ended_at_ms: 61_000,
        start_position_s: 0.0,
        end_position_s: 60.0,
        source_type: "ALBUM".into(),
        source_id: "555".into(),
    }
}

const CLAIMS: &str = r#"{"uid":12345,"cid":8017,"sid":"s-1"}"#;
const K: &str = "SendMessageBatchRequestEntry.1";

fn form(req: &Request) -> HashMap<String, String> {
    url::form_urlencoded::parse(&req.body)
        .into_owned()
        .collect()
}

/// The Headers message attribute, which carries the bare token.
fn headers(req: &Request) -> serde_json::Value {
    serde_json::from_str(&form(req)[&format!("{K}.MessageAttribute.2.Value.StringValue")]).unwrap()
}

async fn mount(server: &MockServer, body: &str) {
    Mock::given(method("POST"))
        .and(path("/api/event-batch"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/xml"))
        .mount(server)
        .await;
}

#[test]
fn session_id_is_uuid_v4() {
    let id = new_session_id();
    assert_eq!(id.len(), 36);
    assert_eq!(id.as_bytes()[14], b'4');
    assert!("89ab".contains(id.as_bytes()[19] as char));
    assert_ne!(id, new_session_id());
}

#[tokio::test]
async fn report_matches_documented_payload() {
    let server = MockServer::start().await;
    mount(&server, "<SendMessageBatchResponse/>").await;
    let token = jwt(CLAIMS);
    setup(&server, token.clone())
        .report_playback(&report())
        .await
        .unwrap();

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method.as_str(), "POST");
    assert_eq!(reqs[0].url.path(), "/api/event-batch");
    let form = form(&reqs[0]);
    assert_eq!(
        form[&format!("{K}.MessageAttribute.1.Value.StringValue")],
        "playback_session"
    );
    let body: serde_json::Value = serde_json::from_str(&form[&format!("{K}.MessageBody")]).unwrap();
    assert_eq!(body["group"], "play_log");
    assert_eq!(body["version"], 2);
    assert_eq!(body["ts"], 61_000);
    assert_eq!(body["user"]["id"], 12345);
    assert_eq!(body["user"]["clientId"], 8017);
    assert_eq!(body["user"]["sessionId"], "s-1");
    let p = &body["payload"];
    assert_eq!(p["actualProductId"], "777");
    assert_eq!(p["sourceType"], "ALBUM");
    assert_eq!(p["sourceId"], "555");
    assert_eq!(p["playbackSessionId"], "play-1");
    assert_eq!(p["startTimestamp"], 1_000);
    assert_eq!(p["endTimestamp"], 61_000);
    assert_eq!(p["endAssetPosition"], 60.0);
    let headers = headers(&reqs[0]);
    assert_eq!(headers.as_object().unwrap().len(), 9);
    assert_eq!(headers["authorization"], token);
}

/// S1: the collector answers 401 or 403 to a stale token: refresh, then retry once with the
/// new token in the body too.
#[tokio::test]
async fn stale_token_is_refreshed_and_the_retry_carries_the_new_one() {
    for status in [401, 403] {
        let server = MockServer::start().await;
        let (old, new) = (jwt(CLAIMS), jwt(r#"{"uid":12345,"cid":8017,"sid":"s-2"}"#));
        for (token, reply) in [(&old, status), (&new, 200)] {
            Mock::given(method("POST"))
                .and(path("/api/event-batch"))
                .and(header("authorization", format!("Bearer {token}")))
                .respond_with(ResponseTemplate::new(reply).set_body_raw("<ok/>", "text/xml"))
                .expect(1)
                .mount(&server)
                .await;
        }
        Mock::given(method("POST"))
            .and(path("/v1/oauth2/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"access_token": new, "expires_in": 14400})),
            )
            .expect(1)
            .mount(&server)
            .await;
        setup(&server, old)
            .report_playback(&report())
            .await
            .unwrap_or_else(|e| panic!("{status}: {e}"));
        let reqs = server.received_requests().await.unwrap();
        let retry = reqs.last().unwrap();
        assert_eq!(headers(retry)["authorization"], new, "after {status}");
        let body: serde_json::Value =
            serde_json::from_str(&form(retry)[&format!("{K}.MessageBody")]).unwrap();
        assert_eq!(body["user"]["sessionId"], "s-2", "after {status}");
    }
}

#[tokio::test]
async fn batch_error_entry_is_error() {
    let server = MockServer::start().await;
    mount(
        &server,
        "<R><BatchResultErrorEntry><Id>x</Id></BatchResultErrorEntry></R>",
    )
    .await;
    let err = setup(&server, jwt(CLAIMS))
        .report_playback(&report())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Status { status: 200, .. }));
}

#[tokio::test]
async fn token_without_claims_is_auth_error() {
    let server = MockServer::start().await;
    mount(&server, "<ok/>").await;
    let err = setup(&server, jwt(r#"{"uid":1}"#))
        .report_playback(&report())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Auth(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
}
