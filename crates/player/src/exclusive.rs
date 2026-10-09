//! Exclusive output: the device held at the track's own format, bit-perfect.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Acquire;

use fastframe_audio::Device;

use crate::{Format, Renderer};

#[cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    expect(dead_code, reason = "used on macOS and Linux, type-checked here")
)]
mod cpal;
#[cfg(windows)]
mod wasapi;

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
    #[cfg(windows)]
    {
        wasapi::open(device, format, renderer)
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        cpal::open(device, format, renderer)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (device, format, renderer);
        Err("exclusive output is not supported on this platform".into())
    }
}

/// A sample from the ring as the device's left-justified integer, saturating at full scale.
/// Exact for any sample of up to 24 bits.
#[cfg_attr(
    not(any(test, windows, target_os = "macos", target_os = "linux")),
    expect(dead_code, reason = "no OS backend")
)]
pub(crate) fn to_i32(v: f32) -> i32 {
    (v * 2_147_483_648.0) as i32
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::atomic::Ordering::{Relaxed, Release};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::render::Shared;

    /// Info and above to stderr, past the test harness's capture.
    struct Stderr;

    impl log::Log for Stderr {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, r: &log::Record) {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr(),
                "{} {}: {}",
                r.level(),
                r.target(),
                r.args()
            );
        }

        fn flush(&self) {}
    }

    /// The backend's hardware test: silence through `open` on the default device and on it by
    /// `name` (its name as fastframe lists it), paused and resumed.
    pub(super) fn opens_the_default_device(name: String) {
        let _ = log::set_logger(&Stderr);
        log::set_max_level(log::LevelFilter::Info);
        let names = fastframe_audio::output_device_names().unwrap();
        assert!(names.contains(&name), "{name} not in {names:?}");
        let opens = [
            (Device::Default, (44_100, 16)),
            (Device::Named(name), (96_000, 24)),
        ];
        let ms = |n| thread::sleep(Duration::from_millis(n));
        for (device, (rate, bits)) in opens {
            let format = Format {
                rate,
                bits,
                channels: 2,
            };
            // A paused player's: it renders silence.
            let shared = Arc::new(Shared::default());
            shared.paused.store(true, Release);
            let (rings, _ring) = mpsc::channel();
            let renderer = Renderer::new(Arc::clone(&shared), rings);
            let mut x = open(&device, format, renderer).unwrap();
            let calls = || shared.callbacks.load(Relaxed);
            ms(200);
            assert!(calls() > 0, "{format:?}: running");
            x.pause();
            ms(20);
            let parked = calls();
            ms(20);
            assert_eq!(calls(), parked, "{format:?}: no wakeups while paused");
            x.resume();
            ms(200);
            assert!(calls() > parked, "{format:?}: running again");
            assert!(!x.failed(), "{format:?}");
            drop(x);
        }
    }

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
