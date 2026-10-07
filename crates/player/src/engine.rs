//! The player thread: decoder -> resampler -> ring, commands in, events out.

use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use fastframe_audio::{Output, OutputOptions};
use rtrb::Producer;

use crate::render::{NewRing, Renderer, Shared};
use crate::{DecodeError, Decoder, ProgressiveFile, Resampler};

/// How long the thread sleeps on the command channel when there is nothing to decode.
const IDLE_WAIT: Duration = Duration::from_millis(50);
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

/// Handle to the player thread. Cheap to clone; dropping the last clone disconnects the
/// command channel, and the thread (and the device output it owns) then exits.
#[derive(Clone)]
pub struct Player {
    cmds: Sender<PlayerCmd>,
    shared: Arc<Shared>,
}

impl Player {
    /// Opens the audio output (on the player thread; it can take a moment) and starts playing
    /// through it. Fails when the device cannot be opened.
    pub fn spawn(options: OutputOptions) -> io::Result<(Player, Receiver<PlayerEvent>)> {
        let (player, events, renderer, engine) = parts();
        let (ready, opened) = mpsc::channel();
        std::thread::Builder::new()
            .name("player".into())
            .spawn(move || match Output::open(options, renderer) {
                Ok(output) => {
                    let _ = ready.send(Ok(()));
                    engine.run(Some(output));
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
        std::thread::spawn(move || engine.run(None));
        (player, events, renderer)
    }

    pub fn send(&self, cmd: PlayerCmd) {
        // The thread only exits once every `Player` is gone, so this cannot fail.
        let _ = self.cmds.send(cmd);
    }

    pub fn position(&self) -> Duration {
        self.shared.position()
    }
}

fn parts() -> (Player, Receiver<PlayerEvent>, Renderer, Engine) {
    let shared = Arc::new(Shared::default());
    let (cmds, cmd_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let (rings, ring_rx) = mpsc::channel();
    let engine = Engine {
        shared: Arc::clone(&shared),
        cmds: cmd_rx,
        events,
        rings: ring_rx,
        ring: None,
        rate: 0,
        track: None,
        state: PlayState::Stopped,
        scratch: Vec::new(),
    };
    let renderer = Renderer::new(Arc::clone(&shared), rings);
    (Player { cmds, shared }, event_rx, renderer, engine)
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
    cmds: Receiver<PlayerCmd>,
    events: Sender<PlayerEvent>,
    rings: Receiver<NewRing>,
    ring: Option<Producer<f32>>,
    /// Device rate the ring and resampler are built for (0 before the first `configure`).
    rate: u32,
    track: Option<Track>,
    state: PlayState,
    scratch: Vec<f32>,
}

impl Engine {
    fn run(mut self, mut output: Option<Output<Renderer>>) {
        let mut maintained = Instant::now();
        let mut idle = false;
        loop {
            // A zero timeout is a try_recv: commands first, block only when idle.
            match self
                .cmds
                .recv_timeout(if idle { IDLE_WAIT } else { Duration::ZERO })
            {
                Ok(cmd) => {
                    self.command(cmd, output.as_mut());
                    idle = false;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if let Some(out) = output.as_mut()
                && maintained.elapsed() >= MAINTAIN_EVERY
            {
                maintained = Instant::now();
                self.maintain(out);
            }
            self.adopt_ring();
            idle = !self.step();
        }
    }

    fn command(&mut self, cmd: PlayerCmd, output: Option<&mut Output<Renderer>>) {
        match cmd {
            PlayerCmd::Load(src) => self.load(src),
            PlayerCmd::Play => {
                self.shared.paused.store(false, Release);
                if let Some(out) = output {
                    out.resume();
                }
                if self.state == PlayState::Paused {
                    self.set_state(PlayState::Playing);
                }
            }
            PlayerCmd::Pause => {
                self.shared.paused.store(true, Release);
                if let Some(out) = output {
                    out.pause();
                }
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
        self.flush(Duration::ZERO); // the old track stops now, not after the probe
        self.set_state(PlayState::Buffering);
        let decoder = ProgressiveFile::spawn(src.reader, src.len)
            .map_err(DecodeError::Io)
            .and_then(|file| Decoder::new(Box::new(file)));
        match decoder {
            Ok(decoder) => {
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
        s.flush.store(true, Release);
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

    fn maintain(&mut self, output: &mut Output<Renderer>) {
        // A reopen at another rate calls `configure`, which `adopt_ring` picks up.
        if let fastframe_audio::Maintained::Failed(e) = output.maintain() {
            log::warn!("audio output: {e}");
        }
        for e in output.take_errors() {
            log::warn!("audio output: {e}");
            if e.is_fatal()
                && let Some(t) = &self.track
            {
                let id = t.id;
                self.fail(id, e.to_string());
            }
        }
    }

    /// One unit of work: push pending audio, end the track, or decode a packet.
    /// False when there was nothing to do.
    fn step(&mut self) -> bool {
        let (Some(t), Some(ring)) = (&mut self.track, &mut self.ring) else {
            return false;
        };
        if self.shared.flush.load(Acquire) {
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
