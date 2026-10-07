#![allow(clippy::unwrap_used)]

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use std::collections::HashMap;
use std::sync::Arc;
use tidal::{Client, ClientConfig, Error, MemoryStore, PlaybackReport, Session, new_session_id};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
        ..ClientConfig::default()
    };
    let session = Session {
        access_token: token,
        refresh_token: "R".into(),
        expires_at: u64::MAX,
        user_id: 12345,
        country_code: "US".into(),
    };
    Client::new(cfg, session, Arc::new(MemoryStore::default())).unwrap()
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
    let form: HashMap<String, String> = url::form_urlencoded::parse(&reqs[0].body)
        .into_owned()
        .collect();
    let k = "SendMessageBatchRequestEntry.1";
    assert_eq!(
        form[&format!("{k}.MessageAttribute.1.Value.StringValue")],
        "playback_session"
    );
    let body: serde_json::Value = serde_json::from_str(&form[&format!("{k}.MessageBody")]).unwrap();
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
    let headers: serde_json::Value =
        serde_json::from_str(&form[&format!("{k}.MessageAttribute.2.Value.StringValue")]).unwrap();
    assert_eq!(headers.as_object().unwrap().len(), 9);
    assert_eq!(headers["authorization"], token);
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
