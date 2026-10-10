//! ListenBrainz (api.listenbrainz.org): the token check, and listen submissions.

use super::{Listen, Verdict};
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};

/// The most listens one `import` takes.
pub const MAX_LISTENS: usize = 1000;

/// A submission's `listen_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What plays now: no `listened_at`, and ListenBrainz keeps it only for a while.
    PlayingNow,
    Single,
    /// Up to `MAX_LISTENS`.
    Import,
}

/// The JSON body that submits `listens` as `kind`.
pub fn payload(kind: Kind, listens: &[&Listen]) -> Value {
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    let listen_type = match kind {
        Kind::PlayingNow => "playing_now",
        Kind::Single => "single",
        Kind::Import => "import",
    };
    let item = |l: &&Listen| {
        let mut item = json!({
            "track_metadata": {
                "artist_name": l.artist,
                "track_name": l.title,
                "release_name": l.album,
                "additional_info": {
                    "media_player": "Tidash",
                    "media_player_version": VERSION,
                    "submission_client": "Tidash",
                    "submission_client_version": VERSION,
                    "music_service": "tidal.com",
                    "origin_url": format!("https://tidal.com/browse/track/{}", l.track_id),
                    "duration_ms": u64::from(l.duration_s) * 1000,
                    // A string, as documented.
                    "tracknumber": l.track_number.to_string(),
                },
            },
        });
        if kind != Kind::PlayingNow {
            item["listened_at"] = l.started_at.into();
        }
        item
    };
    json!({
        "listen_type": listen_type,
        "payload": listens.iter().map(item).collect::<Vec<_>>(),
    })
}

/// `Token …`, marked sensitive so that no `Debug` output shows it; `None` if it cannot be a
/// header.
fn authorization(token: &str) -> Option<HeaderValue> {
    let mut value = HeaderValue::from_str(&format!("Token {token}")).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// What an answer means: 401, the token is no good; 429 and 5xx, try later; any other 4xx,
/// the data is refused.
fn verdict(status: u16) -> Verdict {
    match status {
        200..=299 => Verdict::Done,
        401 => Verdict::Reconnect,
        429 => Verdict::Retry,
        400..=499 => Verdict::Drop,
        _ => Verdict::Retry,
    }
}

/// Submits `listens` as `kind` with `token`. A network error is a `Retry`.
pub async fn submit(
    http: &reqwest::Client,
    base: &Url,
    token: &str,
    kind: Kind,
    listens: &[&Listen],
) -> Verdict {
    let Some(auth) = authorization(token) else {
        return Verdict::Reconnect;
    };
    let sent = http
        .post(format!("{base}1/submit-listens"))
        .header(AUTHORIZATION, auth)
        .json(&payload(kind, listens))
        .send()
        .await;
    match sent {
        Ok(r) => {
            let v = verdict(r.status().as_u16());
            if v != Verdict::Done {
                log::warn!("ListenBrainz answered {} to {kind:?}: {v:?}", r.status());
            }
            v
        }
        Err(e) => {
            log::debug!("ListenBrainz {kind:?}: {e}");
            Verdict::Retry
        }
    }
}

/// Whose `token` this is; `None` if ListenBrainz does not know it.
pub async fn validate(
    http: &reqwest::Client,
    base: &Url,
    token: &str,
) -> reqwest::Result<Option<String>> {
    #[derive(Deserialize)]
    struct Answer {
        valid: bool,
        #[serde(default)]
        user_name: String,
    }
    let Some(auth) = authorization(token) else {
        return Ok(None);
    };
    let answer: Answer = http
        .get(format!("{base}1/validate-token"))
        .header(AUTHORIZATION, auth)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(answer.valid.then_some(answer.user_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobble::tests::listen;

    #[test]
    fn listenbrainz_payload_shapes() {
        let (a, b) = (listen(7, 1_700_000_100), listen(8, 1_700_000_300));
        let version = env!("CARGO_PKG_VERSION");
        let now = payload(Kind::PlayingNow, &[&a]);
        let want = json!({
            "listen_type": "playing_now",
            "payload": [{
                "track_metadata": {
                    "artist_name": "Artist",
                    "track_name": "Track 7",
                    "release_name": "Album",
                    "additional_info": {
                        "media_player": "Tidash",
                        "media_player_version": version,
                        "submission_client": "Tidash",
                        "submission_client_version": version,
                        "music_service": "tidal.com",
                        "origin_url": "https://tidal.com/browse/track/7",
                        "duration_ms": 200_000,
                        "tracknumber": "3",
                    },
                },
            }],
        });
        assert_eq!(now, want, "no listened_at");
        let single = payload(Kind::Single, &[&a]);
        assert_eq!(single["listen_type"], "single");
        let mut stamped = want["payload"][0].clone();
        stamped["listened_at"] = 1_700_000_100.into();
        assert_eq!(single["payload"], json!([stamped]));
        let import = payload(Kind::Import, &[&a, &b]);
        assert_eq!(import["listen_type"], "import");
        let at: Vec<_> = (0..2)
            .map(|i| &import["payload"][i]["listened_at"])
            .collect();
        assert_eq!(at, [1_700_000_100, 1_700_000_300]);
        let track = &import["payload"][1]["track_metadata"]["track_name"];
        assert_eq!(track, "Track 8", "in order");
    }

    #[test]
    fn answers_map_to_verdicts() {
        let cases = [
            (200, Verdict::Done),
            (401, Verdict::Reconnect),
            (429, Verdict::Retry),
            (500, Verdict::Retry),
            (503, Verdict::Retry),
            (400, Verdict::Drop),
            (413, Verdict::Drop),
        ];
        for (status, want) in cases {
            assert_eq!(verdict(status), want, "{status}");
        }
    }
}
