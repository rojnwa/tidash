use super::*;
use fastframe_audio::Render;

fn fixture(id: u64) -> Source {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/s16_44100.mp4");
    let file = std::fs::File::open(path).unwrap();
    let len = file.metadata().unwrap().len();
    Source {
        id,
        reader: Box::new(file),
        len: Some(len),
        gain: 1.0,
    }
}

/// `sync_output` applies `output_action` at the top of every loop iteration; this drives
/// the engine synchronously through each transition.
#[test]
fn output_runs_only_with_an_unpaused_track() {
    let (_player, _events, mut r, mut e) = parts();
    let mut block = [0.0f32; 960];
    r.configure(48_000, 2);
    assert_eq!(e.output_action(), Some(false), "stopped at launch");
    e.command(PlayerCmd::Play);
    assert_eq!(e.output_action(), Some(false), "play with nothing loaded");
    e.command(PlayerCmd::Load(fixture(1)));
    assert_eq!(e.output_action(), Some(true), "loaded");
    r.render(&mut block); // takes the load flushes
    e.command(PlayerCmd::Pause);
    assert_eq!(e.output_action(), Some(false), "paused");
    e.command(PlayerCmd::Play);
    assert_eq!(e.output_action(), Some(true), "playing");
    e.command(PlayerCmd::Stop);
    assert_eq!(
        e.output_action(),
        None,
        "runs until the stop flush is taken"
    );
    r.render(&mut block);
    assert_eq!(e.output_action(), Some(false), "stopped");
    let broken = Source {
        id: 2,
        reader: Box::new(io::empty()),
        len: None,
        gain: 1.0,
    };
    e.command(PlayerCmd::Load(broken));
    r.render(&mut block);
    assert_eq!(e.output_action(), Some(false), "load failed");
    e.command(PlayerCmd::Load(fixture(3)));
    for _ in 0..10_000 {
        if e.track.is_none() {
            break;
        }
        r.render(&mut block);
        e.step();
    }
    assert!(e.track.is_none(), "track did not end");
    assert_eq!(e.output_action(), None, "ended: takes the stop flush first");
    r.render(&mut block);
    assert_eq!(e.output_action(), Some(false), "ended");
    // A device that never calls back is paused anyway once the grace is over.
    e.command(PlayerCmd::Load(fixture(4)));
    e.command(PlayerCmd::Stop);
    assert_eq!(e.output_action(), None);
    std::thread::sleep(STOP_GRACE + Duration::from_millis(10));
    assert_eq!(e.output_action(), Some(false), "no callback: paused anyway");
}

/// After a failed call the decoder is rebuilt and resumes exactly where it was.
#[test]
fn stale_decoder_resumes_seamlessly() {
    let decode_all = |stale_at: Option<usize>| {
        let (_player, _events, _r, mut e) = parts();
        e.command(PlayerCmd::Load(fixture(1)));
        let t = e.track.as_mut().unwrap();
        let mut out = Vec::new();
        for i in 0.. {
            if Some(i) == stale_at {
                t.stale = true;
            }
            let Some(p) = t.next().unwrap() else { break };
            let drop = t.skip.min(p.samples.len() / t.channels);
            t.skip -= drop;
            out.extend_from_slice(&p.samples[drop * t.channels..]);
        }
        out
    };
    let straight = decode_all(None);
    assert_eq!(straight.len(), 22_050 * 2);
    assert!(decode_all(Some(5)) == straight, "resumed decode differs");
}

/// A decoder gone stale at the very end is rebuilt there and finds nothing left: the end of
/// the stream, not an error.
#[test]
fn a_stale_decoder_at_the_end_ends_the_stream() {
    let (_player, _events, _r, mut e) = parts();
    e.command(PlayerCmd::Load(fixture(1)));
    let t = e.track.as_mut().unwrap();
    while t.next().unwrap().is_some() {}
    t.stale = true;
    let next = t.next().map(|p| p.is_some());
    assert!(matches!(next, Ok(false)), "{next:?}");
}

/// Only a forward seek can be refused for lying past the end. A backward one that fails
/// (here the rebuilt decoder finds the data gone bad) is an error, not the track's end.
#[test]
fn a_failed_backward_seek_is_an_error() {
    let (_player, events, _r, mut e) = parts();
    e.command(PlayerCmd::Load(fixture(1)));
    let t = e.track.as_mut().unwrap();
    for _ in 0..5 {
        t.next().unwrap();
    }
    let garbage = io::repeat(0x5a).take(64 * 1024);
    t.file = ProgressiveFile::spawn(Box::new(garbage), None, &e.interrupt).unwrap();
    e.command(PlayerCmd::Seek(Duration::ZERO));
    let seen: Vec<_> = events.try_iter().collect();
    assert!(
        seen.iter()
            .any(|ev| matches!(ev, PlayerEvent::Error { id: 1, .. })),
        "{seen:?}"
    );
    assert!(!seen.contains(&PlayerEvent::TrackEnded(1)), "{seen:?}");
}

/// A forward seek from a stale decoder rebuilds it too: when that fails, it is an error.
#[test]
fn a_failed_forward_seek_on_a_stale_decoder_is_an_error() {
    let (_player, events, _r, mut e) = parts();
    e.command(PlayerCmd::Load(fixture(1)));
    let t = e.track.as_mut().unwrap();
    for _ in 0..5 {
        t.next().unwrap();
    }
    let to = Duration::from_millis(400);
    assert!(!t.behind(to), "not a forward seek");
    t.stale = true;
    let garbage = io::repeat(0x5a).take(64 * 1024);
    t.file = ProgressiveFile::spawn(Box::new(garbage), None, &e.interrupt).unwrap();
    e.command(PlayerCmd::Seek(to));
    let seen: Vec<_> = events.try_iter().collect();
    assert!(
        seen.iter()
            .any(|ev| matches!(ev, PlayerEvent::Error { id: 1, .. })),
        "{seen:?}"
    );
    assert!(!seen.contains(&PlayerEvent::TrackEnded(1)), "{seen:?}");
}

/// A rebuilt decoder refusing a seek past the end still ends the track.
#[test]
fn a_seek_past_the_end_on_a_stale_decoder_ends_the_track() {
    let (_player, events, _r, mut e) = parts();
    e.command(PlayerCmd::Load(fixture(1)));
    let t = e.track.as_mut().unwrap();
    for _ in 0..5 {
        t.next().unwrap();
    }
    t.stale = true;
    e.command(PlayerCmd::Seek(Duration::from_secs(10)));
    let seen: Vec<_> = events.try_iter().collect();
    assert!(seen.contains(&PlayerEvent::TrackEnded(1)), "{seen:?}");
    assert!(
        !seen
            .iter()
            .any(|ev| matches!(ev, PlayerEvent::Error { .. })),
        "{seen:?}"
    );
}

#[test]
fn a_lost_output_is_reported_once_per_outage() {
    let (_player, events, _r, mut e) = parts();
    for failed in [true, true, false, true] {
        e.output_failed(failed.then_some(OpenError::NoDevice));
    }
    let lost = events
        .try_iter()
        .filter(|ev| *ev == PlayerEvent::OutputLost)
        .count();
    assert_eq!(lost, 2);
}

#[test]
fn polls_fast_for_a_flush_only_while_called_back() {
    let (_player, _events, mut r, mut e) = parts();
    let mut block = [0.0f32; 960];
    r.configure(48_000, 2);
    e.command(PlayerCmd::Load(fixture(1)));
    r.render(&mut block); // takes the load flushes: a callback just now
    e.command(PlayerCmd::Seek(Duration::from_millis(100)));
    assert_eq!(e.idle_wait(), FLUSH_WAIT);
    std::thread::sleep(CALLBACK_RECENT + Duration::from_millis(10));
    assert_eq!(e.idle_wait(), IDLE_WAIT, "no callbacks: device gone");
}
