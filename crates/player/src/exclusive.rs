//! Exclusive output: the device held at the track's own format, bit-perfect.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Acquire;

use fastframe_audio::Device;

use crate::{Format, Renderer};

/// An OS backend's open stream; dropping it closes the device.
trait Stream: Send {
    fn pause(&mut self);
    fn resume(&mut self);
}

/// An open exclusive stream at one format, pulling from a `Renderer`. Dropping it closes the
/// device.
pub(crate) struct Exclusive {
    pub(crate) format: Format,
    /// `None` when detached (tests).
    stream: Option<Box<dyn Stream>>,
    /// Set by the backend when the device fails or goes away.
    failed: Arc<AtomicBool>,
    paused: bool,
}

impl Exclusive {
    pub(crate) fn pause(&mut self) {
        if !self.paused {
            self.paused = true;
            if let Some(s) = &mut self.stream {
                s.pause();
            }
        }
    }

    pub(crate) fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            if let Some(s) = &mut self.stream {
                s.resume();
            }
        }
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Acquire)
    }

    /// No device; setting the flag fails it.
    #[cfg(test)]
    pub(crate) fn detached(format: Format) -> (Exclusive, Arc<AtomicBool>) {
        let failed = Arc::new(AtomicBool::new(false));
        let x = Exclusive {
            format,
            stream: None,
            failed: Arc::clone(&failed),
            paused: false,
        };
        (x, failed)
    }
}

/// Holds `device` at `format` (no resampling, no volume) and starts it pulling from
/// `renderer`, configured at the format's rate and channels first. `Err` is why it refused.
pub(crate) fn open(
    device: &Device,
    format: Format,
    renderer: Renderer,
) -> Result<Exclusive, String> {
    let _ = (device, format, renderer);
    Err("exclusive output is not supported on this platform".into())
}

/// A sample from the ring as the device's left-justified integer, saturating at full scale.
/// Exact for any sample of up to 24 bits.
#[cfg_attr(not(test), expect(dead_code, reason = "no OS backend yet"))]
pub(crate) fn to_i32(v: f32) -> i32 {
    (v * 2_147_483_648.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_i32_round_trips_every_24_bit_value() {
        for i in -(1 << 23)..(1 << 23) {
            let v = ((i << 8) as f32) / 2_147_483_648.0;
            assert_eq!(to_i32(v), i << 8, "{i}");
        }
        assert_eq!(to_i32(-1.0), i32::MIN);
        assert_eq!(to_i32(1.0), i32::MAX, "saturates");
    }
}
