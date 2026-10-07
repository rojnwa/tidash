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
    pub(crate) paused: AtomicBool,
    /// Set by the player thread (after `pending_base_ns` and `channels`), cleared by the
    /// renderer once it discarded the ring. The player never pushes while it is set.
    pub(crate) flush: AtomicBool,
    pub(crate) pending_base_ns: AtomicU64,
    /// Channels of what is pushed after the pending flush.
    pub(crate) channels: AtomicU32,
    pub(crate) base_ns: AtomicU64,
    /// Frames of the current track rendered since `base_ns`.
    pub(crate) frames: AtomicU64,
    pub(crate) device_rate: AtomicU32,
    pub(crate) device_channels: AtomicU32,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            volume: AtomicU32::new(1.0f32.to_bits()),
            paused: AtomicBool::new(false),
            flush: AtomicBool::new(false),
            pending_base_ns: AtomicU64::new(0),
            channels: AtomicU32::new(2),
            base_ns: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            device_rate: AtomicU32::new(0),
            device_channels: AtomicU32::new(0),
        }
    }
}

impl Shared {
    /// `base_ns + frames / device_rate`; a pending flush (seek, load) already counts.
    /// Read without locking: a momentary inconsistency is fine for UI.
    pub(crate) fn position(&self) -> Duration {
        if self.flush.load(Acquire) {
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
}

impl Renderer {
    pub(crate) fn new(shared: Arc<Shared>, rings: Sender<NewRing>) -> Self {
        Self {
            shared,
            ring: None,
            rings,
            rate: 0,
            channels: 2,
            src_channels: 2,
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
        // Keep position() continuous across the rate change.
        let heard = frames_to_ns(s.frames.swap(0, Relaxed), self.rate);
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
        if s.paused.load(Acquire) {
            return;
        }
        let Some(ring) = &mut self.ring else { return };
        if s.flush.load(Acquire) {
            if let Ok(stale) = ring.read_chunk(ring.slots()) {
                stale.commit_all();
            }
            self.src_channels = (s.channels.load(Relaxed) as usize).max(1);
            s.base_ns.store(s.pending_base_ns.load(Relaxed), Relaxed);
            s.frames.store(0, Relaxed);
            s.flush.store(false, Release);
            return;
        }
        let (dev, src) = (self.channels, self.src_channels);
        let n = (out.len() / dev).min(ring.slots() / src);
        let Ok(chunk) = ring.read_chunk(n * src) else {
            return;
        };
        let volume = f32::from_bits(s.volume.load(Relaxed));
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
        s.frames.fetch_add(n as u64, Relaxed);
    }
}
