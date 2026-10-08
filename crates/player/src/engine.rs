//! The player thread: decoder -> resampler -> ring, commands in, events out.

use std::io::{self, Read};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use fastframe_audio::{Maintained, OpenError, Output, OutputOptions};
use rtrb::Producer;

use crate::render::{NewRing, Renderer, Shared};
use crate::stream::is_interrupt;
use crate::{DecodeError, Decoder, Interrupt, Pcm, ProgressiveFile, Resampler};

/// How long the thread sleeps on the command channel when there is nothing to decode.
const IDLE_WAIT: Duration = Duration::from_millis(50);
/// The same, while a running output is about to take a flush (start, seek): resume quickly.
const FLUSH_WAIT: Duration = Duration::from_millis(5);
/// Callbacks this recent mean the device is running (and will take a pending flush soon).
const CALLBACK_RECENT: Duration = Duration::from_millis(100);
/// Longest the device keeps running after a stop, waiting for the renderer to take the flush.
const STOP_GRACE: Duration = Duration::from_millis(50);
const MAINTAIN_EVERY: Duration = Duration::from_secs(1);

/// A track to play: a forward-only byte stream and, if known, its length.
pub struct Source {
    pub id: u64,
    pub reader: Box<dyn Read + Send>,
    pub len: Option<u64>,
}

pub enum PlayerCmd {
    Load(Source),
    Play,
    Pause,
    Stop,
    Seek(Duration),
    SetVolume(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Buffering,
    Playing,
    Paused,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerEvent {
    /// Sent only when the state actually changes.
    State(PlayState),
    TrackStarted(u64),
    TrackEnded(u64),
    Error {
        id: u64,
        message: String,
    },
    /// The output could not be opened again; sent on the first failed reopen, not on the retries
    /// after it. The output keeps trying, and a loaded track plays on once it is back.
    OutputLost,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Handle to the player thread. Cheap to clone; dropping the last clone disconnects the
/// command channel, and the thread (and the device output it owns) then exits.
#[derive(Clone)]
pub struct Player {
    /// Each command with whether it raised `interrupt`.
    cmds: Sender<(PlayerCmd, bool)>,
    shared: Arc<Shared>,
    interrupt: Interrupt,
    /// Loads sent whose probe has not finished. Also orders `send`s (see there).
    loads: Arc<Mutex<u32>>,
}

impl Player {
    /// Opens the audio output (on the player thread; it can take a moment). Fails when the
    /// device cannot be opened. The output is paused until a track is loaded.
    pub fn spawn(options: OutputOptions) -> io::Result<(Player, Receiver<PlayerEvent>)> {
        let (player, events, renderer, engine) = parts();
        let (ready, opened) = mpsc::channel();
        std::thread::Builder::new()
            .name("player".into())
            .spawn(move || match Output::open(options, renderer) {
                Ok(output) => {
                    let _ = ready.send(Ok(()));
                    Engine {
                        output: Some(output),
                        ..engine
                    }
                    .run();
                }
                Err(e) => {
                    let _ = ready.send(Err(io::Error::other(e)));
                }
            })?;
        opened
            .recv()
            .map_err(|_| io::Error::other("player thread exited"))??;
        Ok((player, events))
    }

    /// No device: the caller drives the returned renderer (tests).
    pub fn spawn_manual() -> (Player, Receiver<PlayerEvent>, Renderer) {
        let (player, events, renderer, engine) = parts();
        std::thread::spawn(move || engine.run());
        (player, events, renderer)
    }

    pub fn send(&self, cmd: PlayerCmd) {
        // Pause, Play and volume reach the renderer at once, even while the thread waits on a
        // read; the thread applies them again in order and reports the state.
        match cmd {
            PlayerCmd::Play => self.shared.paused.store(false, Release),
            PlayerCmd::Pause => self.shared.paused.store(true, Release),
            PlayerCmd::SetVolume(v) => self.shared.volume.store(v.to_bits(), Relaxed),
            _ => {}
        }
        // Load, Stop and Seek must not queue behind a read waiting on the network, whichever
        // source it is reading (even one the thread has yet to open): raise the interrupt. A
        // Seek leaves unfinished Loads alone; it is meant for that track. The lock keeps the
        // decision, the interrupt and the queue order together.
        let mut loads = lock(&self.loads);
        let interrupt = match cmd {
            PlayerCmd::Load(_) => {
                *loads += 1;
                true
            }
            PlayerCmd::Stop => true,
            PlayerCmd::Seek(_) => *loads == 0,
            _ => false,
        };
        if interrupt {
            self.interrupt.interrupt();
        }
        // The thread only exits once every `Player` is gone, so this cannot fail.
        let _ = self.cmds.send((cmd, interrupt));
    }

    pub fn position(&self) -> Duration {
        self.shared.position()
    }
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player").finish_non_exhaustive()
    }
}

fn parts() -> (Player, Receiver<PlayerEvent>, Renderer, Engine) {
    let shared = Arc::new(Shared::default());
    let interrupt = Interrupt::default();
    let loads = Arc::new(Mutex::new(0));
    let (cmds, cmd_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let (rings, ring_rx) = mpsc::channel();
    let engine = Engine {
        shared: Arc::clone(&shared),
        interrupt: interrupt.clone(),
        loads: Arc::clone(&loads),
        cmds: cmd_rx,
        events,
        rings: ring_rx,
        ring: None,
        rate: 0,
        track: None,
        state: PlayState::Stopped,
        scratch: Vec::new(),
        output: None,
        output_lost: false,
        pause_by: None,
        callbacks_seen: 0,
        callback_at: Instant::now(),
    };
    let renderer = Renderer::new(Arc::clone(&shared), rings);
    let player = Player {
        cmds,
        shared,
        interrupt,
        loads,
    };
    (player, event_rx, renderer, engine)
}

/// Track rate -> device rate. Before the first `configure` there is no device rate:
/// passthrough until `adopt_ring` rebuilds it.
fn resampler(decoder: &Decoder, device_rate: u32) -> Resampler {
    let to = if device_rate == 0 {
        decoder.rate()
    } else {
        device_rate
    };
    Resampler::new(decoder.rate(), to, decoder.channels())
}

/// An error caused by a Load, Stop or Seek arriving (a read given up): not reported, that
/// command decides what happens next. symphonia's probe turns a failed read into "no format
/// found", so any error while an interrupt is pending counts; a real one comes back after.
fn superseded(e: &DecodeError, interrupt: &Interrupt) -> bool {
    matches!(e, DecodeError::Io(e) if is_interrupt(e)) || interrupt.raised()
}

struct Track {
    id: u64,
    /// The download; the decoder reads a forward-only view of it.
    file: ProgressiveFile,
    decoder: Decoder,
    /// Source frames the decoder has gone through: where its next packet starts.
    decoded: u64,
    /// A decoder call failed (e.g. a read was interrupted): symphonia may be left mid-atom
    /// (its `next_header` drops `skip_atom`'s error), so it is rebuilt before any further use.
    stale: bool,
    resampler: Resampler,
    channels: usize,
    /// Source frames still to drop after a seek (it lands at or before the target).
    skip: usize,
    /// Resampled samples that did not fit in the ring yet.
    pending: Vec<f32>,
    eof: bool,
    started: bool,
}

impl Track {
    /// The decoder only reads forward: going back (or on from a stale decoder) starts a new
    /// one at byte 0 (downloaded already, so local) and seeks forward. Returns where it landed.
    fn seek(&mut self, to: Duration) -> Result<Duration, DecodeError> {
        let rate = f64::from(self.decoder.rate());
        if self.stale || to.as_secs_f64() * rate < self.decoded as f64 {
            self.decoder = Decoder::new(Box::new(self.file.forward()?))?;
            (self.decoded, self.stale) = (0, false);
        }
        let at = self.decoder.seek(to).inspect_err(|_| self.stale = true)?;
        self.decoded = (at.as_secs_f64() * rate).round() as u64;
        Ok(at)
    }

    /// The next packet. A stale decoder is first rebuilt back at `decoded`; the frames it
    /// decodes again up to there are skipped.
    fn next(&mut self) -> Result<Option<Pcm>, DecodeError> {
        if self.stale {
            let was = self.decoded;
            self.seek(Duration::from_secs_f64(
                was as f64 / f64::from(self.decoder.rate()),
            ))?;
            self.skip += usize::try_from(was.saturating_sub(self.decoded)).unwrap_or(usize::MAX);
        }
        let pcm = self.decoder.next().inspect_err(|_| self.stale = true)?;
        if let Some(p) = &pcm {
            self.decoded += (p.samples.len() / self.channels) as u64;
        }
        Ok(pcm)
    }
}

struct Engine {
    shared: Arc<Shared>,
    interrupt: Interrupt,
    loads: Arc<Mutex<u32>>,
    cmds: Receiver<(PlayerCmd, bool)>,
    events: Sender<PlayerEvent>,
    rings: Receiver<NewRing>,
    ring: Option<Producer<f32>>,
    /// Device rate the ring and resampler are built for (0 before the first `configure`).
    rate: u32,
    track: Option<Track>,
    state: PlayState,
    scratch: Vec<f32>,
    output: Option<Output<Renderer>>,
    output_lost: bool,
    /// Set by a stop: the latest moment to pause the device (see `output_action`).
    pause_by: Option<Instant>,
    callbacks_seen: u64,
    callback_at: Instant,
}

impl Engine {
    fn run(mut self) {
        let mut maintained = Instant::now();
        let mut idle = false;
        loop {
            self.sync_output();
            let wait = if idle {
                self.idle_wait()
            } else {
                Duration::ZERO // a try_recv: commands first
            };
            match self.cmds.recv_timeout(wait) {
                Ok((cmd, raised)) => {
                    if raised {
                        self.interrupt.handled();
                    }
                    self.command(cmd);
                    idle = false;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if maintained.elapsed() >= MAINTAIN_EVERY {
                maintained = Instant::now();
                self.maintain();
            }
            self.adopt_ring();
            idle = !self.step();
        }
    }

    /// Poll fast only while a flush is about to be taken: the output should run and has
    /// called back lately. A device that is gone (no callbacks) gets the normal wait.
    fn idle_wait(&mut self) -> Duration {
        let n = self.shared.callbacks.load(Relaxed);
        if n != self.callbacks_seen {
            self.callbacks_seen = n;
            self.callback_at = Instant::now();
        }
        if self.shared.flush_pending()
            && self.output_wanted()
            && self.callback_at.elapsed() < CALLBACK_RECENT
        {
            FLUSH_WAIT
        } else {
            IDLE_WAIT
        }
    }

    /// A track is loaded and the user has not paused.
    fn output_wanted(&self) -> bool {
        self.track.is_some() && !self.shared.paused.load(Acquire)
    }

    /// What the device should do: run (`Some(true)`) while wanted, pause (`Some(false)`)
    /// otherwise — so stopped (also right after opening) it makes no callbacks. On a stop it
    /// first keeps running (`None`) until the renderer took the stop flush, at most
    /// `STOP_GRACE`, so no period of the old track stays in the device buffer to play at the
    /// next resume.
    fn output_action(&self) -> Option<bool> {
        if self.output_wanted() {
            return Some(true);
        }
        let grace =
            self.shared.flush_pending() && self.pause_by.is_some_and(|by| Instant::now() < by);
        (!grace).then_some(false)
    }

    fn sync_output(&mut self) {
        let action = self.output_action();
        let Some(out) = &mut self.output else { return };
        match action {
            Some(true) => {
                if out.is_paused() {
                    self.callback_at = Instant::now(); // expect callbacks from now on
                }
                out.resume();
            }
            Some(false) => out.pause(),
            None => {}
        }
    }

    fn command(&mut self, cmd: PlayerCmd) {
        match cmd {
            PlayerCmd::Load(src) => self.load(src),
            PlayerCmd::Play => {
                self.shared.paused.store(false, Release);
                if self.state == PlayState::Paused {
                    self.set_state(PlayState::Playing);
                }
            }
            PlayerCmd::Pause => {
                self.shared.paused.store(true, Release);
                if self.state == PlayState::Playing {
                    self.set_state(PlayState::Paused);
                }
            }
            PlayerCmd::Stop => self.stop(Duration::ZERO),
            PlayerCmd::Seek(to) => self.seek(to),
            PlayerCmd::SetVolume(v) => self.shared.volume.store(v.to_bits(), Relaxed),
        }
    }

    fn load(&mut self, src: Source) {
        self.track = None;
        self.adopt_ring(); // so the resampler is built for the current device rate
        self.flush(Duration::ZERO); // the old track stops now, not after the probe
        self.set_state(PlayState::Buffering);
        let opened = ProgressiveFile::spawn(src.reader, src.len, &self.interrupt)
            .and_then(|file| Ok((file.forward()?, file)))
            .map_err(DecodeError::Io)
            .and_then(|(view, file)| Ok((Decoder::new(Box::new(view))?, file)));
        // This Load's probe is over: Seeks may interrupt reads again. Decide whether a failure
        // was superseded while still holding the lock, so a Seek sent after it cannot have
        // raised the interrupt (and so hide a real probe error).
        let mut loads = lock(&self.loads);
        *loads = loads.saturating_sub(1);
        let opened = opened.map_err(|e| (superseded(&e, &self.interrupt), e));
        drop(loads);
        match opened {
            Ok((decoder, file)) => {
                self.track = Some(Track {
                    id: src.id,
                    resampler: resampler(&decoder, self.rate),
                    channels: usize::from(decoder.channels().max(1)),
                    file,
                    decoder,
                    decoded: 0,
                    stale: false,
                    skip: 0,
                    pending: Vec::new(),
                    eof: false,
                    started: false,
                });
                self.flush(Duration::ZERO); // again, now with the track's channels
            }
            Err((true, _)) => {}
            Err((false, e)) => self.fail(src.id, e.to_string()),
        }
    }

    fn seek(&mut self, to: Duration) {
        let Some(t) = &mut self.track else { return };
        match t.seek(to) {
            Ok(at) => {
                let skip = to.saturating_sub(at).as_secs_f64() * f64::from(t.decoder.rate());
                t.skip = skip.round() as usize;
                t.pending.clear();
                t.eof = false;
                t.resampler = resampler(&t.decoder, self.rate);
                self.flush(to);
            }
            Err(e) if superseded(&e, &self.interrupt) => {} // the next command re-seeks or drops the track
            Err(DecodeError::Io(e)) => {
                let id = t.id;
                self.fail(id, e.to_string());
            }
            Err(_) => {
                // symphonia knows no fMP4 duration up front: a refused seek is past the end.
                let id = t.id;
                self.emit(PlayerEvent::TrackEnded(id));
                self.stop(Duration::ZERO);
            }
        }
    }

    /// Discard the ring; the renderer restarts the clock at `at`.
    fn flush(&self, at: Duration) {
        let s = &self.shared;
        s.pending_base_ns
            .store(u64::try_from(at.as_nanos()).unwrap_or(u64::MAX), Relaxed);
        if let Some(t) = &self.track {
            s.channels.store(t.channels as u32, Relaxed);
        }
        s.flush_req.fetch_add(1, Release);
    }

    /// Drop the track and flush (the clock then reads `at`); the device pauses once the
    /// renderer took that flush (`output_action`).
    fn stop(&mut self, at: Duration) {
        self.track = None;
        self.flush(at);
        self.pause_by = Some(Instant::now() + STOP_GRACE);
        self.set_state(PlayState::Stopped);
    }

    fn fail(&mut self, id: u64, message: String) {
        self.emit(PlayerEvent::Error { id, message });
        self.stop(Duration::ZERO);
    }

    /// A ring from `configure` (first open, or a device rate/channel change): what the old
    /// ring held is gone, so play on from what was heard, resampled for the new rate.
    fn adopt_ring(&mut self) {
        let Some((ring, rate)) = self.rings.try_iter().last() else {
            return;
        };
        self.ring = Some(ring);
        self.rate = rate;
        if self.track.is_some() {
            self.seek(self.shared.position());
        }
    }

    fn maintain(&mut self) {
        let Some(out) = &mut self.output else { return };
        // A reopen at another rate calls `configure`, which `adopt_ring` picks up.
        let failed = match out.maintain() {
            Maintained::Failed(e) => Some(e),
            _ => None,
        };
        let errors = out.take_errors();
        self.output_failed(failed);
        for e in &errors {
            log::warn!("audio output: {e}");
        }
        if let Some(e) = errors.iter().find(|e| e.is_fatal())
            && let Some(t) = &self.track
        {
            let id = t.id;
            self.fail(id, e.to_string());
        }
    }

    /// A failed reopen comes back every `maintain` until one works: tell it once.
    fn output_failed(&mut self, failed: Option<OpenError>) {
        let was = std::mem::replace(&mut self.output_lost, failed.is_some());
        if let Some(e) = failed
            && !was
        {
            log::warn!("audio output: {e}");
            self.emit(PlayerEvent::OutputLost);
        }
    }

    /// One unit of work: push pending audio, end the track, or decode a packet.
    /// False when there was nothing to do.
    fn step(&mut self) -> bool {
        let (Some(t), Some(ring)) = (&mut self.track, &mut self.ring) else {
            return false;
        };
        if self.shared.flush_pending() {
            return false;
        }
        if !t.pending.is_empty() {
            let n = t.pending.len().min(ring.slots()) / t.channels * t.channels;
            if n == 0 {
                return false;
            }
            let _ = ring.push_partial_slice(&t.pending[..n]); // n slots are free
            t.pending.drain(..n);
            if !t.started {
                t.started = true;
                let id = t.id;
                self.emit(PlayerEvent::TrackStarted(id));
                self.set_state(if self.shared.paused.load(Acquire) {
                    PlayState::Paused
                } else {
                    PlayState::Playing
                });
            }
            return true;
        }
        if t.eof {
            if ring.slots() < ring.buffer().capacity() {
                return false; // still playing out
            }
            let id = t.id;
            self.emit(PlayerEvent::TrackEnded(id));
            self.stop(self.shared.position()); // the clock stays at the end
            return true;
        }
        match t.next() {
            Ok(Some(pcm)) => {
                let ch = t.channels;
                let frames = pcm.samples.len() / ch;
                let drop = t.skip.min(frames);
                t.skip -= drop;
                self.scratch.clear();
                self.scratch.extend(
                    pcm.samples[drop * ch..]
                        .iter()
                        .map(|&s| s as f32 / 2_147_483_648.0),
                );
                t.resampler.process(&self.scratch, &mut t.pending);
            }
            Ok(None) => {
                t.resampler.flush(&mut t.pending);
                t.eof = true;
            }
            // The interrupting command is queued (or about to be): wait for it, not spin.
            Err(e) if superseded(&e, &self.interrupt) => return false,
            Err(e) => {
                let id = t.id;
                self.fail(id, e.to_string());
            }
        }
        true
    }

    fn emit(&self, event: PlayerEvent) {
        let _ = self.events.send(event); // nobody listening is fine
    }

    fn set_state(&mut self, state: PlayState) {
        if state != self.state {
            self.state = state;
            self.emit(PlayerEvent::State(state));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
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
}
