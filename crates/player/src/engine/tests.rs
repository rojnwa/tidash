use super::*;
use crate::exclusive::{Exclusive, to_i32};
use fastframe_audio::Render;
use std::sync::atomic::AtomicBool;

fn source(id: u64, name: &str) -> Source {
    let path = format!("{}/tests/fixtures/{name}.mp4", env!("CARGO_MANIFEST_DIR"));
    let file = std::fs::File::open(path).unwrap();
    let len = file.metadata().unwrap().len();
    Source {
        id,
        reader: Box::new(file),
        len: Some(len),
        gain: 1.0,
    }
}

fn fixture(id: u64) -> Source {
    source(id, "s16_44100")
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

/// A seek before the boundary takes the switch back: the ending track is current again, the
/// next one a preload again, and the renderer's boundary is gone.
#[test]
fn a_seek_before_the_boundary_undoes_the_switch() {
    let (_player, _events, mut r, mut e) = parts();
    r.configure(48_000, 2);
    e.command(PlayerCmd::Load(fixture(1)));
    e.command(PlayerCmd::Preload(Some(fixture(2))));
    r.render(&mut [0.0; 960]); // takes the load flushes
    for _ in 0..1_000 {
        if e.ending.is_some() {
            break;
        }
        e.step(); // all of track 1 fits in the ring: no rendering needed
    }
    assert!(e.ending.is_some(), "no switch");
    assert_eq!(e.track.as_ref().unwrap().id, 2);
    e.command(PlayerCmd::Seek(Duration::from_millis(100)));
    assert!(e.ending.is_none());
    assert_eq!(e.track.as_ref().unwrap().id, 1);
    assert_eq!(e.next.as_ref().unwrap().id, 2);
    assert_eq!(e.shared.boundary.load(Relaxed), u64::MAX);
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

const S16_44100: Format = Format {
    rate: 44_100,
    bits: 16,
    channels: 2,
};
const S24_96000: Format = Format {
    rate: 96_000,
    bits: 24,
    channels: 2,
};

fn exclusive() -> OutputConfig {
    OutputConfig {
        exclusive: true,
        ..OutputConfig::default()
    }
}

/// A test's exclusive device: the formats it was asked to open, and for each open, the
/// renderer it was given and its lost-device flag.
struct Dac {
    asked: Receiver<Format>,
    opened: Receiver<(Renderer, Arc<AtomicBool>)>,
}

impl Dac {
    fn asked(&self) -> Vec<Format> {
        self.asked.try_iter().collect()
    }
}

/// Replaces `e`'s exclusive opener. It takes a format when `accept` says so, else refuses
/// with "no 88.2 kHz"; it configures the renderer it gets at that format and returns a
/// detached stream.
fn dac(e: &mut Engine, accept: fn(Format) -> bool) -> Dac {
    let (asked_tx, asked) = mpsc::channel();
    let (opened_tx, opened) = mpsc::channel();
    e.open_exclusive = Box::new(move |_, format, mut r| {
        asked_tx.send(format).unwrap();
        if !accept(format) {
            return Err("no 88.2 kHz".into());
        }
        r.configure(format.rate, format.channels);
        let (x, lost) = Exclusive::detached(format);
        opened_tx.send((r, lost)).unwrap();
        Ok(x)
    });
    Dac { asked, opened }
}

/// One pass of the player thread's loop after its commands: take a new ring, note a switch,
/// decode until the ring is full.
fn run(e: &mut Engine) {
    e.adopt_ring();
    e.poll_switch();
    while e.step() {}
}

/// 480 frames of stereo: 10 ms at 48 kHz.
const BLOCK: usize = 960;

/// Renders `blocks` blocks from `r`, running `e` before each.
fn render(e: &mut Engine, r: &mut Renderer, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0; blocks * BLOCK];
    for block in out.chunks_mut(BLOCK) {
        run(e);
        r.render(block);
    }
    out
}

/// Renders from `r`, running `e` before each block, until `done` holds for the events seen:
/// the samples and the events.
fn render_until(
    e: &mut Engine,
    r: &mut Renderer,
    events: &Receiver<PlayerEvent>,
    mut done: impl FnMut(&[PlayerEvent]) -> bool,
) -> (Vec<f32>, Vec<PlayerEvent>) {
    let (mut out, mut seen) = (Vec::new(), Vec::new());
    for _ in 0..10_000 {
        run(e);
        seen.extend(events.try_iter());
        if done(&seen) {
            return (out, seen);
        }
        let mut block = [0.0; BLOCK];
        r.render(&mut block);
        out.extend_from_slice(&block);
    }
    panic!("not done; saw {seen:?}");
}

fn ended(id: u64) -> impl FnMut(&[PlayerEvent]) -> bool {
    move |seen| seen.contains(&PlayerEvent::TrackEnded(id))
}

/// The fixture's `.raw` reference: interleaved, left-justified `i32`.
fn raw(name: &str) -> Vec<i32> {
    let path = format!("{}/tests/fixtures/{name}.raw", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).unwrap();
    let (words, _) = bytes.as_chunks::<4>();
    words.iter().map(|b| i32::from_le_bytes(*b)).collect()
}

/// Stereo samples without their leading and trailing silent frames.
fn trimmed(samples: &[i32]) -> &[i32] {
    let frames = samples.as_chunks::<2>().0;
    let start = frames.iter().position(|f| *f != [0; 2]);
    let start = start.unwrap_or(frames.len());
    let end = frames.iter().rposition(|f| *f != [0; 2]);
    &samples[start * 2..end.map_or(start, |i| i + 1) * 2]
}

/// `out`, each sample as the device's integer, is the fixture's reference.
fn assert_bit_identical(out: &[f32], name: &str) {
    let got: Vec<i32> = out.iter().map(|&v| to_i32(v)).collect();
    let want = raw(name);
    let (got, want) = (trimmed(&got), trimmed(&want));
    let first = got.iter().zip(want).position(|(g, w)| g != w);
    assert!(
        got.len() == want.len() && first.is_none(),
        "{name}: {} vs {} samples; first difference at {first:?}",
        got.len(),
        want.len()
    );
}

fn sounding_frames(samples: &[f32]) -> usize {
    let frames = samples.as_chunks::<2>().0;
    frames.iter().filter(|f| **f != [0.0; 2]).count()
}

/// Plays `src` (id 1) to its end on an exclusive device that takes every format: the formats
/// it was asked for, and what it rendered.
fn play_exclusive(
    e: &mut Engine,
    events: &Receiver<PlayerEvent>,
    src: Source,
) -> (Vec<Format>, Vec<f32>) {
    let dac = dac(e, |_| true);
    e.command(PlayerCmd::SetOutput(exclusive()));
    e.command(PlayerCmd::Load(src));
    let asked = dac.asked();
    let (mut r, _) = dac.opened.try_recv().unwrap();
    let (out, _) = render_until(e, &mut r, events, ended(1));
    (asked, out)
}

#[test]
fn exclusive_output_is_bit_identical() {
    for (name, format) in [
        ("s16_44100", S16_44100),
        ("s24_96000", S24_96000),
        (
            "s24_192000",
            Format {
                rate: 192_000,
                ..S24_96000
            },
        ),
    ] {
        let (_player, events, _r, mut e) = parts();
        let (asked, out) = play_exclusive(&mut e, &events, source(1, name));
        assert_eq!(asked, [format], "{name}");
        assert_bit_identical(&out, name);
    }
}

#[test]
fn exclusive_ignores_volume_and_gain() {
    let (_player, events, _r, mut e) = parts();
    e.command(PlayerCmd::SetVolume(0.5));
    let src = Source {
        gain: 0.5,
        ..source(1, "s24_96000")
    };
    let (_, out) = play_exclusive(&mut e, &events, src);
    assert_bit_identical(&out, "s24_96000");
}

#[test]
fn a_refused_exclusive_plays_shared_with_a_fallback() {
    let (_player, events, mut r, mut e) = parts();
    r.configure(48_000, 2);
    let dac = dac(&mut e, |_| false);
    e.command(PlayerCmd::SetOutput(exclusive()));
    e.command(PlayerCmd::Load(fixture(1)));
    assert_eq!(dac.asked(), [S16_44100]);
    let (out, seen) = render_until(&mut e, &mut r, &events, ended(1));
    let fallback = seen
        .iter()
        .position(|ev| *ev == PlayerEvent::Fallback("no 88.2 kHz".into()));
    let format = seen.iter().position(|ev| {
        matches!(
            ev,
            PlayerEvent::Format {
                exclusive: false,
                device_rate: 48_000,
                ..
            }
        )
    });
    assert!(fallback.is_some() && fallback < format, "{seen:?}");
    let frames = sounding_frames(&out);
    assert!((23_760..=24_240).contains(&frames), "frames = {frames}"); // 500 ms at 48 kHz
}

#[test]
fn same_format_next_track_keeps_the_device() {
    let (_player, events, _r, mut e) = parts();
    let dac = dac(&mut e, |_| true);
    e.command(PlayerCmd::SetOutput(exclusive()));
    e.command(PlayerCmd::Load(fixture(1)));
    e.command(PlayerCmd::Preload(Some(fixture(2))));
    let (mut r, _) = dac.opened.try_recv().unwrap();
    let (_, seen) = render_until(&mut e, &mut r, &events, ended(2));
    assert_eq!(dac.asked(), [S16_44100]);
    assert!(seen.contains(&PlayerEvent::TrackStarted(2)), "{seen:?}");
    assert!(!seen.contains(&PlayerEvent::TrackEnded(1)), "{seen:?}");
}

#[test]
fn format_change_reopens_after_the_tail() {
    let (_player, events, _r, mut e) = parts();
    let dac = dac(&mut e, |_| true);
    e.command(PlayerCmd::SetOutput(exclusive()));
    e.command(PlayerCmd::Load(fixture(1)));
    e.command(PlayerCmd::Preload(Some(source(2, "s24_96000"))));
    let (mut first, _) = dac.opened.try_recv().unwrap();
    let mut second = None;
    let (out, mut seen) = render_until(&mut e, &mut first, &events, |_| {
        second = dac.opened.try_recv().ok();
        second.is_some()
    });
    assert_bit_identical(&out, "s16_44100");
    assert_eq!(dac.asked(), [S16_44100, S24_96000]);
    let (mut second, _) = second.unwrap();
    let b =
        |ev: &PlayerEvent| matches!(ev, PlayerEvent::Format { track, .. } if track.rate == 96_000);
    let (_, rest) = render_until(&mut e, &mut second, &events, |seen| seen.iter().any(b));
    seen.extend(rest);
    let started = seen
        .iter()
        .position(|ev| *ev == PlayerEvent::TrackStarted(2));
    let format = seen.iter().position(b);
    assert!(started.is_some() && started < format, "{seen:?}");
    let want = PlayerEvent::Format {
        track: S24_96000,
        exclusive: true,
        device_rate: 96_000,
    };
    assert_eq!(seen[format.unwrap()], want);
}

#[test]
fn a_lost_exclusive_device_falls_back_and_continues() {
    let (_player, events, _r, mut e) = parts();
    let dac = dac(&mut e, |_| true);
    e.command(PlayerCmd::SetOutput(exclusive()));
    e.command(PlayerCmd::Load(fixture(1)));
    let (mut r, lost) = dac.opened.try_recv().unwrap();
    render(&mut e, &mut r, 10); // 100 ms
    let before = e.shared.position();
    lost.store(true, Relaxed);
    e.maintain();
    let seen: Vec<_> = events.try_iter().collect();
    let fallback = seen
        .iter()
        .position(|ev| *ev == PlayerEvent::Fallback("audio device lost".into()));
    let format = seen.iter().rposition(|ev| {
        matches!(
            ev,
            PlayerEvent::Format {
                exclusive: false,
                ..
            }
        )
    });
    assert!(fallback.is_some() && fallback < format, "{seen:?}");
    let after = e.shared.position();
    assert!(after >= before, "{before:?} -> {after:?}");
}

#[test]
fn set_output_mid_track_continues_from_the_position() {
    let (_player, _events, mut r, mut e) = parts();
    r.configure(48_000, 2);
    let dac = dac(&mut e, |_| true);
    e.command(PlayerCmd::Load(fixture(1)));
    render(&mut e, &mut r, 20); // 200 ms
    let before = e.shared.position();
    e.command(PlayerCmd::SetOutput(exclusive()));
    assert_eq!(dac.asked(), [S16_44100]);
    let (mut r, _) = dac.opened.try_recv().unwrap();
    render(&mut e, &mut r, 2); // takes the flush, then plays on
    let after = e.shared.position();
    let moved = after.abs_diff(before);
    assert!(
        moved <= Duration::from_millis(20),
        "{before:?} -> {after:?}"
    );
}

/// Another device chosen while stopped: the next track opens it, not the one held before.
#[test]
fn a_new_device_is_opened_for_the_next_track() {
    let (_player, _events, _r, mut e) = parts();
    let dac = dac(&mut e, |_| true);
    e.command(PlayerCmd::SetOutput(exclusive()));
    e.command(PlayerCmd::Load(fixture(1)));
    e.command(PlayerCmd::Stop);
    let other = OutputConfig {
        device: Device::Named("another DAC".into()),
        ..exclusive()
    };
    e.command(PlayerCmd::SetOutput(other));
    e.command(PlayerCmd::Load(fixture(2)));
    assert_eq!(dac.asked(), [S16_44100, S16_44100]);
}

#[test]
fn a_new_renderer_continues_the_position() {
    let (_player, _events, mut r, mut e) = parts();
    r.configure(48_000, 2);
    e.command(PlayerCmd::Load(fixture(1)));
    render(&mut e, &mut r, 10); // 100 ms
    let before = e.shared.position();
    assert!(before >= Duration::from_millis(50), "at {before:?}");
    let mut r = e.renderer();
    r.configure(44_100, 2);
    render(&mut e, &mut r, 1); // re-seeks into its ring; it takes that flush
    let after = e.shared.position();
    let moved = after.abs_diff(before);
    assert!(moved < Duration::from_millis(1), "{before:?} -> {after:?}");
}
