#![allow(clippy::unwrap_used)]

use base64::{Engine, engine::general_purpose::STANDARD};
use std::sync::Arc;
use tidal::{
    Client, ClientConfig, Error, Failure, MemoryStore, Quality, Session, classify, dash_segments,
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOSSLESS: &str = include_str!("fixtures/playbackinfo_lossless.json");
const HIRES: &str = include_str!("fixtures/playbackinfo_hires.json");
const ERR4005: &str = include_str!("fixtures/playbackinfo_error_4005.json");

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
    Client::new(cfg, session, Arc::new(MemoryStore::default())).unwrap()
}

async fn serve(server: &MockServer, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path("/v1/tracks/1/playbackinfo"))
        .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
        .mount(server)
        .await;
}

fn mpd(inner: &str) -> String {
    let xml = format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011"><Period><AdaptationSet>{inner}</AdaptationSet></Period></MPD>"#
    );
    STANDARD.encode(xml)
}

#[tokio::test]
async fn lossless_fixture_yields_segment_urls() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/tracks/1/playbackinfo"))
        .and(query_param("audioquality", "LOSSLESS"))
        .and(query_param("playbackmode", "STREAM"))
        .and(query_param("assetpresentation", "FULL"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(LOSSLESS, "application/json"))
        .mount(&server)
        .await;
    let s = setup(&server)
        .stream_info(1, Quality::Lossless)
        .await
        .unwrap();
    assert_eq!(s.codec, "flac");
    assert_eq!((s.bit_depth, s.sample_rate), (Some(16), Some(44100)));
    assert_eq!(s.track_id, 328638758);
    assert_eq!(s.quality, "LOSSLESS");
    assert_eq!(s.track_gain, Some(-5.56));
    assert_eq!(s.album_peak, Some(0.988439));
    assert_eq!(s.urls.len(), 1 + 69);
    assert_eq!(s.urls[0], "https://example.invalid/stream.flac");
    assert_eq!(s.urls[1], "https://example.invalid/stream-1.flac");
    assert_eq!(s.urls[69], "https://example.invalid/stream-69.flac");
}

#[tokio::test]
async fn hires_fixture_is_24_88200() {
    let server = MockServer::start().await;
    serve(&server, 200, HIRES).await;
    let s = setup(&server)
        .stream_info(1, Quality::HiResLossless)
        .await
        .unwrap();
    assert_eq!((s.bit_depth, s.sample_rate), (Some(24), Some(88200)));
}

#[test]
fn timeline_repeat_counts() {
    let b64 = mpd(
        r#"<Representation codecs="flac"><SegmentTemplate initialization="i" media="m-$Number$" startNumber="5"><SegmentTimeline><S d="10" r="67"/><S d="3"/></SegmentTimeline></SegmentTemplate></Representation>"#,
    );
    let (codec, urls) = dash_segments(&b64).unwrap();
    assert_eq!(codec, "flac");
    assert_eq!(urls.len(), 70);
    assert_eq!((urls[1].as_str(), urls[69].as_str()), ("m-5", "m-73"));
}

#[test]
fn template_on_adaptation_set_and_decoded_urls() {
    let b64 = mpd(
        r#"<SegmentTemplate initialization="i?a=1&amp;b=2" media="m-$Number$?a=1&amp;b=2"><SegmentTimeline><S d="1"/></SegmentTimeline></SegmentTemplate><Representation codecs="flac"/>"#,
    );
    let (_, urls) = dash_segments(&b64).unwrap();
    assert_eq!(urls, ["i?a=1&b=2", "m-1?a=1&b=2"]);
}

#[test]
fn unsupported_template_identifier_is_rejected() {
    let b64 = mpd(
        r#"<Representation codecs="flac"><SegmentTemplate initialization="i" media="m-$RepresentationID$-$Number$"><SegmentTimeline><S d="1"/></SegmentTimeline></SegmentTemplate></Representation>"#,
    );
    assert!(matches!(dash_segments(&b64), Err(Error::Manifest(_))));
}

#[test]
fn content_protection_is_rejected() {
    let b64 = mpd(
        r#"<ContentProtection schemeIdUri="x"/><Representation codecs="flac"><SegmentTemplate initialization="i" media="m-$Number$"><SegmentTimeline><S d="1"/></SegmentTimeline></SegmentTemplate></Representation>"#,
    );
    assert!(matches!(dash_segments(&b64), Err(Error::Manifest(m)) if m.contains("encrypted")));
}

#[tokio::test]
async fn non_dash_manifest_is_rejected() {
    let server = MockServer::start().await;
    let body = LOSSLESS.replace("application/dash+xml", "application/vnd.tidal.bts");
    serve(&server, 200, &body).await;
    let r = setup(&server).stream_info(1, Quality::Lossless).await;
    assert!(matches!(r, Err(Error::Manifest(_))));
}

#[test]
fn classify_terminal_and_retryable() {
    let st = |s| Error::Status {
        status: 401,
        sub_status: Some(s),
        message: String::new(),
    };
    for s in [4005, 4010, 4030, 4031, 4032, 4034, 4035] {
        assert_eq!(classify(&st(s)), Failure::Terminal, "{s}");
    }
    for s in [4006, 4033] {
        assert_eq!(classify(&st(s)), Failure::Retryable, "{s}");
    }
    let e500 = Error::Status {
        status: 500,
        sub_status: None,
        message: String::new(),
    };
    assert_eq!(classify(&e500), Failure::Retryable);
    assert_eq!(classify(&Error::Manifest("x".into())), Failure::Terminal);
    assert_eq!(classify(&Error::Auth("x".into())), Failure::Retryable);
}

#[tokio::test]
async fn error_fixture_is_terminal() {
    let server = MockServer::start().await;
    serve(&server, 401, ERR4005).await;
    let err = setup(&server)
        .stream_info(1, Quality::Lossless)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Status {
            sub_status: Some(4005),
            ..
        }
    ));
    assert_eq!(classify(&err), Failure::Terminal);
}
