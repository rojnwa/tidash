use super::*;
use player::Source;
use std::io::Cursor;

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
        ..Default::default()
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
    (Playback::new(player, emit, msgs, Default::default()), rx)
}

/// Only for tests that never let it reach TIDAL.
fn client() -> Arc<Client> {
    client_with(Default::default())
}

fn client_with(cfg: tidal::ClientConfig) -> Arc<Client> {
    let session = tidal::Session {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        user_id: 1,
        country_code: "US".into(),
    };
    let store = Arc::new(tidal::MemoryStore::default());
    Arc::new(Client::new(cfg, session, store, None).expect("a client"))
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

/// Loads are spawned on this runtime, which never runs them: nothing reaches TIDAL.
fn idle_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
}

#[test]
fn each_hiccup_in_a_track_gets_one_restart() {
    let rt = idle_runtime();
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
        matches!(sent.try_recv(), Ok(Msg::SessionExpired(dead, _)) if Arc::ptr_eq(&dead, &c)),
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

/// A load superseded by the next one stops, instead of fetching a stream nobody plays.
#[test]
fn a_superseded_load_task_is_aborted() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        let server = MockServer::start().await;
        let gated = ResponseTemplate::new(200).set_delay(Duration::from_secs(600)); // never opens
        Mock::given(method("GET"))
            .respond_with(gated)
            .mount(&server)
            .await;
        let api_base = format!("{}/v1/", server.uri()).parse().expect("a URL");
        let c = client_with(tidal::ClientConfig {
            api_base,
            ..Default::default()
        });
        let c = Some(&c);
        let (mut p, _rx) = playback();
        let (tracks, start) = (tracks(2), 0);
        p.handle(c, Action::PlayTracks { tracks, start });
        let first = p.load.as_ref().expect("loading").abort_handle();
        let sent = async {
            while server
                .received_requests()
                .await
                .is_none_or(|r| r.is_empty())
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        let s5 = Duration::from_secs(5);
        tokio::time::timeout(s5, sent)
            .await
            .expect("the first stream request is sent");
        p.handle(c, Action::Next);
        let stopped = async {
            while !first.is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(s5, stopped)
            .await
            .expect("the first load was aborted");
        let second = p.load.as_ref().expect("loading");
        assert!(!second.is_finished(), "the second load runs");
    });
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

fn current_id(p: &Playback) -> Option<u64> {
    p.queue.current().map(|t| t.id)
}

#[test]
fn natural_end_with_repeat_one_replays() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let c = Some(&c);
    let (mut p, _rx) = playback();
    p.handle(c, Action::SetRepeat(crate::queue::Repeat::One));
    let (tracks, start) = (tracks(3), 1);
    p.handle(c, Action::PlayTracks { tracks, start });
    let end = |p: &mut Playback| {
        p.loading = false; // loaded and played to its end
        let id = p.seq;
        p.on_msg(c, Msg::Player(PlayerEvent::TrackEnded(id)));
    };
    end(&mut p);
    assert!(p.loading, "loads again");
    assert_eq!(current_id(&p), Some(1));
    p.handle(c, Action::SetRepeat(crate::queue::Repeat::Off));
    end(&mut p);
    assert_eq!(current_id(&p), Some(2), "off: the next one");
}

#[test]
fn removing_current_while_playing_loads_next() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let c = Some(&c);
    let (mut p, _rx) = playback();
    let (tracks, start) = (tracks(4), 1);
    p.handle(c, Action::PlayTracks { tracks, start });
    let now = |p: &mut Playback, state| {
        p.loading = false;
        p.state = state;
    };
    now(&mut p, PlayState::Playing);
    let seq = p.seq;
    p.handle(c, Action::RemoveFromQueue(0));
    assert_eq!(
        (p.seq, current_id(&p)),
        (seq, Some(1)),
        "not the current one"
    );
    p.handle(c, Action::RemoveFromQueue(0));
    assert_eq!(current_id(&p), Some(2));
    assert!(p.loading && p.want_play, "the next one loads and plays");
    p.handle(c, Action::Pause);
    now(&mut p, PlayState::Paused);
    p.handle(c, Action::RemoveFromQueue(0));
    assert_eq!(current_id(&p), Some(3));
    assert!(p.loading && !p.want_play, "the next one loads, paused");
    p.handle(c, Action::Play);
    now(&mut p, PlayState::Playing);
    p.handle(c, Action::RemoveFromQueue(0));
    assert_eq!(current_id(&p), None);
    assert!(!p.loading, "none left: stopped");
}

/// Playing track 0 of three, 200 s each, its play started. Needs a runtime entered.
fn playing(c: &Arc<Client>) -> (Playback, std::sync::mpsc::Receiver<Event>) {
    let (mut p, rx) = playback();
    let (tracks, start) = (tracks(3), 0);
    p.handle(Some(c), Action::PlayTracks { tracks, start });
    (p.loading, p.state) = (false, PlayState::Playing);
    let id = p.seq;
    p.on_msg(Some(c), Msg::Player(PlayerEvent::TrackStarted(id)));
    (p, rx)
}

/// A stream for `id` resolved: as a load's or a preload's result.
fn resolved(c: &Arc<Client>, id: u64) -> Msg {
    let source = Source {
        id,
        reader: Box::new(Cursor::new(Vec::new())),
        len: None,
        gain: 1.0,
    };
    Msg::Loaded {
        seq: id,
        client: c.clone(),
        result: Ok(Resolved {
            source,
            quality: "HI_RES_LOSSLESS".into(),
            codec: "flac".into(),
        }),
    }
}

/// Requests the next track's stream 15 s before the end and hands it to the player: its id.
fn sent_preload(p: &mut Playback, c: &Arc<Client>) -> u64 {
    p.maybe_preload(c, S * 185);
    let id = p.preload.as_ref().expect("a preload").id;
    p.on_msg(Some(c), resolved(c, id));
    assert!(p.preload.as_ref().is_some_and(|pre| pre.sent));
    id
}

#[test]
fn preloads_the_next_track_near_the_end() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let (mut p, _rx) = playing(&c);
    p.maybe_preload(&c, S * 170);
    assert!(p.preload.is_none(), "30 s left");
    p.maybe_preload(&c, S * 185);
    let pre = p.preload.as_ref().expect("a preload");
    assert_eq!((pre.track_id, pre.sent), (1, false));
    assert_ne!(pre.id, p.seq, "an id of its own");
    let id = pre.id;
    p.maybe_preload(&c, S * 186);
    assert_eq!(
        p.preload.as_ref().map(|pre| pre.id),
        Some(id),
        "requested once"
    );
}

#[test]
fn a_preloaded_track_starts_without_a_load() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let (mut p, _rx) = playing(&c);
    let x = sent_preload(&mut p, &c);
    p.on_msg(Some(&c), Msg::Player(PlayerEvent::TrackStarted(x)));
    assert_eq!(current_id(&p), Some(1));
    assert_eq!(p.seq, x);
    assert!(!p.loading);
    assert!(p.preload.is_none());
    assert_eq!(
        (p.quality.as_str(), p.codec.as_str()),
        ("HI_RES_LOSSLESS", "flac")
    );
    let play = p.play.as_ref().expect("a new play");
    assert_eq!(play.track_id, 1);
    // No State event comes at a gapless switch: the play counts from its start all the same.
    let report = report_for(play, End::Natural, Instant::now() + S * 31, 0);
    let quality = report.expect("reported").quality;
    assert_eq!(quality, "HI_RES_LOSSLESS");
}

fn track_9() -> Vec<tidal::Track> {
    vec![tidal::Track { id: 9, ..track() }]
}

#[test]
fn a_queue_edit_cancels_a_stale_preload() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let (mut p, _rx) = playing(&c);
    let x = sent_preload(&mut p, &c);
    p.handle(Some(&c), Action::PlayNext(track_9()));
    assert!(p.preload.is_none());
    assert_eq!(p.cancelled, Some(x));
    // The player went on before the cancel landed: the current track has ended.
    p.on_msg(Some(&c), Msg::Player(PlayerEvent::TrackStarted(x)));
    assert_eq!(current_id(&p), Some(9));
    assert!(p.loading, "loads it");
}

#[test]
fn a_late_preload_result_is_dropped() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let (mut p, _rx) = playing(&c);
    p.maybe_preload(&c, S * 185);
    let x = p.preload.as_ref().expect("a preload").id;
    p.handle(Some(&c), Action::PlayNext(track_9()));
    assert!(p.preload.is_none());
    let seq = p.seq;
    p.on_msg(Some(&c), resolved(&c, x));
    assert!(p.preload.is_none() && p.cancelled.is_none(), "nothing sent");
    assert_eq!((p.seq, p.loading), (seq, false), "nor loaded");
    p.maybe_preload(&c, S * 186);
    let next = p.preload.as_ref().map(|pre| pre.track_id);
    assert_eq!(next, Some(9), "the new next track instead");
}

#[test]
fn repeat_one_preloads_the_same_track() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let (mut p, _rx) = playing(&c);
    p.handle(Some(&c), Action::SetRepeat(crate::queue::Repeat::One));
    let x = sent_preload(&mut p, &c);
    assert_eq!(p.preload.as_ref().map(|pre| pre.track_id), Some(0));
    p.on_msg(Some(&c), Msg::Player(PlayerEvent::TrackStarted(x)));
    assert_eq!((current_id(&p), p.seq, p.loading), (Some(0), x, false));
}

/// A preload behind a format change fails on its first packet: the player stopped on it.
#[test]
fn an_error_from_the_preloaded_track_moves_to_it() {
    let rt = idle_runtime();
    let _rt = rt.enter();
    let c = client();
    let (mut p, rx) = playing(&c);
    let x = sent_preload(&mut p, &c);
    p.last_pos = S * 199; // the ended track's
    let message = "bad frame".into();
    p.on_msg(Some(&c), Msg::Player(PlayerEvent::Error { id: x, message }));
    assert_eq!(current_id(&p), Some(1));
    assert!(p.loading, "restarted");
    assert_eq!(p.restarted_at, Some(Duration::ZERO), "from its start");
    assert!(errors(&rx).is_empty());
}

/// The stream request asks for the quality the settings choose.
#[test]
fn a_stream_request_asks_for_the_chosen_quality() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        let server = MockServer::start().await;
        let gated = ResponseTemplate::new(200).set_delay(Duration::from_secs(600)); // never answers
        Mock::given(method("GET"))
            .respond_with(gated)
            .mount(&server)
            .await;
        let api_base = format!("{}/v1/", server.uri()).parse().expect("a URL");
        let c = client_with(tidal::ClientConfig {
            api_base,
            ..Default::default()
        });
        let (mut p, _rx) = playback();
        for (n, hi_res) in [(1, true), (2, false)] {
            p.audio.hi_res = hi_res;
            let _request = p.request(&c, 7, n);
            let sent = async {
                while server.received_requests().await.map_or(0, |r| r.len()) < n as usize {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(5), sent)
                .await
                .expect("the request is sent");
        }
        let requests = server.received_requests().await.expect("recorded");
        let asked: Vec<_> = requests
            .iter()
            .map(|r| {
                let mut q = r.url.query_pairs();
                q.find(|(k, _)| k == "audioquality")
                    .map(|(_, v)| v.into_owned())
            })
            .collect();
        let want = ["HI_RES_LOSSLESS", "LOSSLESS"].map(|q| Some(q.to_string()));
        assert_eq!(asked, want);
    });
}

#[test]
fn toggle_play_pauses_and_resumes() {
    let (mut p, _rx) = playback();
    p.queue.replace(tracks(2), 0);
    for (state, loading, plays) in [
        (PlayState::Playing, false, false),
        (PlayState::Buffering, false, false),
        (PlayState::Stopped, true, false), // the next track resolving
        (PlayState::Paused, false, true),
        (PlayState::Stopped, false, true),
    ] {
        (p.state, p.loading, p.want_play) = (state, loading, !plays);
        p.handle(None, Action::TogglePlay);
        assert_eq!(p.want_play, plays, "{state:?}, loading: {loading}");
    }
}

/// The output under a started 24/96 track, as the player tells it.
fn format(exclusive: bool) -> Msg {
    let track = player::Format {
        rate: 96_000,
        bits: 24,
        channels: 2,
    };
    let device_rate = if exclusive { 96_000 } else { 48_000 };
    Msg::Player(PlayerEvent::Format {
        track,
        exclusive,
        device_rate,
    })
}

/// The `output` of each snapshot published since the last call.
fn outputs(rx: &std::sync::mpsc::Receiver<Event>) -> Vec<Option<OutputInfo>> {
    rx.try_iter()
        .filter_map(|e| match e {
            Event::Playback(s) => Some(s.output),
            _ => None,
        })
        .collect()
}

#[test]
fn format_event_fills_the_snapshot() {
    let (mut p, rx) = playback();
    p.codec = "flac".into();
    p.on_msg(None, format(true));
    let want = OutputInfo {
        codec: "FLAC".into(),
        format: player::Format {
            rate: 96_000,
            bits: 24,
            channels: 2,
        },
        exclusive: true,
        device_rate: 96_000,
    };
    assert_eq!(outputs(&rx).last(), Some(&Some(want)));
    p.stop();
    assert_eq!(outputs(&rx).last(), Some(&None), "stopped: no output shown");
}

/// At the queue's end the player stops itself: no `stop` runs, its State tells.
#[test]
fn a_stopped_player_shows_no_output() {
    let (mut p, rx) = playback();
    p.on_msg(None, format(false));
    p.on_msg(None, Msg::Player(PlayerEvent::State(PlayState::Stopped)));
    assert_eq!(outputs(&rx).last(), Some(&None));
}

#[test]
fn fallback_notice_once_per_streak() {
    let (mut p, rx) = playback();
    let refused = || Msg::Player(PlayerEvent::Fallback("no 96 kHz".into()));
    p.on_msg(None, refused());
    p.on_msg(None, format(false)); // the shared output that took over
    p.on_msg(None, refused()); // the next track, refused again
    assert_eq!(
        errors(&rx),
        ["Exclusive output unavailable (no 96 kHz); playing in shared mode"]
    );
    p.on_msg(None, format(true));
    p.on_msg(None, refused());
    assert_eq!(
        errors(&rx).len(),
        1,
        "exclusive worked in between: a new streak"
    );
}
