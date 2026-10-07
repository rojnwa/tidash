//! The player thread: decoder -> resampler -> ring, commands in, events out.

use std::io::{self, Read};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use fastframe_audio::{Maintained, Output, OutputOptions};
use rtrb::Producer;

use crate::render::{NewRing, Renderer, Shared};
use crate::stream::is_interrupt;
use crate::{DecodeError, Decoder, Interrupt, ProgressiveFile, Resampler};

/// How long the thread sleeps on the command channel when there is nothing to decode.
const IDLE_WAIT: Duration = Duration::from_millis(50);
/// The same, while a running output is about to take a flush (start, seek): resume quickly.
const FLUSH_WAIT: Duration = Duration::from_millis(5);
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
}

/// The source the player thread reads, and whether it is still probing it. Locked by
/// `Player::send` and the player thread, never by the renderer.
type Reading = Arc<Mutex<Option<(Interrupt, bool)>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Handle to the player thread. Cheap to clone; dropping the last clone disconnects the
/// command channel, and the thread (and the device output it owns) then exits.
#[derive(Clone)]
pub struct Player {
    cmds: Sender<(PlayerCmd, Option<Interrupt>)>,
    shared: Arc<Shared>,
    reading: Reading,
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
        // Load, Stop and Seek must not queue behind a read waiting on the network: interrupt
        // it. A Seek leaves a Load's probe alone; it is meant for that track.
        let interrupt = match (&cmd, &*lock(&self.reading)) {
            (PlayerCmd::Load(_) | PlayerCmd::Stop, Some((i, _)))
            | (PlayerCmd::Seek(_), Some((i, false))) => Some(i.clone()),
            _ => None,
        };
        if let Some(i) = &interrupt {
            i.interrupt();
        }
        // The thread only exits once every `Player` is gone, so this cannot fail.
        let _ = self.cmds.send((cmd, interrupt));
    }

    pub fn position(&self) -> Duration {
        self.shared.position()
    }
}

fn parts() -> (Player, Receiver<PlayerEvent>, Renderer, Engine) {
    let shared = Arc::new(Shared::default());
    let reading = Reading::default();
    let (cmds, cmd_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let (rings, ring_rx) = mpsc::channel();
    let engine = Engine {
        shared: Arc::clone(&shared),
        reading: Arc::clone(&reading),
        cmds: cmd_rx,
        events,
        rings: ring_rx,
        ring: None,
        rate: 0,
        track: None,
        state: PlayState::Stopped,
        scratch: Vec::new(),
        output: None,
    };
    let renderer = Renderer::new(Arc::clone(&shared), rings);
    let player = Player {
        cmds,
        shared,
        reading,
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

/// A read given up because a Load, Stop or Seek arrived: no error, that command decides.
fn interrupted(e: &DecodeError) -> bool {
    matches!(e, DecodeError::Io(e) if is_interrupt(e))
}

struct Track {
    id: u64,
    decoder: Decoder,
    resampler: Resampler,
    channels: usize,
    /// Source frames still to drop after a seek (it lands at or before the target).
    skip: usize,
    /// Resampled samples that did not fit in the ring yet.
    pending: Vec<f32>,
    eof: bool,
    started: bool,
}

struct Engine {
    shared: Arc<Shared>,
    reading: Reading,
    cmds: Receiver<(PlayerCmd, Option<Interrupt>)>,
    events: Sender<PlayerEvent>,
    rings: Receiver<NewRing>,
    ring: Option<Producer<f32>>,
    /// Device rate the ring and resampler are built for (0 before the first `configure`).
    rate: u32,
    track: Option<Track>,
    state: PlayState,
    scratch: Vec<f32>,
    output: Option<Output<Renderer>>,
}

impl Engine {
    fn run(mut self) {
        let mut maintained = Instant::now();
        let mut idle = false;
        loop {
            self.sync_output();
            let wait = if !idle {
                Duration::ZERO // a try_recv: commands first
            } else if self.shared.flush_pending() && self.output_wanted() {
                FLUSH_WAIT
            } else {
                IDLE_WAIT
            };
            match self.cmds.recv_timeout(wait) {
                Ok((cmd, interrupt)) => {
                    if let Some(i) = interrupt {
                        i.handled();
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

    /// The device runs only while a track is loaded and the user has not paused: stopped (also
    /// right after opening) it makes no callbacks. A Load resumes it: its flush needs one.
    fn output_wanted(&self) -> bool {
        self.track.is_some() && !self.shared.paused.load(Acquire)
    }

    fn sync_output(&mut self) {
        let run = self.output_wanted();
        if let Some(out) = &mut self.output {
            if run { out.resume() } else { out.pause() }
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
            PlayerCmd::Stop => self.stop(),
            PlayerCmd::Seek(to) => self.seek(to),
            PlayerCmd::SetVolume(v) => self.shared.volume.store(v.to_bits(), Relaxed),
        }
    }

    fn load(&mut self, src: Source) {
        self.track = None;
        self.adopt_ring(); // so the resampler is built for the current device rate
        self.flush(Duration::ZERO); // the old track stops now, not after the probe
        self.set_state(PlayState::Buffering);
        let file = match ProgressiveFile::spawn(src.reader, src.len) {
            Ok(file) => file,
            Err(e) => {
                self.fail(src.id, e.to_string());
                return;
            }
        };
        *lock(&self.reading) = Some((file.interrupt_handle(), true));
        match Decoder::new(Box::new(file)) {
            Ok(decoder) => {
                if let Some((_, probing)) = lock(&self.reading).as_mut() {
                    *probing = false;
                }
                self.track = Some(Track {
                    id: src.id,
                    resampler: resampler(&decoder, self.rate),
                    channels: usize::from(decoder.channels().max(1)),
                    decoder,
                    skip: 0,
                    pending: Vec::new(),
                    eof: false,
                    started: false,
                });
                self.flush(Duration::ZERO); // again, now with the track's channels
            }
            Err(e) if interrupted(&e) => {}
            Err(e) => self.fail(src.id, e.to_string()),
        }
    }

    fn seek(&mut self, to: Duration) {
        let Some(t) = &mut self.track else { return };
        match t.decoder.seek(to) {
            Ok(at) => {
                let skip = to.saturating_sub(at).as_secs_f64() * f64::from(t.decoder.rate());
                t.skip = skip.round() as usize;
                t.pending.clear();
                t.eof = false;
                t.resampler = resampler(&t.decoder, self.rate);
                self.flush(to);
            }
            Err(e) if interrupted(&e) => {} // the next command re-seeks or drops the track
            Err(DecodeError::Io(e)) => {
                let id = t.id;
                self.fail(id, e.to_string());
            }
            Err(_) => {
                // symphonia knows no fMP4 duration up front: a refused seek is past the end.
                let id = t.id;
                self.emit(PlayerEvent::TrackEnded(id));
                self.stop();
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

    fn stop(&mut self) {
        self.track = None;
        self.flush(Duration::ZERO);
        self.set_state(PlayState::Stopped);
    }

    fn fail(&mut self, id: u64, message: String) {
        self.emit(PlayerEvent::Error { id, message });
        self.stop();
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
        if let Maintained::Failed(e) = out.maintain() {
            log::warn!("audio output: {e}");
        }
        let errors = out.take_errors();
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
            self.track = None;
            self.emit(PlayerEvent::TrackEnded(id));
            self.set_state(PlayState::Stopped);
            return true;
        }
        match t.decoder.next() {
            Ok(Some(pcm)) => {
                let ch = t.channels;
                let drop = t.skip.min(pcm.samples.len() / ch);
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
            Err(e) if interrupted(&e) => return false,
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

    /// The device output follows `output_wanted` at the top of every loop iteration
    /// (`sync_output`); this drives the engine synchronously through each transition.
    #[test]
    fn output_runs_only_with_an_unpaused_track() {
        let (_player, _events, mut r, mut e) = parts();
        r.configure(48_000, 2);
        assert!(!e.output_wanted(), "stopped at launch");
        e.command(PlayerCmd::Play);
        assert!(!e.output_wanted(), "play with nothing loaded");
        e.command(PlayerCmd::Load(fixture(1)));
        assert!(e.output_wanted(), "loaded");
        e.command(PlayerCmd::Pause);
        assert!(!e.output_wanted(), "paused");
        e.command(PlayerCmd::Play);
        assert!(e.output_wanted(), "playing");
        e.command(PlayerCmd::Stop);
        assert!(!e.output_wanted(), "stopped");
        let broken = Source {
            id: 2,
            reader: Box::new(io::empty()),
            len: None,
        };
        e.command(PlayerCmd::Load(broken));
        assert!(!e.output_wanted(), "load failed");
        e.command(PlayerCmd::Load(fixture(3)));
        let mut block = [0.0f32; 960];
        for _ in 0..10_000 {
            if e.track.is_none() {
                break;
            }
            r.render(&mut block);
            e.step();
        }
        assert!(e.track.is_none(), "track did not end");
        assert!(!e.output_wanted(), "track ended");
    }
}
