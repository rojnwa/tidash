use super::*;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

/// A reader over `segments` (url -> bytes) whose `open` logs each URL and fails on `bad`.
fn reader(
    segments: &[(&str, &[u8])],
    bad: Option<&'static str>,
) -> (SegmentReader, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let data: Vec<(String, Vec<u8>)> = segments
        .iter()
        .map(|(u, b)| (u.to_string(), b.to_vec()))
        .collect();
    let urls = data.iter().map(|(u, _)| u.clone()).collect();
    let opened = log.clone();
    let r = SegmentReader::new(urls, move |url| {
        opened.lock().expect("log").push(url.to_string());
        if Some(url) == bad {
            return Err(io::Error::other("403"));
        }
        let bytes = data
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, b)| b.clone())
            .unwrap_or_default();
        Ok(Box::new(Cursor::new(bytes)) as Body)
    });
    (r, log)
}

#[test]
fn segment_reader_concatenates_in_order() {
    let (mut r, log) = reader(&[("init", b"ab"), ("1", b"cde"), ("2", b"f")], None);
    assert!(
        log.lock().expect("log").is_empty(),
        "nothing opened before the first read"
    );
    let mut first = [0u8; 1];
    assert_eq!(r.read(&mut first).expect("read"), 1);
    assert_eq!(*log.lock().expect("log"), ["init"], "opened lazily");
    let mut rest = Vec::new();
    r.read_to_end(&mut rest).expect("read_to_end");
    assert_eq!([&first[..], &rest].concat(), b"abcdef");
    assert_eq!(*log.lock().expect("log"), ["init", "1", "2"]);
}

#[test]
fn segment_reader_surfaces_open_error() {
    let (mut r, _) = reader(&[("init", b"ab"), ("1", b"cd"), ("2", b"ef")], Some("1"));
    let mut got = Vec::new();
    assert!(r.read_to_end(&mut got).is_err());
    assert_eq!(
        got, b"ab",
        "the first segment's bytes come before the error"
    );
}

fn track() -> tidal::Track {
    tidal::Track {
        id: 7,
        title: "t".into(),
        duration: 200,
        track_number: 1,
        volume_number: 1,
        artists: vec![],
        album: tidal::AlbumRef {
            id: 42,
            title: "a".into(),
            cover: None,
        },
        stream_ready: true,
    }
}

const S: Duration = Duration::from_secs(1);

/// A play that started at `t0` and was playing for `secs` seconds by `t0 + secs`.
fn played(t0: Instant, secs: u32) -> PlayAccounting {
    let mut p = PlayAccounting::new(&track(), "LOSSLESS".into(), Duration::ZERO, 1_000);
    p.set_playing(true, t0);
    p.set_playing(false, t0 + S * secs);
    p
}

#[test]
fn short_play_is_not_reported() {
    let t0 = Instant::now();
    assert!(report_for(&played(t0, 29), End::Natural, t0 + S * 29, 30_000).is_none());
}

#[test]
fn natural_end_reports_the_duration() {
    let t0 = Instant::now();
    let r = report_for(&played(t0, 199), End::Natural, t0 + S * 199, 200_000).expect("report");
    assert_eq!(r.end_position_s, 200.0);
    assert_eq!(
        (
            r.track_id,
            r.start_position_s,
            r.started_at_ms,
            r.ended_at_ms
        ),
        (7, 0.0, 1_000, 200_000)
    );
    assert_eq!(
        (
            r.source_type.as_str(),
            r.source_id.as_str(),
            r.quality.as_str()
        ),
        ("ALBUM", "42", "LOSSLESS")
    );
    assert_eq!(r.streaming_session_id.len(), 36);
}

#[test]
fn skip_reports_the_position_clamped_to_the_duration() {
    let t0 = Instant::now();
    let p = played(t0, 95);
    let at = |secs| {
        report_for(&p, End::At(S * secs), t0 + S * 95, 2)
            .expect("report")
            .end_position_s
    };
    assert_eq!(at(95), 95.0);
    assert_eq!(at(250), 200.0);
}

/// Device-less player, no client: every load stops at once, so no runtime is needed.
fn playback() -> (Playback, std::sync::mpsc::Receiver<Event>) {
    let (player, _events, _renderer) = Player::spawn_manual();
    let (tx, rx) = std::sync::mpsc::channel();
    let emit = Arc::new(Emitter {
        tx,
        waker: Box::new(|| {}),
    });
    let (msgs, _) = tokio::sync::mpsc::unbounded_channel();
    (Playback::new(player, emit, msgs), rx)
}

/// Only for tests that never let it reach TIDAL.
fn client() -> Arc<Client> {
    let session = tidal::Session {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        user_id: 1,
        country_code: "US".into(),
    };
    let store = Arc::new(tidal::MemoryStore::default());
    Arc::new(Client::new(Default::default(), session, store, None).expect("a client"))
}

fn tracks(n: u64) -> Vec<tidal::Track> {
    (0..n)
        .map(|id| tidal::Track {
            id,
            title: format!("t{id}"),
            ..track()
        })
        .collect()
}

fn errors(rx: &std::sync::mpsc::Receiver<Event>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|e| {
            if let Event::Error(e) = e {
                Some(e)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn three_failures_in_a_row_stop_and_a_new_queue_starts_over() {
    let (mut p, rx) = playback();
    p.handle(
        None,
        Action::PlayTracks {
            tracks: tracks(9),
            start: 0,
        },
    );
    p.fail_track(None);
    p.fail_track(None);
    p.handle(
        None,
        Action::PlayTracks {
            tracks: tracks(9),
            start: 0,
        },
    );
    assert_eq!(p.failures, 0, "a new queue starts over");
    for _ in 0..3 {
        p.fail_track(None);
    }
    let errs = errors(&rx);
    assert_eq!(errs.len(), 6);
    assert_eq!(errs[0], "t0 is unavailable");
    assert_eq!(errs[5], "Playback stopped after repeated errors");
    p.fail_track(None);
    p.reset(None);
    assert_eq!(p.failures, 0, "logout starts over");
}

#[test]
fn a_lost_output_is_shown() {
    let (mut p, rx) = playback();
    p.on_msg(None, Msg::Player(PlayerEvent::OutputLost));
    assert_eq!(
        errors(&rx),
        ["Audio output lost; playback resumes when it is back"]
    );
}

#[test]
fn each_hiccup_in_a_track_gets_one_restart() {
    // Loads are spawned on this runtime, which never runs them: nothing reaches TIDAL.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime");
    let _rt = rt.enter();
    let c = client();
    let c = Some(&c);
    let (mut p, rx) = playback();
    p.handle(
        c,
        Action::PlayTracks {
            tracks: tracks(3),
            start: 0,
        },
    );
    let hiccup = |p: &mut Playback, at_secs: u32| {
        p.last_pos = S * at_secs; // as sampled while it played
        let id = p.seq;
        let message = "connection reset".into();
        p.on_msg(c, Msg::Player(PlayerEvent::Error { id, message }));
    };
    hiccup(&mut p, 120);
    hiccup(&mut p, 900);
    assert_eq!(p.restarted_at, Some(S * 900));
    assert!(errors(&rx).is_empty(), "both restarted");
    hiccup(&mut p, 901); // again right where it resumed
    assert_eq!(errors(&rx), ["t0 is unavailable"]);
}

#[test]
fn a_dead_session_stops_the_load_but_fails_no_track() {
    let (mut p, rx) = playback();
    let (msgs, mut sent) = tokio::sync::mpsc::unbounded_channel();
    p.msgs = msgs;
    p.queue.replace(tracks(3), 0);
    let c = client();
    let fail_load = |p: &mut Playback, e: tidal::Error| {
        p.loading = true;
        let (seq, client, result) = (p.seq, c.clone(), Err(e.into()));
        p.on_msg(
            None,
            Msg::Loaded {
                seq,
                client,
                result,
            },
        );
    };
    fail_load(&mut p, tidal::Error::Auth("invalid_grant".into()));
    assert!(
        matches!(sent.try_recv(), Ok(Msg::SessionExpired(dead)) if Arc::ptr_eq(&dead, &c)),
        "the load's client is the dead one"
    );
    assert!(errors(&rx).is_empty(), "no track reported unavailable");
    assert_eq!(p.queue.current().map(|t| t.id), Some(0), "the queue stays");
    assert!(!p.loading);
    let offline = tidal::Error::Status {
        status: 503,
        sub_status: None,
        message: "down".into(),
    };
    fail_load(&mut p, offline);
    assert!(sent.try_recv().is_err(), "the session is fine");
    assert_eq!(errors(&rx), ["t0 is unavailable"]);
}

#[test]
fn previous_on_the_first_track_seeks_instead_of_reloading() {
    let (mut p, _rx) = playback();
    p.queue.replace(tracks(3), 0);
    p.state = PlayState::Playing;
    let seq = p.seq;
    p.handle(None, Action::Previous);
    assert_eq!(p.seq, seq, "no reload");
    p.queue.next();
    p.handle(None, Action::Previous);
    assert_eq!(p.queue.current().map(|t| t.id), Some(0));
    assert_ne!(p.seq, seq, "the previous track loads");
}

#[test]
fn seek_while_loading_waits_for_the_load() {
    let (mut p, _rx) = playback();
    p.loading = true;
    p.handle(None, Action::Seek(S * 5));
    assert_eq!(p.resume_at, Some(S * 5));
    p.loading = false;
    p.handle(None, Action::Seek(S * 7));
    assert_eq!(p.resume_at, Some(S * 5), "loaded: goes to the player");
}

#[test]
fn pause_time_is_excluded() {
    let t0 = Instant::now();
    let mut p = PlayAccounting::new(&track(), "LOSSLESS".into(), Duration::ZERO, 0);
    p.set_playing(true, t0);
    p.set_playing(false, t0 + S * 20); // paused for 100 s
    p.set_playing(true, t0 + S * 120);
    assert!(
        report_for(&p, End::At(S * 25), t0 + S * 125, 0).is_none(),
        "25 s played"
    );
    assert!(
        report_for(&p, End::At(S * 30), t0 + S * 130, 0).is_some(),
        "30 s played, still running"
    );
}
