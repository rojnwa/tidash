#![allow(clippy::unwrap_used)]

use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use fastframe_audio::Render;
use player::PlayState::{Buffering, Paused, Playing, Stopped};
use player::PlayerEvent::{State, TrackEnded, TrackStarted};
use player::{Player, PlayerCmd, PlayerEvent, Renderer, Source};

/// 10 ms of stereo at 48 kHz.
const BLOCK: usize = 960;
const DEADLINE: Duration = Duration::from_secs(5);

fn fixture(name: &str) -> std::fs::File {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.mp4"));
    std::fs::File::open(path).unwrap()
}

fn source(id: u64, name: &str) -> Source {
    let file = fixture(name);
    let len = file.metadata().unwrap().len();
    Source {
        id,
        reader: Box::new(file),
        len: Some(len),
        gain: 1.0,
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
    render_paced(renderer, events, want, Duration::from_millis(1))
}

/// A pause between blocks that renders at ~2x real time, not ~6x as `render_until` does: for
/// tests that must not underrun (a debug build resamples barely faster than 6x, a busy machine
/// slower) or that time the player thread.
const PACE: Duration = Duration::from_millis(4);

/// `render_until`, with `pause` between the 10 ms blocks.
fn render_paced(
    renderer: &mut Renderer,
    events: &Receiver<PlayerEvent>,
    want: &PlayerEvent,
    pause: Duration,
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
        let mut block = [0.0f32; BLOCK];
        renderer.render(&mut block);
        out.extend_from_slice(&block);
        std::thread::sleep(pause);
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

/// The fixture's `.raw` reference (interleaved `i32`), scaled to `f32` as the engine does.
fn raw(name: &str) -> Vec<f32> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.raw"));
    let bytes = std::fs::read(path).unwrap();
    let (words, _) = bytes.as_chunks::<4>();
    words
        .iter()
        .map(|b| i32::from_le_bytes(*b) as f32 / 2_147_483_648.0)
        .collect()
}

/// Stereo samples without their leading and trailing all-zero frames.
fn trimmed(samples: &[f32]) -> &[f32] {
    let frames = samples.as_chunks::<2>().0;
    let sound = |f: &[f32; 2]| f.iter().any(|s| *s != 0.0);
    let start = frames.iter().position(sound).unwrap_or(frames.len());
    let end = frames.iter().rposition(sound).map_or(start, |i| i + 1);
    &samples[start * 2..end * 2]
}

fn assert_same(got: &[f32], want: &[f32]) {
    let first = got.iter().zip(want).position(|(g, w)| g != w);
    assert!(
        got.len() == want.len() && first.is_none(),
        "{} vs {} samples; first difference at {first:?}",
        got.len(),
        want.len()
    );
}

/// Configures the renderer at `rate`, loads `s16_44100` as track 1, preloads `next` and plays.
fn start_with_preload(rate: u32, next: Source) -> (Player, Receiver<PlayerEvent>, Renderer) {
    let (player, events, mut renderer) = Player::spawn_manual();
    renderer.configure(rate, 2);
    player.send(PlayerCmd::Load(source(1, "s16_44100")));
    player.send(PlayerCmd::Preload(Some(next)));
    player.send(PlayerCmd::Play);
    (player, events, renderer)
}

/// Waits (without rendering) until the position reads `target`.
fn wait_position(player: &Player, target: Duration) {
    let deadline = Instant::now() + DEADLINE;
    while player.position() != target {
        assert!(Instant::now() < deadline, "at {:?}", player.position());
        std::thread::sleep(Duration::from_millis(1));
    }
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

#[test]
fn back_to_back_seeks_keep_the_last() {
    let (player, events, mut r) = start("s16_44100");
    render_until(&mut r, &events, &State(Playing));
    // Two flush requests before the renderer runs again.
    player.send(PlayerCmd::Seek(Duration::from_millis(100)));
    player.send(PlayerCmd::Seek(Duration::from_millis(300)));
    let deadline = Instant::now() + DEADLINE;
    while player.position() < Duration::from_millis(300) {
        assert!(Instant::now() < deadline, "at {:?}", player.position());
        std::thread::sleep(Duration::from_millis(1));
    }
    render_ms(&mut r, 10);
    assert!(
        player.position() >= Duration::from_millis(300),
        "at {:?}",
        player.position()
    );
    let (out, _) = render_until(&mut r, &events, &TrackEnded(1));
    let frames = nonsilent_frames(&out);
    assert!((9_504..=9_696).contains(&frames), "frames = {frames}"); // 200 ms ± 1%
}

/// Serves a fixture until `left` bytes were read, then fails (`stall: false`) or never returns.
struct Cut {
    file: std::fs::File,
    left: usize,
    stall: bool,
}

impl Cut {
    fn source(id: u64, name: &str, left: usize, stall: bool) -> Source {
        let file = fixture(name);
        Source {
            id,
            reader: Box::new(Cut { file, left, stall }),
            len: None,
            gain: 1.0,
        }
    }
}

impl Read for Cut {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            if self.stall {
                loop {
                    std::thread::park(); // a dead connection without a timeout
                }
            }
            return Err(io::Error::new(io::ErrorKind::ConnectionReset, "boom"));
        }
        let n = buf.len().min(self.left);
        let n = self.file.read(&mut buf[..n])?;
        self.left -= n;
        Ok(n)
    }
}

/// Stop within 500 ms, with no Error, and the next Load plays.
fn stop_promptly(player: &Player, events: &Receiver<PlayerEvent>) {
    let t = Instant::now();
    player.send(PlayerCmd::Stop);
    let seen = wait_for(events, &State(Stopped));
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "took {:?}",
        t.elapsed()
    );
    assert!(
        !seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })),
        "{seen:?}"
    );
}

#[test]
fn stop_is_not_stuck_behind_a_stalled_stream() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    player.send(PlayerCmd::Load(Cut::source(1, "s16_44100", 4096, true)));
    player.send(PlayerCmd::Play);
    render_ms(&mut r, 100); // the player thread now waits for bytes that never come
    stop_promptly(&player, &events);
    player.send(PlayerCmd::Load(source(2, "s16_44100")));
    player.send(PlayerCmd::Play);
    render_until(&mut r, &events, &TrackStarted(2));
}

#[test]
fn load_then_stop_back_to_back() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    player.send(PlayerCmd::Load(Cut::source(1, "s16_44100", 4096, true)));
    stop_promptly(&player, &events); // no render, no sleep in between
    player.send(PlayerCmd::Load(source(2, "s16_44100")));
    player.send(PlayerCmd::Play);
    render_until(&mut r, &events, &TrackStarted(2));
}

/// Delivers a fixture 2 KiB at a time, 10 ms apart; flags when its last byte went out.
struct Trickle {
    file: std::fs::File,
    left: u64,
    last: Arc<AtomicBool>,
}

impl Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        std::thread::sleep(Duration::from_millis(10));
        let n = buf.len().min(2048);
        let n = self.file.read(&mut buf[..n])?;
        self.left = self.left.saturating_sub(n as u64);
        if self.left == 0 {
            self.last.store(true, Ordering::SeqCst);
        }
        Ok(n)
    }
}

#[test]
fn starts_before_download_completes() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    let file = fixture("s24_96000"); // 60 KB: about 300 ms to trickle in
    let left = file.metadata().unwrap().len();
    let last = Arc::new(AtomicBool::new(false));
    player.send(PlayerCmd::Load(Source {
        id: 1,
        reader: Box::new(Trickle {
            file,
            left,
            last: Arc::clone(&last),
        }),
        len: None, // as with DASH
        gain: 1.0,
    }));
    player.send(PlayerCmd::Play);
    render_until(&mut r, &events, &TrackStarted(1));
    assert!(
        !last.load(Ordering::SeqCst),
        "started only after the whole download"
    );
}

/// Serves the first `first` bytes of a fixture, then the rest once the test opens the gate.
struct Gated {
    file: std::fs::File,
    first: usize,
    gate: Option<std::sync::mpsc::Receiver<()>>,
}

impl Read for Gated {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.first == 0
            && let Some(gate) = self.gate.take()
        {
            let _ = gate.recv();
        }
        let n = if self.first > 0 {
            buf.len().min(self.first)
        } else {
            buf.len()
        };
        let n = self.file.read(&mut buf[..n])?;
        self.first = self.first.saturating_sub(n);
        Ok(n)
    }
}

#[test]
fn second_forward_seek_after_interrupted_seek() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    let (open, gate) = std::sync::mpsc::channel();
    let file = fixture("s24_96000");
    let first = file.metadata().unwrap().len() as usize * 2 / 5; // ~200 ms of 500
    player.send(PlayerCmd::Load(Source {
        id: 1,
        reader: Box::new(Gated {
            file,
            first,
            gate: Some(gate),
        }),
        len: None,
        gain: 1.0,
    }));
    player.send(PlayerCmd::Play);
    let (_, mut seen) = render_until(&mut r, &events, &State(Playing));
    render_ms(&mut r, 50);
    player.send(PlayerCmd::Seek(Duration::from_millis(450))); // waits for bytes
    std::thread::sleep(Duration::from_millis(50));
    let target = Duration::from_millis(300);
    player.send(PlayerCmd::Seek(target)); // interrupts it
    std::thread::sleep(Duration::from_millis(50));
    open.send(()).unwrap();
    let deadline = Instant::now() + DEADLINE;
    while player.position() != target {
        seen.extend(events.try_iter());
        let at = player.position();
        assert!(Instant::now() < deadline, "at {at:?}; saw {seen:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
    let (out, rest) = render_until(&mut r, &events, &TrackEnded(1));
    seen.extend(rest);
    assert!(
        !seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })),
        "{seen:?}"
    );
    let frames = nonsilent_frames(&out);
    assert!((9_504..=9_696).contains(&frames), "frames = {frames}"); // 200 ms ± 1%
}

#[test]
fn pause_and_volume_act_while_the_player_waits_on_a_read() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    let (_open, gate) = std::sync::mpsc::channel();
    let file = fixture("s24_96000");
    let first = file.metadata().unwrap().len() as usize * 2 / 5; // ~200 ms of 500, then it waits
    player.send(PlayerCmd::Load(Source {
        id: 1,
        reader: Box::new(Gated {
            file,
            first,
            gate: Some(gate),
        }),
        len: None,
        gain: 1.0,
    }));
    player.send(PlayerCmd::Play);
    render_until(&mut r, &events, &State(Playing));
    std::thread::sleep(Duration::from_millis(100)); // decodes what arrived, then waits for more
    player.send(PlayerCmd::SetVolume(0.0));
    assert_eq!(nonsilent_frames(&render_ms(&mut r, 20)), 0, "volume waited");
    player.send(PlayerCmd::SetVolume(1.0));
    assert!(nonsilent_frames(&render_ms(&mut r, 20)) > 0, "ring ran dry");
    player.send(PlayerCmd::Pause);
    let at = player.position();
    assert_eq!(nonsilent_frames(&render_ms(&mut r, 20)), 0, "pause waited");
    assert_eq!(player.position(), at);
}

#[test]
fn seek_backwards_after_progress() {
    let (player, events, mut r) = start("s16_44100");
    render_until(&mut r, &events, &State(Playing));
    render_ms(&mut r, 300);
    let target = Duration::from_millis(100);
    player.send(PlayerCmd::Seek(target));
    let deadline = Instant::now() + DEADLINE;
    while player.position() != target {
        assert!(Instant::now() < deadline, "at {:?}", player.position());
        std::thread::sleep(Duration::from_millis(1));
    }
    let (out, _) = render_until(&mut r, &events, &TrackEnded(1));
    let frames = nonsilent_frames(&out);
    assert!((19_008..=19_392).contains(&frames), "frames = {frames}"); // 400 ms ± 1%
}

#[test]
fn seek_and_stop_are_not_stuck_behind_a_stall_mid_track() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    // About half the file, length known (else the probe waits for all of it): playing starts,
    // then decoding waits for bytes that never come.
    player.send(PlayerCmd::Load(Source {
        len: Some(fixture("s24_96000").metadata().unwrap().len()),
        ..Cut::source(1, "s24_96000", 32 * 1024, true)
    }));
    player.send(PlayerCmd::Play);
    render_until(&mut r, &events, &State(Playing));
    // Play out what arrived (~270 ms): once the ring runs dry, the player thread is waiting.
    let t = Instant::now();
    let mut heard = 0;
    loop {
        let n = nonsilent_frames(&render_ms(&mut r, 10));
        heard += n;
        if heard >= 9_600 && n == 0 {
            break;
        }
        assert!(t.elapsed() < DEADLINE, "heard {heard} frames");
    }
    let target = Duration::from_millis(123);
    let t = Instant::now();
    player.send(PlayerCmd::Seek(target));
    while player.position() != target {
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "seek stuck; at {:?}",
            player.position()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    while nonsilent_frames(&render_ms(&mut r, 10)) == 0 {
        assert!(t.elapsed() < DEADLINE, "no audio after the seek");
    }
    stop_promptly(&player, &events);
}

#[test]
fn stream_error_emits_error() {
    let (player, events, mut r) = Player::spawn_manual();
    r.configure(48_000, 2);
    let t = Instant::now();
    player.send(PlayerCmd::Load(Cut::source(1, "s16_44100", 4096, false)));
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

/// Plays `s16_44100` twice (ids 1, 2) at `rate`, gaplessly: what was rendered until the stop,
/// and the events. Compared sample for sample, the output must not underrun: the ring fills
/// before rendering goes on, at `PACE`.
fn play_gapless(rate: u32) -> (Vec<f32>, Vec<PlayerEvent>) {
    let (_player, events, mut r) = start_with_preload(rate, source(2, "s16_44100"));
    let (mut out, mut seen) = render_until(&mut r, &events, &TrackStarted(1));
    std::thread::sleep(Duration::from_millis(200));
    let (rest, more) = render_paced(&mut r, &events, &State(Stopped), PACE);
    out.extend(rest);
    seen.extend(more);
    (out, seen)
}

#[test]
fn gapless_is_sample_exact() {
    let (out, seen) = play_gapless(44_100);
    assert_in_order(&seen, &[TrackStarted(1), TrackStarted(2), TrackEnded(2)]);
    assert!(!seen.contains(&TrackEnded(1)), "{seen:?}");
    let want = [raw("s16_44100"), raw("s16_44100")].concat();
    assert_same(trimmed(&out), trimmed(&want));
}

#[test]
fn gapless_resampled_matches_one_resample() {
    let (out, _) = play_gapless(48_000);
    let mut want = Vec::new();
    let mut resampler = player::Resampler::new(44_100, 48_000, 2);
    resampler.process(&[raw("s16_44100"), raw("s16_44100")].concat(), &mut want);
    resampler.flush(&mut want);
    assert_same(trimmed(&out), trimmed(&want));
}

#[test]
fn position_restarts_at_the_boundary() {
    let (player, events, mut r) = start_with_preload(48_000, source(2, "s16_44100"));
    render_paced(&mut r, &events, &TrackStarted(2), PACE);
    let at = player.position();
    assert!(at <= Duration::from_millis(100), "started at {at:?}");
    render_paced(&mut r, &events, &TrackEnded(2), PACE);
    let at = player.position();
    let diff = at.abs_diff(Duration::from_millis(500));
    assert!(diff <= Duration::from_millis(10), "ended at {at:?}");
}

#[test]
fn a_late_preload_still_follows() {
    let (player, events, mut r) = start("s16_44100");
    let mut out = render_ms(&mut r, 100); // A is fully decoded by then
    player.send(PlayerCmd::Preload(Some(source(2, "s16_44100"))));
    let (rest, seen) = render_until(&mut r, &events, &State(Stopped));
    out.extend(rest);
    assert!(seen.contains(&TrackStarted(2)), "{seen:?}");
    assert!(!seen.contains(&TrackEnded(1)), "{seen:?}");
    let frames = nonsilent_frames(&out);
    assert!((47_520..=48_480).contains(&frames), "frames = {frames}"); // 1 s ± 1%
}

#[test]
fn seek_before_the_boundary_stays_in_the_ending_track() {
    let (player, events, mut r) = start_with_preload(48_000, source(2, "s16_44100"));
    render_ms(&mut r, 50); // the switch to B is decoded, not heard
    let target = Duration::from_millis(100);
    player.send(PlayerCmd::Seek(target));
    wait_position(&player, target);
    let (out, seen) = render_until(&mut r, &events, &State(Stopped));
    assert!(seen.contains(&TrackStarted(2)), "{seen:?}");
    let frames = nonsilent_frames(&out);
    assert!((42_768..=43_632).contains(&frames), "frames = {frames}"); // 900 ms ± 1%
}

#[test]
fn cancelled_preload_is_not_played() {
    let (player, events, mut r) = start("s16_44100");
    player.send(PlayerCmd::Preload(Some(source(2, "s16_44100"))));
    player.send(PlayerCmd::Preload(None));
    let (_, seen) = render_until(&mut r, &events, &State(Stopped));
    assert!(seen.contains(&TrackEnded(1)), "{seen:?}");
    assert!(!seen.contains(&TrackStarted(2)), "{seen:?}");
}

#[test]
fn preload_after_the_end_is_dropped() {
    let (player, events, mut r) = start("s16_44100");
    render_until(&mut r, &events, &State(Stopped));
    player.send(PlayerCmd::Preload(Some(source(2, "s16_44100"))));
    let until = Instant::now() + Duration::from_millis(200);
    let mut seen = Vec::new();
    while Instant::now() < until {
        render_ms(&mut r, 10);
        seen.extend(events.try_iter());
    }
    assert!(!seen.contains(&TrackStarted(2)), "{seen:?}");
}

#[test]
fn a_preload_that_fails_to_open_ends_the_track_normally() {
    let broken = Source {
        id: 2,
        reader: Box::new(io::empty()),
        len: None,
        gain: 1.0,
    };
    let (_player, events, mut r) = start_with_preload(48_000, broken);
    let (_, seen) = render_until(&mut r, &events, &State(Stopped));
    assert!(seen.contains(&TrackEnded(1)), "{seen:?}");
    assert!(
        !seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })),
        "{seen:?}"
    );
}

#[test]
fn a_mono_track_follows_after_a_flush() {
    let (_player, events, mut r) = start_with_preload(48_000, source(2, "s16_44100_mono"));
    let (head, mut seen) = render_until(&mut r, &events, &TrackStarted(2));
    let (tail, rest) = render_until(&mut r, &events, &State(Stopped));
    seen.extend(rest);
    assert!(!seen.contains(&TrackEnded(1)), "{seen:?}");
    // B's frames: those after the silent block of the flush between the tracks, and on.
    let gap = head
        .chunks(BLOCK)
        .rposition(|b| nonsilent_frames(b) == 0)
        .unwrap();
    let frames = nonsilent_frames(&head[(gap + 1) * BLOCK..]) + nonsilent_frames(&tail);
    assert!((23_760..=24_240).contains(&frames), "frames = {frames}"); // 500 ms ± 1%
}

#[test]
fn gain_scales_output() {
    let run = |gain: f32| {
        let (player, events, mut r) = Player::spawn_manual();
        r.configure(48_000, 2);
        player.send(PlayerCmd::Load(Source {
            gain,
            ..source(1, "s16_44100")
        }));
        player.send(PlayerCmd::Play);
        peak(&render_until(&mut r, &events, &TrackEnded(1)).0)
    };
    let (full, half) = (run(1.0), run(0.5));
    assert!(full > 0.05, "full = {full}");
    assert!((half / full - 0.5).abs() <= 0.01, "{half} / {full}");
}
