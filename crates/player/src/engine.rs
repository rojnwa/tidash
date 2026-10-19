//! The player thread: decoder -> resampler -> ring, commands in, events out.

use std::io::{self, Read};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use fastframe_audio::{Device, Maintained, OpenError, Output};
use rtrb::Producer;

use crate::exclusive::{self, Exclusive};
use crate::render::{NewRing, Renderer, Shared};
use crate::stream::is_interrupt;
use crate::track::{Next, Track, resampler};
use crate::{DecodeError, Decoder, Format, Interrupt, ProgressiveFile};

#[path = "sink.rs"]
mod sink;
use sink::{Sink, shared_options};

/// How long the thread sleeps on the command channel when there is nothing to decode.
const IDLE_WAIT: Duration = Duration::from_millis(50);
/// The same, while a running output is about to take a flush (start, seek): resume quickly.
const FLUSH_WAIT: Duration = Duration::from_millis(5);
/// The same, while the renderer is about to reach the next track: tell its start promptly.
const SWITCH_WAIT: Duration = Duration::from_millis(1);
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
    /// Linear, multiplies every decoded sample (ReplayGain); 1.0 leaves them unchanged.
    pub gain: f32,
}

pub enum PlayerCmd {
    Load(Source),
    Play,
    Pause,
    Stop,
    Seek(Duration),
    SetVolume(f32),
    /// `Some`: play this right after the current track (gapless when it can), replacing any
    /// earlier preload; dropped when nothing is loaded. `None` cancels a preload not switched
    /// to yet. `Load` and `Stop` drop any preload.
    Preload(Option<Source>),
    /// Play this way from now on. A loaded track moves to it at once, from its position;
    /// with none, shared output opens on the device now, and exclusive at the next `Load`.
    SetOutput(OutputConfig),
}

/// Which device to play on, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputConfig {
    pub device: Device,
    /// Hold the device at each track's own format (bit-perfect: no resampling, volume or
    /// gain), when it lets us; shared output otherwise.
    pub exclusive: bool,
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
    /// The track is heard from now on. If another track was playing, that one ended here
    /// (gapless, or after a short gap).
    TrackStarted(u64),
    /// Playback stopped at the track's natural end, with no preload after it.
    TrackEnded(u64),
    /// The track failed and playback stopped. A preload that fails before it is heard sends
    /// none: it is dropped, and the track before it plays out and ends.
    Error { id: u64, message: String },
    /// The output could not be opened again; sent on the first failed reopen, not on the retries
    /// after it. The output keeps trying, and a loaded track plays on once it is back.
    OutputLost,
    /// What is heard and how: the track's own format, whether the device is held exclusively,
    /// and the rate it runs at. Sent with every `TrackStarted`, and whenever the output under
    /// a started track changes.
    Format {
        track: Format,
        exclusive: bool,
        device_rate: u32,
    },
    /// Exclusive output was refused or lost (the reason): shared output plays now.
    Fallback(String),
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
    /// Opens shared output on the device (on the player thread; it can take a moment). Fails
    /// when the device cannot be opened. The output is paused until a track is loaded; with
    /// `exclusive`, that track then holds the device at its own format if it lets us.
    pub fn spawn(config: OutputConfig) -> io::Result<(Player, Receiver<PlayerEvent>)> {
        let (player, events, renderer, engine) = parts();
        let (ready, opened) = mpsc::channel();
        let options = shared_options(&config.device);
        std::thread::Builder::new()
            .name("player".into())
            .spawn(move || match Output::open(options, renderer) {
                Ok(output) => {
                    let _ = ready.send(Ok(()));
                    Engine {
                        sink: Some(Sink::Shared(Box::new(output))),
                        manual: false,
                        config,
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

/// A player and its engine, in manual mode: the returned renderer is the shared output.
fn parts() -> (Player, Receiver<PlayerEvent>, Renderer, Engine) {
    let shared = Arc::new(Shared::default());
    let interrupt = Interrupt::default();
    let loads = Arc::new(Mutex::new(0));
    let (cmds, cmd_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let (ring_tx, rings) = mpsc::channel();
    let engine = Engine {
        shared: Arc::clone(&shared),
        interrupt: interrupt.clone(),
        loads: Arc::clone(&loads),
        cmds: cmd_rx,
        events,
        rings,
        ring_tx,
        ring: None,
        rate: 0,
        track: None,
        ending: None,
        next: None,
        switches_seen: 0,
        state: PlayState::Stopped,
        scratch: Vec::new(),
        sink: None,
        manual: true,
        config: OutputConfig::default(),
        open_exclusive: Box::new(exclusive::open),
        output_lost: false,
        pause_by: None,
        callbacks_seen: 0,
        callback_at: Instant::now(),
    };
    let renderer = engine.renderer();
    let player = Player {
        cmds,
        shared,
        interrupt,
        loads,
    };
    (player, event_rx, renderer, engine)
}

/// An error caused by a Load, Stop or Seek arriving (a read given up): not reported, that
/// command decides what happens next. symphonia's probe turns a failed read into "no format
/// found", so any error while an interrupt is pending counts; a real one comes back after.
fn superseded(e: &DecodeError, interrupt: &Interrupt) -> bool {
    matches!(e, DecodeError::Io(e) if is_interrupt(e)) || interrupt.raised()
}

/// Whether `next` can play on in `current`'s ring, with no flush between them. An exclusive
/// device stays at one format.
fn seamless(current: &Track, next: &Decoder, exclusive: bool) -> bool {
    current.channels == usize::from(next.channels().max(1))
        && (!exclusive || current.decoder.format() == next.format())
}

type OpenExclusive = Box<dyn FnMut(&Device, Format, Renderer) -> Result<Exclusive, String> + Send>;

struct Engine {
    shared: Arc<Shared>,
    interrupt: Interrupt,
    loads: Arc<Mutex<u32>>,
    cmds: Receiver<(PlayerCmd, bool)>,
    events: Sender<PlayerEvent>,
    rings: Receiver<NewRing>,
    /// For each new renderer: where it sends its rings.
    ring_tx: Sender<NewRing>,
    ring: Option<Producer<f32>>,
    /// Device rate the ring and resampler are built for (0 before the first `configure`).
    rate: u32,
    track: Option<Track>,
    /// Gapless: the track still heard while `track` follows it in the ring, until the
    /// renderer reaches the boundary between them.
    ending: Option<Track>,
    /// The preloaded track to play after `track`.
    next: Option<Next>,
    /// `Shared::switches` as last seen.
    switches_seen: u64,
    state: PlayState,
    scratch: Vec<f32>,
    /// None only in manual mode, or after shared output failed to open.
    sink: Option<Sink>,
    /// No device of our own for shared output: the caller renders (tests).
    manual: bool,
    config: OutputConfig,
    /// `exclusive::open`; tests put a fake device here.
    open_exclusive: OpenExclusive,
    output_lost: bool,
    /// Set by a stop: the latest moment to pause the device (see `output_action`).
    pause_by: Option<Instant>,
    callbacks_seen: u64,
    callback_at: Instant,
}

impl Engine {
    fn run(mut self) {
        // This thread decodes what the output plays: a busy machine must not starve it.
        fastframe_audio::raise_feeder_priority();
        let mut maintained = Instant::now();
        let mut idle = false;
        loop {
            self.sync_output();
            let mut wait = Duration::ZERO; // a try_recv: commands first
            if idle {
                wait = self.idle_wait();
                if wait < IDLE_WAIT {
                    // A timed wait on the channel is only as fine as the OS timer (15.6 ms on
                    // Windows): sleep through a short one, then take commands.
                    std::thread::sleep(wait);
                    wait = Duration::ZERO;
                }
            }
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
            self.poll_switch(); // apart from `step`, which waits while the ring is full
            idle = !self.step();
        }
    }

    /// Poll fast only while a flush is about to be taken or the next track to be reached:
    /// the output should run and has called back lately. A device that is gone (no
    /// callbacks) gets the normal wait.
    fn idle_wait(&mut self) -> Duration {
        let n = self.shared.callbacks.load(Relaxed);
        if n != self.callbacks_seen {
            self.callbacks_seen = n;
            self.callback_at = Instant::now();
        }
        if !self.output_wanted() || self.callback_at.elapsed() >= CALLBACK_RECENT {
            IDLE_WAIT
        } else if self.ending.is_some() {
            SWITCH_WAIT
        } else if self.shared.flush_pending() {
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
        let Some(sink) = &mut self.sink else { return };
        match action {
            Some(true) => {
                if sink.is_paused() {
                    self.callback_at = Instant::now(); // expect callbacks from now on
                }
                sink.resume();
            }
            // ponytail: exclusive device held while paused; release after a long pause like
            // shared if users ask
            Some(false) => sink.pause(),
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
            PlayerCmd::Preload(src) => self.preload(src),
            PlayerCmd::SetOutput(config) => self.set_output(config),
        }
    }

    /// Starts downloading `src` to follow the current track; it is opened once that track's
    /// decoder is done. No track: dropped.
    fn preload(&mut self, src: Option<Source>) {
        self.next = src.filter(|_| self.track.is_some()).and_then(|src| {
            let file = ProgressiveFile::spawn(src.reader, src.len, &self.interrupt)
                .inspect_err(|e| log::warn!("preload of track {}: {e}", src.id))
                .ok()?;
            Some(Next {
                id: src.id,
                file,
                gain: src.gain,
                after_tail: false,
            })
        });
    }

    fn load(&mut self, src: Source) {
        (self.track, self.ending, self.next) = (None, None, None);
        self.adopt_ring(); // so the resampler is built for the current device rate
        self.flush(Duration::ZERO); // the old track stops now, not after the probe
        self.set_state(PlayState::Buffering);
        let opened = ProgressiveFile::spawn(src.reader, src.len, &self.interrupt)
            .map_err(DecodeError::Io)
            .and_then(|file| self.begin(src.id, file, src.gain));
        // This Load's probe is over: Seeks may interrupt reads again. Decide whether a failure
        // was superseded while still holding the lock, so a Seek sent after it cannot have
        // raised the interrupt (and so hide a real probe error).
        let mut loads = lock(&self.loads);
        *loads = loads.saturating_sub(1);
        let opened = opened.map_err(|e| (superseded(&e, &self.interrupt), e));
        drop(loads);
        match opened {
            Ok(()) | Err((true, _)) => {}
            Err((false, e)) => self.fail(src.id, e.to_string()),
        }
    }

    /// Opens `file` and makes it the current track, from its start, behind a flush: how every
    /// track starts but one that follows gaplessly.
    fn begin(&mut self, id: u64, file: ProgressiveFile, gain: f32) -> Result<(), DecodeError> {
        let decoder = Decoder::new(Box::new(file.forward()?))?;
        self.track = None; // any track before has played out
        self.use_output(decoder.format()); // a new output's ring is adopted: its rate is ours
        self.track = Some(Track::new(id, file, decoder, gain, self.rate));
        self.flush(Duration::ZERO); // again, now with the track's channels
        Ok(())
    }

    fn seek(&mut self, to: Duration) {
        self.undo_switch();
        let Some(t) = &mut self.track else { return };
        match t.seek(to) {
            Ok(at) => {
                let skip = to.saturating_sub(at).as_secs_f64() * f64::from(t.decoder.rate());
                t.skip = skip.round() as usize;
                t.pending.clear();
                t.eof = false;
                t.resampler = resampler(&t.decoder, self.rate);
                t.out_mark = 0;
                self.flush(to);
            }
            Err(e) if superseded(&e, &self.interrupt) => {} // the next command re-seeks or drops the track
            Err(DecodeError::PastEnd) => {
                let id = t.id;
                self.emit(PlayerEvent::TrackEnded(id));
                self.stop(Duration::ZERO);
            }
            // Anything else (IO, data gone bad under a rebuilt decoder) is a real failure.
            Err(e) => {
                let id = t.id;
                self.fail(id, e.to_string());
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
        s.boundary.store(u64::MAX, Relaxed); // gone with the ring
        s.flush_req.fetch_add(1, Release);
    }

    /// Drop the track and flush (the clock then reads `at`); the device pauses once the
    /// renderer took that flush (`output_action`).
    fn stop(&mut self, at: Duration) {
        (self.track, self.ending, self.next) = (None, None, None);
        self.flush(at);
        self.pause_by = Some(Instant::now() + STOP_GRACE);
        self.set_state(PlayState::Stopped);
    }

    fn fail(&mut self, id: u64, message: String) {
        self.emit(PlayerEvent::Error { id, message });
        self.stop(Duration::ZERO);
    }

    /// A ring from `configure` (an output opened, or a device rate/channel change): what the
    /// old ring held is gone, so play on from what was heard, resampled for the new rate.
    /// False when there was none.
    fn adopt_ring(&mut self) -> bool {
        let Some((ring, rate)) = self.rings.try_iter().last() else {
            return false;
        };
        self.ring = Some(ring);
        self.rate = rate;
        if self.track.is_some() {
            let at = self.poll_switch(); // one made before `configure`: the new track's position
            self.seek(at);
            self.format_event();
        }
        true
    }

    /// What is heard and how, once its track has started.
    fn format_event(&self) {
        let Some(t) = self.ending.as_ref().or(self.track.as_ref()) else {
            return;
        };
        if t.started {
            self.emit(PlayerEvent::Format {
                track: t.decoder.format(),
                exclusive: self.shared.exclusive.load(Relaxed),
                device_rate: self.rate,
            });
        }
    }

    fn maintain(&mut self) {
        let out = match &mut self.sink {
            Some(Sink::Shared(out)) => out,
            Some(Sink::Exclusive(x)) => {
                if x.failed() {
                    self.sink = None;
                    self.fallback("audio device lost".into());
                    self.sink_changed();
                }
                return;
            }
            None if self.manual => return,
            None => {
                // Shared output failed to open: again, until it does.
                self.open_shared();
                if self.sink.is_some() {
                    self.sink_changed();
                }
                return;
            }
        };
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
            && let Some(t) = self.ending.as_ref().or(self.track.as_ref())
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
                self.format_event();
                self.set_state(if self.shared.paused.load(Acquire) {
                    PlayState::Paused
                } else {
                    PlayState::Playing
                });
            }
            return true;
        }
        if t.eof {
            let (id, drained) = (t.id, ring.slots() == ring.buffer().capacity());
            if drained {
                self.switched(); // the ring ran dry: the renderer passed any boundary in it
            }
            if self.follow() {
                return true;
            }
            if !drained {
                return false; // still playing out
            }
            if let Some(n) = self.next.take()
                && self.begin(n.id, n.file, n.gain).is_ok()
            {
                return true; // its TrackStarted comes with its first samples, as after a Load
            }
            self.emit(PlayerEvent::TrackEnded(id));
            self.stop(self.shared.position()); // the clock stays at the end
            return true;
        }
        match t.next() {
            Ok(Some(pcm)) => {
                let ch = t.channels;
                let frames = pcm.samples.len() / ch;
                let drop = t.skip.min(frames);
                let bit_perfect = self.shared.exclusive.load(Relaxed);
                let gain = if bit_perfect { 1.0 } else { t.gain };
                t.skip -= drop;
                self.scratch.clear();
                self.scratch.extend(
                    pcm.samples[drop * ch..]
                        .iter()
                        .map(|&s| s as f32 / 2_147_483_648.0 * gain),
                );
                t.resampler.process(&self.scratch, &mut t.pending);
            }
            Ok(None) => {
                if !self.follow()
                    && let Some(t) = &mut self.track
                {
                    t.resampler.flush(&mut t.pending);
                    t.eof = true;
                }
            }
            // The interrupting command is queued (or about to be): wait for it, not spin.
            Err(e) if superseded(&e, &self.interrupt) => return false,
            Err(e) => {
                let id = t.id;
                let at = self.poll_switch(); // heard by now: then its error is the current track's
                if self.ending.is_some() {
                    // A preload failing before it is heard is dropped: the ending track plays
                    // out instead, and the app meets the error when it loads that one itself.
                    log::warn!("preloaded track {id}: {e}");
                    self.seek(at); // undoes the switch first
                    self.next = None;
                } else {
                    self.fail(id, e.to_string());
                }
            }
        }
        true
    }

    /// The current track's decoder is done and a preload waits: open it. When it can play on
    /// in the same ring, it becomes the current track at once, its samples queued behind the
    /// boundary, and the ending one stays heard until the renderer gets there. True when it
    /// switched; otherwise the current track ends as before.
    fn follow(&mut self) -> bool {
        if self.ending.is_some() || !self.next.as_ref().is_some_and(|n| !n.after_tail) {
            return false; // a boundary is pending already, or nothing new to open
        }
        let (Some(a), Some(next)) = (&mut self.track, self.next.take()) else {
            return false;
        };
        let opened = (next.file.forward())
            .map_err(DecodeError::Io)
            .and_then(|view| Decoder::new(Box::new(view)));
        let decoder = match opened {
            Ok(decoder) => decoder,
            // Interrupted: kept, for the command that interrupted it to re-arm or drop.
            Err(e) if superseded(&e, &self.interrupt) => {
                self.next = Some(next);
                return false;
            }
            Err(e) => {
                log::warn!("preloaded track {}: {e}", next.id);
                return false; // dropped
            }
        };
        if !seamless(a, &decoder, self.config.exclusive) {
            self.next = Some(Next {
                after_tail: true,
                ..next
            });
            return false;
        }
        // In the renderer's count of `a`'s frames; taken before anything moves.
        let boundary = a.resampler.expected_out().saturating_sub(a.out_mark);
        let mut b = Track::new(next.id, next.file, decoder, next.gain, self.rate);
        b.pending = std::mem::take(&mut a.pending);
        if !a.eof && a.decoder.rate() == b.decoder.rate() {
            // One resampler goes through both tracks' samples, as through one stream.
            b.out_mark = a.resampler.expected_out();
            std::mem::swap(&mut a.resampler, &mut b.resampler);
        } else if !a.eof {
            a.resampler.flush(&mut b.pending);
        }
        b.started = true; // its TrackStarted comes with the switch
        // Before arming: a switch counted until now raced a flush and is not this one.
        self.switches_seen = self.shared.switches.load(Acquire);
        self.shared.boundary.store(boundary, Release);
        self.ending = self.track.replace(b);
        true
    }

    /// The renderer reached the boundary: the current track is heard from now on. Returns the
    /// position heard, read at the same moment as the switch.
    fn poll_switch(&mut self) -> Duration {
        let (n, at) = self.shared.heard();
        if n != self.switches_seen {
            self.switches_seen = n;
            self.switched();
        }
        at
    }

    /// The ending track is over. With none (an undone switch), nothing happens.
    fn switched(&mut self) {
        if self.ending.take().is_some()
            && let Some(t) = &self.track
        {
            self.emit(PlayerEvent::TrackStarted(t.id));
            self.format_event();
        }
    }

    /// A seek (or device change) before the renderer reached the boundary: the ending track
    /// is the one heard, so it is current again, and the next goes back to being a preload.
    fn undo_switch(&mut self) {
        let Some(a) = self.ending.take() else { return };
        if let Some(b) = self.track.replace(a) {
            self.next = Some(Next {
                id: b.id,
                file: b.file,
                gain: b.gain,
                after_tail: false,
            });
        }
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
#[path = "engine/tests.rs"]
mod tests;
