//! The real-time side: what the audio device (or a test) pulls samples from.

use std::sync::Arc;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::mpsc::Sender;
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

/// What the player thread, the renderer and `Player` handles share. Lock-free.
pub(crate) struct Shared {
    /// Linear gain, as `f32` bits.
    pub(crate) volume: AtomicU32,
    /// The output is exclusive: bit-perfect, so the renderer ignores `volume`.
    pub(crate) exclusive: AtomicBool,
    pub(crate) paused: AtomicBool,
    /// Flush generations. The player thread stores `pending_base_ns` and `channels`, then bumps
    /// `flush_req`; the renderer discards the ring, takes them and stores that generation in
    /// `flush_ack`. The player pushes only while `flush_ack == flush_req`. A counter, not a
    /// flag: a request landing while the renderer handles the previous one is not lost.
    pub(crate) flush_req: AtomicU64,
    pub(crate) flush_ack: AtomicU64,
    pub(crate) pending_base_ns: AtomicU64,
    /// Channels of what is pushed after the pending flush.
    pub(crate) channels: AtomicU32,
    pub(crate) base_ns: AtomicU64,
    /// Frames of the current track rendered since `base_ns`.
    pub(crate) frames: AtomicU64,
    /// The `frames` count where the next track starts in the ring (gapless), `u64::MAX` for
    /// none. Once it is rendered, the renderer restarts the clock there at 0, clears it and
    /// bumps `switches`. Every flush clears it first.
    pub(crate) boundary: AtomicU64,
    pub(crate) switches: AtomicU64,
    pub(crate) device_rate: AtomicU32,
    pub(crate) device_channels: AtomicU32,
    /// Render calls so far: tells the player thread whether the device is calling back.
    pub(crate) callbacks: AtomicU64,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            volume: AtomicU32::new(1.0f32.to_bits()),
            exclusive: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            flush_req: AtomicU64::new(0),
            flush_ack: AtomicU64::new(0),
            pending_base_ns: AtomicU64::new(0),
            channels: AtomicU32::new(2),
            base_ns: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            boundary: AtomicU64::new(u64::MAX),
            switches: AtomicU64::new(0),
            device_rate: AtomicU32::new(0),
            device_channels: AtomicU32::new(0),
            callbacks: AtomicU64::new(0),
        }
    }
}

impl Shared {
    pub(crate) fn flush_pending(&self) -> bool {
        self.flush_ack.load(Acquire) != self.flush_req.load(Acquire)
    }

    /// `base_ns + frames / device_rate`; a pending flush (seek, load) already counts.
    /// Read without locking: a momentary inconsistency is fine for UI.
    pub(crate) fn position(&self) -> Duration {
        if self.flush_pending() {
            return Duration::from_nanos(self.pending_base_ns.load(Relaxed));
        }
        let frames = self.frames.load(Relaxed);
        let ns = frames_to_ns(frames, self.device_rate.load(Relaxed));
        Duration::from_nanos(self.base_ns.load(Relaxed).saturating_add(ns))
    }
}

fn frames_to_ns(frames: u64, rate: u32) -> u64 {
    if rate == 0 {
        return 0;
    }
    u64::try_from(u128::from(frames) * 1_000_000_000 / u128::from(rate)).unwrap_or(u64::MAX)
}

/// A new ring and the device rate it was sized for, from `configure` to the player thread.
pub(crate) type NewRing = (Producer<f32>, u32);

/// Pulls interleaved `f32` from the player's ring. [`render`](Self::render) never allocates,
/// locks, blocks or logs: atomics and the ring consumer only.
pub struct Renderer {
    shared: Arc<Shared>,
    ring: Option<Consumer<f32>>,
    rings: Sender<NewRing>,
    rate: u32,
    channels: usize,
    /// Channels of the samples in the ring, taken at each flush.
    src_channels: usize,
    /// Last flush generation handled.
    acked: u64,
}

impl Renderer {
    /// Carries on the player's clock and flushes from where they stand, so any number can be
    /// made over a player's life, one in use at a time.
    pub(crate) fn new(shared: Arc<Shared>, rings: Sender<NewRing>) -> Self {
        let acked = shared.flush_ack.load(Acquire);
        let src_channels = (shared.channels.load(Relaxed) as usize).max(1);
        Self {
            shared,
            ring: None,
            rings,
            rate: 0,
            channels: 2,
            src_channels,
            acked,
        }
    }
}

impl fastframe_audio::Render for Renderer {
    /// A new rate or channel count gets a new ring (0.5 s at the device rate × channels):
    /// what the old one held is dropped, and the player thread re-seeks to what was heard.
    fn configure(&mut self, sample_rate: u32, channels: u16) {
        let channels = usize::from(channels.max(1));
        if self.ring.is_some() && (sample_rate, channels) == (self.rate, self.channels) {
            return;
        }
        let s = &*self.shared;
        // Keep position() continuous across the rate change, whichever renderer counted them.
        let heard = frames_to_ns(s.frames.swap(0, Relaxed), s.device_rate.load(Relaxed));
        s.base_ns.fetch_add(heard, Relaxed);
        s.device_rate.store(sample_rate, Relaxed);
        s.device_channels.store(channels as u32, Relaxed);
        (self.rate, self.channels) = (sample_rate, channels);
        let (producer, consumer) = RingBuffer::new((sample_rate as usize / 2).max(1) * channels);
        self.ring = Some(consumer);
        // The player thread is gone only when the player is: nothing to tell then.
        let _ = self.rings.send((producer, sample_rate));
    }

    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let s = &*self.shared;
        s.callbacks.fetch_add(1, Relaxed);
        if s.paused.load(Acquire) {
            return;
        }
        let Some(ring) = &mut self.ring else { return };
        let req = s.flush_req.load(Acquire);
        if req != self.acked {
            if let Ok(stale) = ring.read_chunk(ring.slots()) {
                stale.commit_all();
            }
            self.src_channels = (s.channels.load(Relaxed) as usize).max(1);
            s.base_ns.store(s.pending_base_ns.load(Relaxed), Relaxed);
            s.frames.store(0, Relaxed);
            s.flush_ack.store(req, Release);
            self.acked = req;
            return;
        }
        let (dev, src) = (self.channels, self.src_channels);
        let n = (out.len() / dev).min(ring.slots() / src);
        let Ok(chunk) = ring.read_chunk(n * src) else {
            return;
        };
        let volume = if s.exclusive.load(Relaxed) {
            1.0
        } else {
            f32::from_bits(s.volume.load(Relaxed))
        };
        let mut samples = chunk.into_iter();
        for frame in out.chunks_exact_mut(dev).take(n) {
            // Source channel i -> device channel i; extra device channels stay silent.
            for (i, v) in samples.by_ref().take(src).enumerate() {
                if let Some(o) = frame.get_mut(i) {
                    *o = v * volume;
                }
            }
            if src == 1 && dev > 1 {
                frame[1] = frame[0];
            }
        }
        drop(samples); // commits what was read
        let frames = s.frames.fetch_add(n as u64, Relaxed) + n as u64;
        let boundary = s.boundary.load(Acquire);
        if frames >= boundary {
            // The next track began `frames - boundary` frames ago: its clock starts there.
            s.base_ns.store(0, Relaxed);
            s.frames.store(frames - boundary, Relaxed);
            s.boundary.store(u64::MAX, Relaxed);
            s.switches.fetch_add(1, Release);
        }
    }
}
