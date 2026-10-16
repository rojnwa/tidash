//! Self-update from GitHub releases (spec §8): fastframe-update over our reqwest 0.13, on a
//! thread of its own, as every updater call blocks.

use crate::backend::Emitter;
use crate::i18n::tr;
use crate::messages::{UpdateState, UpdateStep};
use crate::model::Event;
use fastframe_update::{
    CHECK_INTERVAL, MacConfig, Prepared, Prereleases, Request, Response, Transport, UpdateConfig,
    Updater,
};
use reqwest::header::{ACCEPT, LOCATION, USER_AGENT};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub const CONFIG: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        bundle_ids: &["io.github.rojnwa.Tidash"],
        executable_names: &[], // the slug, `tidash`
        legacy_bundle_names: &[],
    },
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    // A release candidate is offered the next one and the release; a release never a candidate.
    prereleases: Prereleases::WhenRunningPrerelease,
    ..UpdateConfig::new(
        "rojnwa/tidash",
        "Tidash",
        "tidash",
        env!("CARGO_PKG_VERSION"),
    )
};

/// What the backend asks of the update thread.
#[derive(Debug, PartialEq)]
pub enum Command {
    /// The user asked: check now, then download what is new.
    Check,
    /// The Automatic updates setting changed.
    Automatic(bool),
    /// Hand the downloaded update to the helper, then quit.
    Install,
}

/// The update thread; it ends when the returned sender is dropped (the backend stops).
pub fn start(emit: Arc<Emitter>, automatic: bool) -> Sender<Command> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new().name("updates".into());
    let spawned = thread.spawn(move || {
        let send = |ev| emit.send(ev);
        match Http::new() {
            Ok(http) => {
                let updater = Updater::new(CONFIG, http);
                run(&updater, &rx, automatic, CHECK_INTERVAL, &send);
            }
            Err(e) => log::warn!("no update checks: {e}"),
        }
    });
    if let Err(e) = spawned {
        log::error!("update thread failed: {e}");
    }
    tx
}

/// Checks when due (`next_wait`) and on `Command::Check`, until `rx` disconnects; the updater,
/// and the blocking client in it, drop on this thread.
fn run(
    updater: &Updater,
    rx: &Receiver<Command>,
    mut automatic: bool,
    every: Duration,
    emit: &dyn Fn(Event),
) {
    let mut next = Instant::now();
    let mut prepared: Option<Prepared> = None;
    loop {
        let wait = next_wait(automatic, prepared.is_some(), next, Instant::now());
        // `Duration::MAX` waits for good: std falls back to `recv` when the deadline overflows.
        match rx.recv_timeout(wait) {
            Err(RecvTimeoutError::Timeout) => {
                next = Instant::now() + every;
                prepared = check(updater, false, emit);
            }
            Ok(Command::Check) => {
                if prepared.is_none() {
                    next = Instant::now() + every;
                    prepared = check(updater, true, emit);
                }
            }
            // Turned on after `next` has passed, the next wait is zero: it checks at once.
            Ok(Command::Automatic(on)) => automatic = on,
            Ok(Command::Install) => {
                if let Some(ready) = prepared.take() {
                    install(updater, ready, emit);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// How long `run` sleeps: until `next` while automatic, for good with an update `ready` (so
/// the timer neither checks nor downloads again) or with automatic updates off.
fn next_wait(automatic: bool, ready: bool, next: Instant, now: Instant) -> Duration {
    if automatic && !ready {
        next.saturating_duration_since(now)
    } else {
        Duration::MAX
    }
}

/// Checks, then downloads a newer release this copy can install. Only a check the user
/// `asked` for says it is checking, up to date, or failed.
fn check(updater: &Updater, asked: bool, emit: &dyn Fn(Event)) -> Option<Prepared> {
    let say = |state| emit(Event::Update(state));
    if asked {
        say(UpdateState::Checking);
    }
    let release = match updater.check() {
        Ok(Some(release)) => release,
        Ok(None) => {
            if asked {
                say(UpdateState::UpToDate);
            }
            return None;
        }
        Err(e) => {
            log::info!("update check failed: {e:#}");
            if asked {
                say(UpdateState::Failed(UpdateStep::Check));
            }
            return None;
        }
    };
    let version = release.version.clone();
    if let Err(reason) = updater.installation() {
        say(UpdateState::Unsupported { version, reason });
        return None;
    }
    let mut shown = None;
    let progress = |received: u64, total: u64| {
        let percent = (received * 100)
            .checked_div(total)
            .map_or(0, |p| p.min(100) as u8);
        if shown != Some(percent) {
            shown = Some(percent);
            let version = version.clone();
            say(UpdateState::Downloading { version, percent });
        }
    };
    match updater.download(&release, progress) {
        Ok(prepared) => {
            say(UpdateState::Ready {
                version: version.clone(),
            });
            let notice =
                tr("Tidash {version} is ready. Restart to update it in Settings › Updates.");
            emit(Event::Error(notice.replace("{version}", &version)));
            Some(prepared)
        }
        Err(e) => {
            log::warn!("update download failed: {e:#}");
            say(UpdateState::Failed(UpdateStep::Download));
            None
        }
    }
}

/// The helper takes over and the app quits through its normal shutdown; the helper waits up
/// to a minute for it to exit. The relaunch takes no arguments.
fn install(updater: &Updater, prepared: Prepared, emit: &dyn Fn(Event)) {
    match updater.handoff(prepared, Vec::new()) {
        Ok(()) => emit(Event::Quit),
        Err(e) => {
            log::warn!("update handoff failed: {e:#}");
            emit(Event::Update(UpdateState::Failed(UpdateStep::Install)));
        }
    }
}

/// fastframe-update's transport over our reqwest 0.13 (its `ReqwestTransport` is 0.12): every
/// status comes back as is, and redirects are the updater's, which follows them only to
/// GitHub's release hosts.
struct Http(reqwest::blocking::Client);

impl Http {
    fn new() -> reqwest::Result<Self> {
        reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(15 * 60))
            .build()
            .map(Self)
    }
}

impl Transport for Http {
    fn get(&self, request: &Request<'_>) -> anyhow::Result<Response> {
        let response = self
            .0
            .get(request.url)
            .header(ACCEPT, request.accept)
            .header(USER_AGENT, request.user_agent)
            .send()?;
        let location = response.headers().get(LOCATION);
        let location = location.and_then(|v| v.to_str().ok()).map(str::to_owned);
        let status = response.status().as_u16();
        Ok(Response {
            status,
            location,
            body: Box::new(response),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::channel;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Answers every request with the release `tag` (`None`: a transport failure), counting calls.
    struct Feed {
        tag: Option<&'static str>,
        calls: Arc<AtomicUsize>,
    }

    impl Transport for Feed {
        fn get(&self, _: &Request<'_>) -> anyhow::Result<Response> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let tag = self.tag.ok_or_else(|| anyhow::anyhow!("offline"))?;
            let body = format!(
                r#"{{"tag_name":"{tag}","html_url":"https://github.com/rojnwa/tidash/releases/tag/{tag}"}}"#
            );
            Ok(Response::ok(std::io::Cursor::new(body.into_bytes())))
        }
    }

    fn feed(tag: Option<&'static str>) -> (Feed, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let feed = Feed {
            tag,
            calls: Arc::clone(&calls),
        };
        (feed, calls)
    }

    /// `run` on a thread of its own, for version 1.0.0; what it emits comes out of the receiver.
    fn spawn(feed: Feed, automatic: bool, every: Duration) -> (Sender<Command>, Receiver<Event>) {
        let (tx, rx) = channel();
        let (events, emitted) = channel();
        let config = UpdateConfig::new("rojnwa/tidash", "Tidash", "tidash", "1.0.0");
        std::thread::spawn(move || {
            let emit = |ev| {
                let _ = events.send(ev);
            };
            run(&Updater::new(config, feed), &rx, automatic, every, &emit);
        });
        (tx, emitted)
    }

    /// The next `n` events, each an update state within 2 s; then none for 100 ms.
    fn states(events: &Receiver<Event>, n: usize) -> Vec<UpdateState> {
        let mut got = Vec::new();
        for _ in 0..n {
            match events.recv_timeout(Duration::from_secs(2)) {
                Ok(Event::Update(state)) => got.push(state),
                other => panic!("{other:?} after {got:?}"),
            }
        }
        quiet(events, Duration::from_millis(100));
        got
    }

    fn quiet(events: &Receiver<Event>, wait: Duration) {
        let ev = events.recv_timeout(wait);
        assert!(ev.is_err(), "{ev:?}");
    }

    /// `calls` once `done` holds, or after `within`.
    fn calls_once(calls: &AtomicUsize, within: Duration, done: impl Fn(usize) -> bool) -> usize {
        let start = Instant::now();
        loop {
            let n = calls.load(Ordering::SeqCst);
            if done(n) || start.elapsed() > within {
                return n;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn config_is_valid() {
        CONFIG.validate().expect("a valid config");
        assert!(CONFIG.publisher_key.is_some());
        assert_eq!(CONFIG.slug, "tidash");
    }

    #[test]
    fn a_check_reports_up_to_date() {
        let (feed, _) = feed(Some("v1.0.0"));
        let (tx, events) = spawn(feed, false, CHECK_INTERVAL);
        tx.send(Command::Check).expect("sent");
        let want = [UpdateState::Checking, UpdateState::UpToDate];
        assert_eq!(states(&events, 2), want);
    }

    /// The test executable has no marker beside it and is in no bundle: nothing downloads.
    #[test]
    fn a_release_this_copy_cannot_install_is_reported() {
        let (feed, calls) = feed(Some("v9.0.0"));
        let (tx, events) = spawn(feed, false, CHECK_INTERVAL);
        tx.send(Command::Check).expect("sent");
        let got = states(&events, 2);
        assert!(
            matches!(
                &got[..],
                [UpdateState::Checking, UpdateState::Unsupported { version, .. }] if version == "9.0.0"
            ),
            "{got:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "only the check");
    }

    #[test]
    fn a_failed_check_is_silent_unless_asked() {
        let (feed, calls) = feed(None);
        let (tx, events) = spawn(feed, true, Duration::from_secs(60 * 60));
        let n = calls_once(&calls, Duration::from_secs(2), |n| n == 1);
        assert_eq!(n, 1, "the check at start");
        quiet(&events, Duration::from_millis(50));
        tx.send(Command::Check).expect("sent");
        let want = [
            UpdateState::Checking,
            UpdateState::Failed(UpdateStep::Check),
        ];
        assert_eq!(states(&events, 2), want);
    }

    #[test]
    fn automatic_checks_follow_the_setting() {
        let (feed, calls) = feed(Some("v1.0.0"));
        let (tx, events) = spawn(feed, false, Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "off");
        tx.send(Command::Automatic(true)).expect("sent");
        let n = calls_once(&calls, Duration::from_secs(1), |n| n >= 2);
        assert!(n >= 2, "on: {n}");
        tx.send(Command::Automatic(false)).expect("sent");
        std::thread::sleep(Duration::from_millis(200));
        let n = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::SeqCst), n, "off again");
        let ev = events.try_recv();
        assert!(ev.is_err(), "unasked and up to date: {ev:?}");
    }

    /// A check the user asked for also restarts the timer: the due automatic check does not
    /// follow it.
    #[test]
    fn a_manual_check_puts_off_the_automatic_one() {
        let (feed, calls) = feed(Some("v1.0.0"));
        let (tx, events) = spawn(feed, false, Duration::from_secs(60 * 60));
        tx.send(Command::Check).expect("sent");
        tx.send(Command::Automatic(true)).expect("sent");
        let want = [UpdateState::Checking, UpdateState::UpToDate];
        assert_eq!(states(&events, 2), want);
        let n = calls_once(&calls, Duration::from_millis(200), |n| n > 1);
        assert_eq!(n, 1, "only the asked check");
    }

    /// With a downloaded update the timer neither checks nor downloads again, so only
    /// `Command::Install` installs it. A real `Prepared` needs a signed release, so `run` keeps
    /// this decision in `next_wait`.
    #[test]
    fn a_ready_update_waits_for_install() {
        let now = Instant::now();
        let due = now + Duration::from_secs(60);
        assert_eq!(next_wait(true, true, due, now), Duration::MAX);
        assert_eq!(next_wait(true, false, due, now), Duration::from_secs(60));
        let past = now - Duration::from_secs(1);
        assert_eq!(next_wait(true, false, past, now), Duration::ZERO);
        assert_eq!(next_wait(false, false, due, now), Duration::MAX);
    }

    #[test]
    fn install_without_a_download_does_nothing() {
        let (feed, calls) = feed(Some("v1.0.0"));
        let (tx, events) = spawn(feed, false, CHECK_INTERVAL);
        tx.send(Command::Install).expect("sent");
        quiet(&events, Duration::from_millis(200));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// Every status comes back as is, a redirect unfollowed, with the request's headers sent.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_transport_hands_back_every_status() {
        let server = MockServer::start().await;
        let moved = ResponseTemplate::new(302).insert_header("Location", "https://github.com/x");
        let replies = [
            ("/ok", ResponseTemplate::new(200).set_body_string("x")),
            ("/moved", moved),
            ("/gone", ResponseTemplate::new(404)),
        ];
        for (at, reply) in replies {
            Mock::given(method("GET"))
                .and(path(at))
                .and(header("accept", "application/vnd.github+json"))
                .and(header("user-agent", "Tidash/test"))
                .respond_with(reply)
                .expect(1)
                .mount(&server)
                .await;
        }
        let base = server.uri();
        // A reqwest blocking client must not be built, used or dropped on a runtime thread.
        let client = std::thread::spawn(move || {
            let http = Http::new().expect("a client");
            ["/ok", "/moved", "/gone"].map(|at| {
                let url = format!("{base}{at}");
                let request = Request {
                    url: &url,
                    accept: "application/vnd.github+json",
                    user_agent: "Tidash/test",
                };
                let mut response = http.get(&request).expect("an answer");
                let mut body = String::new();
                response.body.read_to_string(&mut body).expect("the body");
                (response.status, response.location, body)
            })
        });
        let got = client.join().expect("the client thread");
        let moved = Some("https://github.com/x".to_owned());
        let want = [
            (200, None, "x".to_owned()),
            (302, moved, String::new()),
            (404, None, String::new()),
        ];
        assert_eq!(got, want);
    }
}
