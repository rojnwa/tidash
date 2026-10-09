//! macOS and Linux: cpal at the track's own format; on macOS the device in hog mode, on Linux
//! an ALSA `hw:` device.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Release;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, ErrorKind, I24, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize,
    SupportedStreamConfigRange,
};
use fastframe_audio::{Device, Render};

use super::{Exclusive, Stream, to_i32};
use crate::{Format, Renderer};

/// The integer formats that hold a `bits`-bit sample unchanged, preferred first.
// ponytail: DACs that take only S24_3LE fall back to shared on Linux; the alsa crate can drive
// them if users ask.
fn sample_formats(bits: u16) -> &'static [SampleFormat] {
    use SampleFormat::{I16, I24, I32};
    match bits {
        ..=16 => &[I16, I32, I24],
        17..=24 => &[I32, I24],
        _ => &[],
    }
}

/// The stream config and sample format that play `format` unchanged on a device with `ranges`:
/// its exact rate and channels, and a fixed period of 10 ms (within the device's limits).
fn pick(
    ranges: &[SupportedStreamConfigRange],
    format: Format,
) -> Option<(StreamConfig, SampleFormat)> {
    let Format {
        rate,
        bits,
        channels,
    } = format;
    sample_formats(bits).iter().find_map(|&sample| {
        let range = ranges.iter().find(|r| {
            r.sample_format() == sample && r.channels() == channels && r.contains_rate(rate)
        })?;
        let period = match *range.buffer_size() {
            SupportedBufferSize::Range { min, max } => (rate / 100).max(min).min(max),
            SupportedBufferSize::Unknown => rate / 100,
        };
        let config = StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: BufferSize::Fixed(period),
        };
        Some((config, sample))
    })
}

/// cpal lists every CoreAudio range as F32 and sets the physical format to what it is given:
/// the ranges as the integer formats it sends as such. (Its I24 goes out as a 32-bit integer
/// there, so never that.)
#[cfg(target_os = "macos")]
fn integer_ranges(ranges: &[SupportedStreamConfigRange]) -> Vec<SupportedStreamConfigRange> {
    ranges
        .iter()
        .flat_map(|r| {
            [SampleFormat::I16, SampleFormat::I32].map(|f| {
                SupportedStreamConfigRange::new(
                    r.channels(),
                    r.min_sample_rate(),
                    r.max_sample_rate(),
                    *r.buffer_size(),
                    f,
                )
            })
        })
        .collect()
}

/// An open stream; on macOS, with the device in hog mode.
struct Cpal {
    /// Dropped first: its drop returns once the callbacks have stopped.
    stream: cpal::Stream,
    /// Released once the stream is gone.
    #[cfg(target_os = "macos")]
    _hog: hog::Hog,
    /// One device period.
    period: Duration,
    paused: bool,
    failed: Arc<AtomicBool>,
}

impl Stream for Cpal {
    fn pause(&mut self) {
        self.paused = true;
        // A device that cannot pause calls back on: the paused player renders silence.
        let _ = self.stream.pause();
    }

    fn resume(&mut self) {
        self.paused = false;
        if self.stream.play().is_err() {
            self.failed.store(true, Release);
        }
    }
}

impl Drop for Cpal {
    /// Running, the device gets one period to play what it has. Then the fields drop in order:
    /// the stream, so the renderer is never rendered after this returns, then hog mode.
    fn drop(&mut self) {
        if !self.paused {
            thread::sleep(self.period);
        }
        // Stopped first: cpal's macOS device monitor can hold the stream a moment past its drop.
        let _ = self.stream.pause();
    }
}

/// Holds `device` at `format` through cpal, pulling from `renderer`.
pub(super) fn open(
    device: &Device,
    format: Format,
    mut renderer: Renderer,
) -> Result<Exclusive, String> {
    let (dev, name) = find(device)?;
    let ranges: Vec<_> = dev
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .collect();
    #[cfg(target_os = "macos")]
    let ranges = integer_ranges(&ranges);
    let (config, sample) = pick(&ranges, format).ok_or_else(|| {
        format!(
            "{} Hz / {}-bit is not supported in exclusive mode",
            format.rate, format.bits
        )
    })?;
    #[cfg(target_os = "macos")]
    let hog = hog::Hog::take(&name)?;
    renderer.configure(format.rate, format.channels);
    let BufferSize::Fixed(frames) = config.buffer_size else {
        unreachable!("pick fixes the period")
    };
    let scratch = vec![0.0; frames.max(1) as usize * usize::from(format.channels)];
    let failed = Arc::new(AtomicBool::new(false));
    let args = (&dev, config, scratch, renderer, &failed);
    let stream = match sample {
        SampleFormat::I16 => build(args, |v| (to_i32(v) >> 16) as i16),
        // In range: the top 24 bits.
        SampleFormat::I24 => build(args, |v| I24::new_unchecked(to_i32(v) >> 8)),
        _ => build(args, to_i32),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    let frames = stream.buffer_size().unwrap_or(frames);
    let container = match sample {
        SampleFormat::I16 => "16-bit",
        SampleFormat::I24 => "24-in-32-bit",
        _ => "32-bit",
    };
    log::info!(
        "exclusive output: {name}, {} Hz, {container}, period {:.1} ms",
        format.rate,
        f64::from(frames) * 1000.0 / f64::from(format.rate)
    );
    Ok(Exclusive {
        format,
        stream: Some(Box::new(Cpal {
            stream,
            #[cfg(target_os = "macos")]
            _hog: hog,
            period: Duration::from_secs_f64(f64::from(frames) / f64::from(format.rate)),
            paused: false,
            failed: Arc::clone(&failed),
        })),
        failed,
        paused: false,
    })
}

/// The output device `wanted` names (`Default`: the default output's name), and that name. On
/// Linux, the one of that name on an ALSA `hw:` PCM: the others convert or mix.
fn find(wanted: &Device) -> Result<(cpal::Device, String), String> {
    let host = cpal::default_host();
    let name = match wanted {
        Device::Named(n) => n.clone(),
        Device::Default => host
            .default_output_device()
            .ok_or("no output device")?
            .description()
            .map_err(|e| e.to_string())?
            .name()
            .to_owned(),
    };
    let named: Vec<_> = host
        .output_devices()
        .map_err(|e| e.to_string())?
        .filter(|d| d.description().is_ok_and(|x| x.name() == name))
        .collect();
    if named.is_empty() {
        return Err(format!("device {name} not found"));
    }
    let hw = |d: &cpal::Device| {
        !cfg!(target_os = "linux") || d.id().is_ok_and(|id| id.id().starts_with("hw:"))
    };
    let device = named
        .into_iter()
        .find(hw)
        .ok_or("exclusive output needs a hw: device")?;
    Ok((device, name))
}

type BuildArgs<'a> = (
    &'a cpal::Device,
    StreamConfig,
    Vec<f32>,
    Renderer,
    &'a Arc<AtomicBool>,
);

/// A stream of `T`, each sample `sample(v)` of the ring's. The callback renders into `scratch`,
/// in pieces when the device asks for more, and never allocates.
fn build<T: SizedSample + 'static>(
    (device, config, mut scratch, mut renderer, failed): BuildArgs<'_>,
    sample: fn(f32) -> T,
) -> Result<cpal::Stream, cpal::Error> {
    let failed = Arc::clone(failed);
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            for out in data.chunks_mut(scratch.len()) {
                let piece = &mut scratch[..out.len()];
                renderer.render(piece);
                for (o, &v) in out.iter_mut().zip(piece.iter()) {
                    *o = sample(v);
                }
            }
        },
        move |e: cpal::Error| {
            // A glitch or no real-time priority: the device plays on.
            if !matches!(e.kind(), ErrorKind::Xrun | ErrorKind::RealtimeDenied) {
                failed.store(true, Release);
            }
        },
        None,
    )
}

/// macOS hog mode: no other process can use the device while it is held.
#[cfg(target_os = "macos")]
mod hog {
    use coreaudio::audio_unit::macos_helpers::{
        get_device_id_from_name, get_hogging_pid, toggle_hog_mode,
    };

    /// An output device in hog mode for this process; dropping it gives the device back.
    pub(super) struct Hog(u32);

    fn ours(pid: i32) -> bool {
        u32::try_from(pid) == Ok(std::process::id())
    }

    impl Hog {
        /// The output device `name` (CoreAudio's name, as cpal shows it), unless another
        /// process holds it.
        pub(super) fn take(name: &str) -> Result<Hog, String> {
            let id = get_device_id_from_name(name, false)
                .ok_or_else(|| format!("device {name} not found"))?;
            let pid = get_hogging_pid(id).map_err(|e| e.to_string())?;
            if pid != -1 && !ours(pid) {
                return Err("the device is in exclusive use by another app".into());
            }
            if !ours(pid) && !ours(toggle_hog_mode(id).map_err(|e| e.to_string())?) {
                return Err("the device refused exclusive use".into());
            }
            Ok(Hog(id))
        }
    }

    impl Drop for Hog {
        fn drop(&mut self) {
            // Toggling a device no one holds would take it: only while it is ours.
            if get_hogging_pid(self.0).is_ok_and(ours)
                && let Err(e) = toggle_hog_mode(self.0)
            {
                log::warn!("exclusive output: releasing hog mode: {e}");
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn range(
        format: SampleFormat,
        rates: (u32, u32),
        buffer: SupportedBufferSize,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(2, rates.0, rates.1, buffer, format)
    }

    fn format(rate: u32, bits: u16, channels: u16) -> Format {
        Format {
            rate,
            bits,
            channels,
        }
    }

    #[test]
    fn pick_needs_the_exact_rate_and_channels() {
        let ranges = [range(
            SampleFormat::I32,
            (44_100, 48_000),
            SupportedBufferSize::Unknown,
        )];
        let (config, sample) = pick(&ranges, format(48_000, 24, 2)).unwrap();
        assert_eq!(sample, SampleFormat::I32);
        assert_eq!(
            config,
            StreamConfig {
                channels: 2,
                sample_rate: 48_000,
                buffer_size: BufferSize::Fixed(480),
            }
        );
        assert_eq!(pick(&ranges, format(96_000, 24, 2)), None, "rate");
        assert_eq!(pick(&ranges, format(48_000, 24, 1)), None, "channels");
    }

    #[test]
    fn pick_prefers_i32_for_24_bit() {
        let any = (8_000, 192_000);
        let ranges = [
            range(SampleFormat::I24, any, SupportedBufferSize::Unknown),
            range(SampleFormat::I32, any, SupportedBufferSize::Unknown),
        ];
        let (_, sample) = pick(&ranges, format(96_000, 24, 2)).unwrap();
        assert_eq!(sample, SampleFormat::I32);
        assert_eq!(
            pick(&ranges[..1], format(96_000, 24, 2)).unwrap().1,
            SampleFormat::I24
        );
    }

    #[test]
    fn pick_prefers_i16_for_16_bit() {
        let any = (8_000, 192_000);
        let ranges = [
            range(SampleFormat::I32, any, SupportedBufferSize::Unknown),
            range(SampleFormat::I24, any, SupportedBufferSize::Unknown),
            range(SampleFormat::I16, any, SupportedBufferSize::Unknown),
        ];
        let (_, sample) = pick(&ranges, format(44_100, 16, 2)).unwrap();
        assert_eq!(sample, SampleFormat::I16);
        assert_eq!(
            pick(&ranges[..2], format(44_100, 16, 2)).unwrap().1,
            SampleFormat::I32
        );
        assert_eq!(
            pick(&ranges[1..2], format(44_100, 16, 2)).unwrap().1,
            SampleFormat::I24
        );
    }

    #[test]
    fn pick_takes_no_other_format() {
        let any = (8_000, 192_000);
        let ranges = [
            range(SampleFormat::F32, any, SupportedBufferSize::Unknown),
            range(SampleFormat::U32, any, SupportedBufferSize::Unknown),
            range(SampleFormat::I16, any, SupportedBufferSize::Unknown),
        ];
        assert_eq!(
            pick(&ranges, format(96_000, 24, 2)),
            None,
            "16-bit cannot hold 24"
        );
    }

    #[test]
    fn pick_clamps_the_period_to_the_buffer_range() {
        let pick_with = |min, max| {
            let buffer = SupportedBufferSize::Range { min, max };
            let ranges = [range(SampleFormat::I32, (44_100, 44_100), buffer)];
            pick(&ranges, format(44_100, 24, 2)).unwrap().0.buffer_size
        };
        assert_eq!(pick_with(64, 4096), BufferSize::Fixed(441));
        assert_eq!(pick_with(512, 4096), BufferSize::Fixed(512));
        assert_eq!(pick_with(64, 256), BufferSize::Fixed(256));
    }

    /// Silence, on the default device and on it by its name. On Linux the default output has
    /// to be an ALSA `hw:` device.
    #[cfg(not(windows))]
    #[test]
    #[ignore = "holds the default audio device exclusively (silence, under 1 s)"]
    fn exclusive_opens_the_default_device() {
        let name = cpal::default_host()
            .default_output_device()
            .unwrap()
            .description()
            .unwrap()
            .name()
            .to_owned();
        crate::exclusive::tests::opens_the_default_device(name);
    }
}
