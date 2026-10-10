//! Last.fm (ws.audioscrobbler.com): signed calls, the desktop auth, "now playing" and
//! scrobbles.

use super::{Kind, Listen, Verdict};
use crate::i18n::tr;
use reqwest::Url;
use serde_json::Value;
use std::time::Duration;

/// The API account's key and shared secret, compiled in. Without both, Last.fm is hidden.
const KEY: Option<&str> = option_env!("TIDASH_LASTFM_API_KEY");
const SECRET: Option<&str> = option_env!("TIDASH_LASTFM_SECRET");

/// The most scrobbles one `track.scrobble` takes.
pub const MAX_SCROBBLES: usize = 50;
/// Where the user allows Tidash.
const AUTH_PAGE: &str = "https://www.last.fm/api/auth/";
/// A connect asks whether the user has allowed Tidash this often, for at most `WAIT`.
const POLL: Duration = Duration::from_secs(3);
const WAIT: Duration = Duration::from_secs(5 * 60);
/// `auth.getSession`'s "not allowed yet".
const NOT_YET: u32 = 14;

/// The key and the secret that sign every call. No `Debug`: it would show the secret.
#[derive(Clone, Copy)]
pub struct Keys {
    pub key: &'static str,
    pub secret: &'static str,
}

impl Keys {
    /// The keys compiled in; `None` unless both are, and not blank.
    pub fn compiled() -> Option<Keys> {
        let set = |v: Option<&'static str>| v.filter(|v| !v.trim().is_empty());
        Some(Keys {
            key: set(KEY)?,
            secret: set(SECRET)?,
        })
    }
}

/// A user's session: who they are, and the key that signs their scrobbles (a secret).
pub struct Session {
    pub name: String,
    pub key: String,
}

/// A call that came back without an answer to use: the HTTP status (0: none came, or not as
/// JSON), and Last.fm's error code if it gave one.
type Failure = (u16, Option<u32>);

/// The signature of `params`: sorted by name, each name and value but `format` and `callback`
/// run together, the secret after them; md5, in lower-case hex.
pub fn api_sig(params: &[(&str, &str)], secret: &str) -> String {
    let mut signed: Vec<_> = params
        .iter()
        .filter(|(k, _)| !matches!(*k, "format" | "callback"))
        .collect();
    signed.sort_by_key(|(k, _)| *k);
    let mut s: String = signed.iter().map(|(k, v)| format!("{k}{v}")).collect();
    s.push_str(secret);
    format!("{:x}", md5::compute(s))
}

/// What an answer means: 8, 11, 16, 29, a 5xx or none at all: try later; 9, 10, 26: the
/// session key or the API key is no good; any other error: refused for good.
pub fn verdict(http: u16, code: Option<u32>) -> Verdict {
    match (http, code) {
        (_, Some(8 | 11 | 16 | 29)) | (500..=599, _) => Verdict::Retry,
        (_, Some(9 | 10 | 26)) => Verdict::Reconnect,
        (_, Some(_)) => Verdict::Drop,
        (200..=299, None) => Verdict::Done,
        // No answer, or one without an error code (a proxy's, say).
        (_, None) => Verdict::Retry,
    }
}

/// A count or a code, which Last.fm's JSON gives as a number or a string.
fn number(v: &Value) -> Option<u32> {
    let n = v.as_u64().or_else(|| v.as_str()?.parse().ok())?;
    u32::try_from(n).ok()
}

/// Calls `method` with `params`, signed with `keys`, as a form POST: nothing secret is ever in
/// a URL, so none is in a logged error either.
async fn call(
    http: &reqwest::Client,
    base: &Url,
    keys: Keys,
    method: &str,
    mut params: Vec<(String, String)>,
) -> Result<Value, Failure> {
    params.push(("method".into(), method.into()));
    params.push(("api_key".into(), keys.key.into()));
    let pairs: Vec<_> = params
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let sig = api_sig(&pairs, keys.secret);
    params.push(("api_sig".into(), sig));
    params.push(("format".into(), "json".into()));
    let answer = match http.post(base.clone()).form(&params).send().await {
        Ok(answer) => answer,
        Err(e) => {
            log::debug!("Last.fm {method}: {e}");
            return Err((0, None));
        }
    };
    let status = answer.status();
    // Not Last.fm's JSON (a proxy's page, say): as if none came, so nothing is taken as sent.
    let Ok(body) = answer.json::<Value>().await else {
        log::debug!("Last.fm {method}: {status}, not JSON");
        return Err((0, None));
    };
    match number(&body["error"]) {
        None if status.is_success() => Ok(body),
        code => Err((status.as_u16(), code)),
    }
}

/// The listen fields: the first of `listens`' as they are, for "now playing"; else each one's,
/// indexed (`artist[0]`…), with when it started.
fn form(now: bool, listens: &[&Listen]) -> Vec<(String, String)> {
    let mut form = Vec::new();
    let n = if now { 1 } else { listens.len() };
    for (i, l) in listens.iter().take(n).enumerate() {
        let name = |k: &str| {
            if now {
                k.to_owned()
            } else {
                format!("{k}[{i}]")
            }
        };
        form.extend([
            (name("artist"), l.artist.clone()),
            (name("track"), l.title.clone()),
            (name("album"), l.album.clone()),
            (name("duration"), l.duration_s.to_string()),
            (name("trackNumber"), l.track_number.to_string()),
        ]);
        if !now {
            form.push((name("timestamp"), l.started_at.to_string()));
        }
    }
    form
}

/// Sends `listens` with session key `sk`: the first as "now playing" (`Kind::PlayingNow`),
/// else all of them, at most `MAX_SCROBBLES`, as scrobbles. Those Last.fm ignores (too old,
/// filtered, over the daily limit) are taken all the same: never sent again.
pub async fn submit(
    http: &reqwest::Client,
    base: &Url,
    keys: Keys,
    sk: &str,
    kind: Kind,
    listens: &[&Listen],
) -> Verdict {
    let now = kind == Kind::PlayingNow;
    let method = if now {
        "track.updateNowPlaying"
    } else {
        "track.scrobble"
    };
    let mut params = form(now, listens);
    params.push(("sk".into(), sk.into()));
    match call(http, base, keys, method, params).await {
        Ok(answer) => {
            let ignored = number(&answer["scrobbles"]["@attr"]["ignored"]).unwrap_or(0);
            if ignored > 0 {
                log::info!("Last.fm ignored {ignored} of {} scrobbles", listens.len());
            }
            Verdict::Done
        }
        Err((status, code)) => {
            let v = verdict(status, code);
            if status != 0 {
                log::warn!("Last.fm answered {status}, error {code:?}, to {method}: {v:?}");
            }
            v
        }
    }
}

/// The desktop auth: a request token, the page where the user allows Tidash (handed to
/// `open`), then `auth.getSession` every `POLL` until they have, for at most `WAIT`. The
/// session, or the error to show.
pub async fn authorize(
    http: &reqwest::Client,
    base: &Url,
    keys: Keys,
    open: impl FnOnce(String) + Send,
) -> Result<Session, String> {
    let token = match call(http, base, keys, "auth.getToken", Vec::new()).await {
        Ok(answer) => answer["token"].as_str().map(str::to_owned),
        Err((status, code)) => {
            log::warn!("Last.fm auth.getToken: {status}, error {code:?}");
            None
        }
    };
    let Some(token) = token else {
        return Err(tr("Last.fm could not be reached. Try again later.").into_owned());
    };
    let page = [("api_key", keys.key), ("token", token.as_str())];
    let page = Url::parse_with_params(AUTH_PAGE, page).expect("valid constant URL");
    open(page.to_string());
    let until = tokio::time::Instant::now() + WAIT;
    loop {
        tokio::time::sleep(POLL).await;
        let params = vec![("token".to_owned(), token.clone())];
        let (status, code) = match call(http, base, keys, "auth.getSession", params).await {
            Ok(answer) => match session(&answer) {
                Some(s) => return Ok(s),
                None => (200, None),
            },
            Err(failure) => failure,
        };
        // Not allowed yet, or no answer: asked again. Expired (15), invalid (4) and the rest
        // end it.
        if code != Some(NOT_YET) && verdict(status, code) != Verdict::Retry {
            log::warn!("Last.fm auth.getSession: {status}, error {code:?}");
            return Err(tr("Last.fm did not connect. Try again.").into_owned());
        }
        if tokio::time::Instant::now() >= until {
            let why = tr("Last.fm was not connected within 5 minutes. Try again.");
            return Err(why.into_owned());
        }
    }
}

/// The session in an `auth.getSession` answer.
fn session(answer: &Value) -> Option<Session> {
    let s = &answer["session"];
    Some(Session {
        name: s["name"].as_str()?.to_owned(),
        key: s["key"].as_str()?.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobble::tests::{listen, runtime};
    use crate::scrobble::{Apis, Entry, Outcome, Scrobbler, Service};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tidal::{MemoryStore, TokenStore};
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEYS: Keys = Keys {
        key: "KEY",
        secret: "SECRET",
    };

    #[test]
    fn api_sig_vector() {
        let params = [
            ("api_key", "KEY"),
            ("method", "auth.getSession"),
            ("token", "TOKEN"),
            ("format", "json"),
        ];
        let sig = api_sig(&params, "SECRET");
        assert_eq!(sig, "5479efdba54a79385144dca24a423b8a");
    }

    #[test]
    fn error_codes_map_to_retry_reconnect_or_drop() {
        use Verdict::{Done, Drop, Reconnect, Retry};
        let cases = [
            (200, None, Done),
            (0, None, Retry), // no answer
            (503, None, Retry),
            (500, Some(16), Retry),
            (400, Some(11), Retry),
            (400, Some(16), Retry),
            (429, Some(29), Retry),
            (400, Some(8), Retry),
            (403, Some(9), Reconnect),
            (403, Some(10), Reconnect),
            (403, Some(26), Reconnect),
            (400, Some(6), Drop),
            (400, Some(13), Drop),
            (200, Some(2), Drop),
        ];
        for (http, code, want) in cases {
            assert_eq!(verdict(http, code), want, "{http} {code:?}");
        }
    }

    /// A Last.fm call of `method`.
    fn call_of(m: &str) -> wiremock::MockBuilder {
        let named = body_string_contains(format!("method={m}"));
        Mock::given(method("POST")).and(path("/2.0/")).and(named)
    }

    fn json(status: u16, body: &str) -> ResponseTemplate {
        ResponseTemplate::new(status).set_body_raw(body.to_owned(), "application/json")
    }

    /// The form each request to `server` carried.
    async fn forms(server: &MockServer) -> Vec<HashMap<String, String>> {
        let requests = server.received_requests().await.expect("recorded");
        let form = |body: &[u8]| {
            let mut url = Url::parse("http://form.test/").expect("a URL");
            url.set_query(Some(&String::from_utf8_lossy(body)));
            url.query_pairs().into_owned().collect()
        };
        requests.iter().map(|r| form(&r.body)).collect()
    }

    fn base(server: &MockServer) -> Url {
        format!("{}/2.0/", server.uri()).parse().expect("a URL")
    }

    /// Connected to Last.fm at `server` with session key "SK"; the queue only in memory.
    fn scrobbler(server: &MockServer) -> (Scrobbler, Arc<MemoryStore>) {
        let store = Arc::new(MemoryStore::default());
        store.set_secret("lastfm", "SK").expect("stored");
        let apis = Apis {
            lastfm: base(server),
            lastfm_keys: Some(KEYS),
            ..Apis::default()
        };
        let http = reqwest::Client::new();
        let s = Scrobbler::new(http, store.clone(), None, apis, &[Service::LastFm]);
        (s, store)
    }

    /// `n` plays queued for Last.fm, as an earlier run left them, a second apart.
    async fn queue(s: &Scrobbler, n: u64) {
        let entries = (0..n).map(|i| Entry {
            service: Service::LastFm,
            listen: listen(i, 1_700_000_000 + i),
        });
        s.0.queue.lock().await.extend(entries);
    }

    async fn queued(s: &Scrobbler) -> usize {
        s.0.queue.lock().await.len()
    }

    #[test]
    fn scrobble_batches_of_50() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let taken = r#"{"scrobbles":{"@attr":{"accepted":50,"ignored":0}}}"#;
            let answer = json(200, taken);
            call_of("track.scrobble")
                .respond_with(answer)
                .mount(&server)
                .await;
            let (s, _) = scrobbler(&server);
            queue(&s, 120).await;
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(queued(&s).await, 0);
            let forms = forms(&server).await;
            let count = |f: &HashMap<String, String>| {
                f.keys().filter(|k| k.starts_with("timestamp[")).count()
            };
            assert_eq!(forms.iter().map(count).collect::<Vec<_>>(), [50, 50, 20]);
            let first: Vec<_> = forms.iter().map(|f| f["timestamp[0]"].as_str()).collect();
            assert_eq!(
                first,
                ["1700000000", "1700000050", "1700000100"],
                "in order"
            );
            let last = &forms[2];
            let fields = ["artist", "track", "album", "duration", "trackNumber"]
                .map(|k| last[&format!("{k}[19]")].as_str());
            assert_eq!(fields, ["Artist", "Track 119", "Album", "200", "3"]);
            for f in &forms {
                let sent = (
                    f["sk"].as_str(),
                    f["api_key"].as_str(),
                    f["format"].as_str(),
                );
                assert_eq!(sent, ("SK", "KEY", "json"));
                let signed = f.iter().filter(|(k, _)| *k != "api_sig");
                let signed: Vec<_> = signed.map(|(k, v)| (k.as_str(), v.as_str())).collect();
                assert_eq!(f["api_sig"], api_sig(&signed, "SECRET"), "signed");
            }
        });
    }

    #[test]
    fn ignored_items_are_dropped() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let answer = r##"{"scrobbles":{"scrobble":[
                {"timestamp":"1700000000","ignoredMessage":{"code":"0","#text":""}},
                {"timestamp":"1700000001","ignoredMessage":{"code":"3","#text":"Too old"}},
                {"timestamp":"1700000002","ignoredMessage":{"code":"0","#text":""}}],
                "@attr":{"accepted":2,"ignored":1}}}"##;
            let answer = json(200, answer);
            call_of("track.scrobble")
                .respond_with(answer)
                .mount(&server)
                .await;
            let (s, _) = scrobbler(&server);
            queue(&s, 3).await;
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(queued(&s).await, 0, "the ignored one too");
            assert_eq!(s.flush().await, Outcome::default());
            assert_eq!(forms(&server).await.len(), 1, "not sent again");
        });
    }

    #[test]
    fn now_playing_is_never_queued() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let down = ResponseTemplate::new(503);
            call_of("track.updateNowPlaying")
                .respond_with(down)
                .mount(&server)
                .await;
            let (s, _) = scrobbler(&server);
            let out = s.now_playing(listen(7, 1_700_000_000)).await;
            assert_eq!(out, Outcome::default(), "no retry");
            assert_eq!(queued(&s).await, 0);
            assert_eq!(s.flush().await, Outcome::default());
            let forms = forms(&server).await;
            assert_eq!(forms.len(), 1, "not sent again");
            let f = &forms[0];
            let fields = [
                "method",
                "artist",
                "track",
                "album",
                "duration",
                "trackNumber",
            ]
            .map(|k| f.get(k).map(String::as_str));
            let want = [
                "track.updateNowPlaying",
                "Artist",
                "Track 7",
                "Album",
                "200",
                "3",
            ];
            assert_eq!(fields, want.map(Some));
            assert!(!f.contains_key("timestamp"), "{f:?}");
        });
    }

    /// A 200 that is not Last.fm's JSON (a proxy's page) takes nothing: the plays wait.
    #[test]
    fn an_answer_not_from_lastfm_keeps_the_queue() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let page = ResponseTemplate::new(200).set_body_raw("<html>", "text/html");
            call_of("track.scrobble")
                .respond_with(page)
                .mount(&server)
                .await;
            let (s, _) = scrobbler(&server);
            queue(&s, 2).await;
            assert!(s.flush().await.retry);
            assert_eq!(queued(&s).await, 2);
        });
    }

    /// Like ListenBrainz's 401: the plays wait, the session key is forgotten, asked once.
    #[test]
    fn invalid_session_keeps_the_queue() {
        runtime().block_on(async {
            let server = MockServer::start().await;
            let invalid = json(403, r#"{"error":9,"message":"Invalid session key"}"#);
            call_of("track.scrobble")
                .respond_with(invalid)
                .mount(&server)
                .await;
            let (s, store) = scrobbler(&server);
            queue(&s, 2).await;
            let want = Outcome {
                retry: false,
                refused: vec![Service::LastFm],
            };
            assert_eq!(s.flush().await, want);
            assert_eq!(queued(&s).await, 2, "the queue stays");
            assert_eq!(store.secret("lastfm").expect("read"), None, "forgotten");
            assert_eq!(s.flush().await, Outcome::default(), "asked once");
        });
    }

    /// 15 (the token expired) ends the connect at once; 14 (not allowed yet) ends it after 5
    /// minutes.
    #[test]
    fn connect_gives_up_on_refusal_or_after_5_minutes() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .expect("a runtime");
        rt.block_on(async {
            // With time paused, a pending pool timer would move the clock while a request waits.
            let http = reqwest::Client::builder()
                .pool_idle_timeout(None)
                .build()
                .expect("a client");
            let cases = [
                (15, "Last.fm did not connect. Try again.", 1),
                (
                    14,
                    "Last.fm was not connected within 5 minutes. Try again.",
                    100,
                ),
            ];
            for (code, why, asks) in cases {
                let server = MockServer::start().await;
                let token = json(200, r#"{"token":"TOKEN"}"#);
                call_of("auth.getToken")
                    .respond_with(token)
                    .mount(&server)
                    .await;
                let refused = json(403, &format!(r#"{{"error":{code},"message":"No"}}"#));
                call_of("auth.getSession")
                    .respond_with(refused)
                    .mount(&server)
                    .await;
                let start = tokio::time::Instant::now();
                let got = authorize(&http, &base(&server), KEYS, |_| {}).await;
                assert_eq!(got.err().as_deref(), Some(why));
                assert_eq!(forms(&server).await.len(), 1 + asks, "{code}");
                let waited = start.elapsed();
                assert_eq!(waited, Duration::from_secs(3) * asks as u32, "{code}");
            }
        });
    }
}
