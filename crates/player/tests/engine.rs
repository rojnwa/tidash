#![allow(clippy::unwrap_used)]

use std::io::{self, Read};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use fastframe_audio::Render;
use player::PlayState::{Buffering, Paused, Playing, Stopped};
use player::PlayerEvent::{State, TrackEnded, TrackStarted};
use player::{Player, PlayerCmd, PlayerEvent, Renderer, Source};

/// 10 ms of stereo at 48 kHz.
const BLOCK: usize = 960;
const DEADLINE: Duration = Duration::from_secs(5);

fn source(id: u64, name: &str) -> Source {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.mp4"));
    let file = std::fs::File::open(path).unwrap();
    let len = file.metadata().unwrap().len();
    Source {
        id,
        reader: Box::new(file),
        len: Some(len),
    }
}

fn start(name: &str) -> (Player, Receiver<PlayerEvent>, Renderer) {
    let (player, events, mut renderer) = Player::spawn_manual();
    renderer.configure(48_000, 2);
    player.send(PlayerCmd::Load(source(1, name)));
    player.send(PlayerCmd::Play);
    (player, events, renderer)
}

/// Renders `ms` in 10 ms blocks, pausing briefly between them so the player thread can refill.
fn render_ms(renderer: &mut Renderer, ms: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..ms.div_ceil(10) {
        let mut block = [0.0f32; BLOCK];
        renderer.render(&mut block);
        out.extend_from_slice(&block);
        std::thread::sleep(Duration::from_millis(1));
    }
    out
}

/// Renders until an event equal to `want` arrives; returns what was rendered and every event seen.
fn render_until(
    renderer: &mut Renderer,
    events: &Receiver<PlayerEvent>,
    want: &PlayerEvent,
) -> (Vec<f32>, Vec<PlayerEvent>) {
    let deadline = Instant::now() + DEADLINE;
    let (mut out, mut seen) = (Vec::new(), Vec::new());
    loop {
        while let Ok(e) = events.try_recv() {
            let hit = e == *want;
            seen.push(e);
            if hit {
                return (out, seen);
            }
        }
        assert!(Instant::now() < deadline, "no {want:?}; saw {seen:?}");
        out.extend(render_ms(renderer, 10));
    }
}

/// Waits (without rendering) for an event equal to `want`.
fn wait_for(events: &Receiver<PlayerEvent>, want: &PlayerEvent) -> Vec<PlayerEvent> {
    let deadline = Instant::now() + DEADLINE;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(e) if e == *want => {
                seen.push(e);
                return seen;
            }
            Ok(e) => seen.push(e),
            Err(_) => panic!("no {want:?}; saw {seen:?}"),
        }
    }
}

/// `want` appears in order in `seen`, and no state is reported twice in a row.
fn assert_in_order(seen: &[PlayerEvent], want: &[PlayerEvent]) {
    let mut it = seen.iter();
    for w in want {
        assert!(it.any(|e| e == w), "{want:?} not in order in {seen:?}");
    }
    let states: Vec<_> = seen.iter().filter(|e| matches!(e, State(_))).collect();
    assert!(
        states.windows(2).all(|w| w[0] != w[1]),
        "duplicate state in {seen:?}"
    );
}

fn nonsilent_frames(samples: &[f32]) -> usize {
    samples
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|f| f.iter().any(|s| *s != 0.0))
        .count()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn plays_track_to_end() {
    let (_player, events, mut r) = start("s16_44100");
    let (out, seen) = render_until(&mut r, &events, &TrackEnded(1));
    assert_in_order(
        &seen,
        &[
            State(Buffering),
            TrackStarted(1),
            State(Playing),
            TrackEnded(1),
        ],
    );
    let frames = nonsilent_frames(&out);
    assert!((23_760..=24_240).contains(&frames), "frames = {frames}");
    wait_for(&events, &State(Stopped));
}

#[test]
fn volume_scales_output() {
    let run = |volume: Option<f32>| {
        let (player, events, mut r) = Player::spawn_manual();
        r.configure(48_000, 2);
        if let Some(v) = volume {
            player.send(PlayerCmd::SetVolume(v));
        }
        player.send(PlayerCmd::Load(source(1, "s16_44100")));
        player.send(PlayerCmd::Play);
        peak(&render_until(&mut r, &events, &TrackEnded(1)).0)
    };
    let (full, half) = (run(None), run(Some(0.5)));
    assert!(full > 0.05, "full = {full}");
    assert!((half / full - 0.5).abs() <= 0.01, "{half} / {full}");
}

#[test]
fn pause_stops_and_play_resumes() {
    let (player, events, mut r) = start("s16_44100");
    render_until(&mut r, &events, &State(Playing));
    render_ms(&mut r, 50);
    player.send(PlayerCmd::Pause);
    wait_for(&events, &State(Paused));
    let before = player.position();
    let out = render_ms(&mut r, 100);
    assert_eq!(nonsilent_frames(&out), 0);
    assert_eq!(player.position(), before);
    player.send(PlayerCmd::Play);
    wait_for(&events, &State(Playing));
    assert!(nonsilent_frames(&render_ms(&mut r, 50)) > 0);
}

#[test]
fn seek_while_paused_has_no_stale_audio() {
    let (player, events, mut r) = start("s16_44100");
    render_until(&mut r, &events, &State(Playing));
    render_ms(&mut r, 50);
    player.send(PlayerCmd::Pause);
    wait_for(&events, &State(Paused));
    player.send(PlayerCmd::Seek(Duration::from_millis(400)));
    let deadline = Instant::now() + DEADLINE;
    while player.position() < Duration::from_millis(400) {
        assert!(Instant::now() < deadline, "at {:?}", player.position());
        std::thread::sleep(Duration::from_millis(1));
    }
    player.send(PlayerCmd::Play);
    let (out, _) = render_until(&mut r, &events, &TrackEnded(1));
    let frames = nonsilent_frames(&out);
    assert!((4_320..=5_280).contains(&frames), "frames = {frames}");
    // Tighter: the seek lands on a packet start before 400 ms; the engine trims up to it.
    assert!(
        (4_752..=4_848).contains(&frames),
        "untrimmed: frames = {frames}"
    );
}

#[test]
fn seek_past_end_ends_track() {
    let (player, events, mut r) = start("s16_44100");
    render_until(&mut r, &events, &State(Playing));
    player.send(PlayerCmd::Seek(Duration::from_secs(10)));
    let (out, seen) = render_until(&mut r, &events, &TrackEnded(1));
    assert!(
        out.len() <= 50 * BLOCK,
        "rendered {} blocks",
        out.len() / BLOCK
    );
    assert!(!seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })));
    wait_for(&events, &State(Stopped));
}

#[test]
fn device_rate_change_mid_track() {
    let (player, events, mut r) = start("s16_44100");
    render_until(&mut r, &events, &State(Playing));
    render_ms(&mut r, 100);
    r.configure(44_100, 2);
    let deadline = Instant::now() + DEADLINE;
    let mut last = player.position();
    'outer: loop {
        while let Ok(e) = events.try_recv() {
            if e == TrackEnded(1) {
                break 'outer;
            }
        }
        assert!(Instant::now() < deadline, "no end; at {last:?}");
        render_ms(&mut r, 10);
        last = player.position();
    }
    let diff = last.abs_diff(Duration::from_millis(500));
    assert!(diff <= Duration::from_millis(10), "ended at {last:?}");
}

/// Serves a file, then fails once 4 KiB were read.
struct FailAfter(std::fs::File, usize);

impl Read for FailAfter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.1 == 0 {
            return Err(io::Error::new(io::ErrorKind::ConnectionReset, "boom"));
        }
        let n = buf.len().min(self.1);
        let n = self.0.read(&mut buf[..n])?;
        self.1 -= n;
        Ok(n)
    }
}

#[test]
fn stream_error_emits_error() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/s16_44100.mp4");
    let reader = FailAfter(std::fs::File::open(path).unwrap(), 4096);
    let t = Instant::now();
    player.send(PlayerCmd::Load(Source {
        id: 1,
        reader: Box::new(reader),
        len: None,
    }));
    player.send(PlayerCmd::Play);
    let (_, seen) = render_until(&mut r, &events, &State(Stopped));
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "took {:?}",
        t.elapsed()
    );
    let err = seen
        .iter()
        .position(|e| matches!(e, PlayerEvent::Error { id: 1, .. }))
        .unwrap_or_else(|| panic!("no error in {seen:?}"));
    assert_eq!(seen.last(), Some(&State(Stopped)));
    assert!(err < seen.len() - 1);
}
