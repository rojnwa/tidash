//! Play-event reporting (SQS `SendMessageBatch` to TIDAL's event collector).
//! Wire format: docs/spikes/s1-auth-manifests.md `## Play events`.

use crate::{Client, Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngExt;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

// Mirrors SONE's production values (see the findings doc, `## Play events`); they drift, best effort.
const APP_VERSION: &str = "2.205.0";
const OS_NAME: &str = "Android";
const OS_VERSION: &str = "35";
const DEVICE_MODEL: &str = "Pixel 7";
const DEVICE_VENDOR: &str = "Google";

#[derive(Clone, Debug)]
pub struct PlaybackReport {
    pub track_id: u64,
    pub quality: String,
    pub streaming_session_id: String,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    pub start_position_s: f64,
    pub end_position_s: f64,
    /// `ALBUM`, `PLAYLIST`, `ARTIST`, `MIX`, `ITEM` or `MY_ITEMS`; plays without a source never reach Recently Played.
    pub source_type: String,
    pub source_id: String,
}

/// Random UUID v4 string.
pub fn new_session_id() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `(uid, cid, sid)` from our own access token; no signature check.
fn claims(token: &str) -> Result<(u64, u64, String)> {
    let missing = || Error::Auth("access token lacks play-log claims".into());
    let payload = token.split('.').nth(1).ok_or_else(missing)?;
    let json: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| missing())?)
            .map_err(|_| missing())?;
    let n = |k: &str| json.get(k).and_then(Value::as_u64);
    let sid = json.get("sid").and_then(Value::as_str);
    match (n("uid"), n("cid"), sid) {
        (Some(u), Some(c), Some(s)) => Ok((u, c, s.to_owned())),
        _ => Err(missing()),
    }
}

impl Client {
    /// Reports one finished play. Best effort: a stale token (401 or 403, S1) is refreshed and
    /// the report sent once more.
    // ponytail: no retry queue; persist failed reports if plays go missing
    pub async fn report_playback(&self, r: &PlaybackReport) -> Result<()> {
        let token = self.session().access_token;
        match self.send_report(r).await {
            Err(Error::Status { status: 403, .. }) => {
                self.refresh_after(&token).await?;
                self.send_report(r).await
            }
            res => res,
        }
    }

    async fn send_report(&self, r: &PlaybackReport) -> Result<()> {
        let resp = self.send(|token| self.report_request(r, token)).await?;
        let status = resp.status().as_u16();
        if resp.text().await?.contains("<BatchResultErrorEntry") {
            return Err(Error::Status {
                status,
                sub_status: None,
                message: "event rejected".into(),
            });
        }
        Ok(())
    }

    /// The SQS batch for `r`; its body carries `token` as well as the bearer header.
    fn report_request(&self, r: &PlaybackReport, token: &str) -> Result<reqwest::RequestBuilder> {
        let (uid, cid, sid) = claims(token)?;
        let cfg = self.config();
        let message = json!({
            "group": "play_log", "version": 2, "ts": r.ended_at_ms, "uuid": new_session_id(),
            "user": {"id": uid, "clientId": cid, "sessionId": sid},
            "client": {"token": cid.to_string(), "deviceType": "mobile", "version": APP_VERSION, "platform": "android"},
            "payload": {
                "playbackSessionId": r.streaming_session_id, "isPostPaywall": true,
                "productType": "TRACK",
                "requestedProductId": r.track_id.to_string(), "actualProductId": r.track_id.to_string(),
                "actualAssetPresentation": "FULL", "actualAudioMode": "STEREO",
                "actualQuality": r.quality,
                "startTimestamp": r.started_at_ms, "endTimestamp": r.ended_at_ms,
                "startAssetPosition": r.start_position_s, "endAssetPosition": r.end_position_s,
                "actions": [], "sourceType": r.source_type, "sourceId": r.source_id,
            },
        });
        // This JSON goes to TIDAL only; it carries the bare token, so never log it.
        let headers = json!({
            "client-id": cfg.client_id, "app-version": APP_VERSION, "os-name": OS_NAME,
            "os-version": OS_VERSION, "device-model": DEVICE_MODEL, "device-vendor": DEVICE_VENDOR,
            "consent-category": "NECESSARY", "requested-sent-timestamp": now_ms().to_string(),
            "authorization": token,
        });
        let k = "SendMessageBatchRequestEntry.1";
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("Action", "SendMessageBatch")
            .append_pair(&format!("{k}.Id"), &new_session_id())
            .append_pair(&format!("{k}.MessageBody"), &message.to_string())
            .append_pair(&format!("{k}.MessageAttribute.1.Name"), "Name")
            .append_pair(
                &format!("{k}.MessageAttribute.1.Value.StringValue"),
                "playback_session",
            )
            .append_pair(&format!("{k}.MessageAttribute.1.Value.DataType"), "String")
            .append_pair(&format!("{k}.MessageAttribute.2.Name"), "Headers")
            .append_pair(
                &format!("{k}.MessageAttribute.2.Value.StringValue"),
                &headers.to_string(),
            )
            .append_pair(&format!("{k}.MessageAttribute.2.Value.DataType"), "String")
            .finish();
        Ok(self
            .http()
            .post(cfg.events_url.clone())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body))
    }
}
