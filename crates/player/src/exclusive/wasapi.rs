//! Windows: WASAPI exclusive, event-driven, at the track's own format.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::{Acquire, Release};
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fastframe_audio::{Device, Render};
use wasapi::{
    AudioClient, AudioRenderClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode,
    WasapiError, WaveFormat,
};

use super::{Exclusive, Stream, to_i32};
use crate::{Format, Renderer};

/// How long `open` waits for the device.
const OPEN_TIMEOUT_MS: u32 = 2000;
/// How long the render thread waits for an event while running (a period is about 10 ms):
/// past it, the device is gone, and a close waits no longer than that.
const EVENT_TIMEOUT_MS: u32 = 500;
/// `IAudioClient::Initialize`'s AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED.
const BUFFER_SIZE_NOT_ALIGNED: u32 = 0x8889_0019;
/// Integer containers, `(store bits, valid bits)`, smallest first.
const CONTAINERS: [(u16, u16); 4] = [(16, 16), (24, 24), (32, 24), (32, 32)];

/// The containers that hold a `bits`-bit sample unchanged, smallest first.
fn candidates(bits: u16) -> impl Iterator<Item = (u16, u16)> {
    CONTAINERS
        .into_iter()
        .filter(move |&(_, valid)| valid >= bits)
}

/// `samples` in the device's little-endian container of `n` bytes each: the top `n` bytes of
/// each left-justified sample.
fn pack(samples: &[f32], out: &mut [u8], n: usize) {
    for (v, o) in samples.iter().zip(out.chunks_exact_mut(n)) {
        o.copy_from_slice(&to_i32(*v).to_le_bytes()[4 - n..]);
    }
}

/// What `Exclusive` tells the render thread.
#[derive(Default)]
struct Ctl {
    paused: AtomicBool,
    stop: AtomicBool,
}

/// An open stream: the render thread that holds the device.
struct Wasapi {
    ctl: Arc<Ctl>,
    thread: Option<JoinHandle<()>>,
}

impl Stream for Wasapi {
    fn pause(&mut self) {
        self.ctl.paused.store(true, Release);
    }

    fn resume(&mut self) {
        self.ctl.paused.store(false, Release);
        if let Some(t) = &self.thread {
            t.thread().unpark();
        }
    }
}

impl Drop for Wasapi {
    /// Returns once the thread is gone: the renderer is never rendered after.
    fn drop(&mut self) {
        self.ctl.stop.store(true, Release);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// Holds `device` at `format` on a thread of its own, pulling from `renderer`.
pub(super) fn open(
    device: &Device,
    format: Format,
    mut renderer: Renderer,
) -> Result<Exclusive, String> {
    renderer.configure(format.rate, format.channels);
    let ctl = Arc::new(Ctl::default());
    let failed = Arc::new(AtomicBool::new(false));
    // A rendezvous: the thread knows whether `open` took its answer or gave up waiting.
    let (ready, opened) = mpsc::sync_channel(0);
    let thread = thread::Builder::new()
        .name("exclusive-output".into())
        .spawn({
            let (device, ctl, failed) = (device.clone(), Arc::clone(&ctl), Arc::clone(&failed));
            move || run(&device, format, renderer, &ctl, &failed, &ready)
        })
        .map_err(|e| e.to_string())?;
    match opened.recv_timeout(Duration::from_millis(OPEN_TIMEOUT_MS.into())) {
        Ok(Ok(())) => Ok(Exclusive {
            format,
            stream: Some(Box::new(Wasapi {
                ctl,
                thread: Some(thread),
            })),
            failed,
            paused: false,
        }),
        Ok(Err(reason)) => Err(reason),
        Err(_) => Err("the device did not answer".into()),
    }
}

/// The render thread: the device in the COM MTA, then the loop until dropped or failed.
fn run(
    device: &Device,
    format: Format,
    renderer: Renderer,
    ctl: &Ctl,
    failed: &AtomicBool,
    ready: &SyncSender<Result<(), String>>,
) {
    if let Err(e) = wasapi::initialize_mta().ok() {
        let _ = ready.send(Err(e.to_string()));
        return;
    }
    match Opened::new(device, format) {
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
        }
        // When `open` gave up waiting, the renderer is never rendered.
        Ok(o) if ready.send(Ok(())).is_ok() => {
            o.log();
            if let Err(e) = o.play(renderer, ctl) {
                log::warn!("exclusive output: {e}");
                failed.store(true, Release);
            }
        }
        Ok(_) => {}
    }
    wasapi::deinitialize();
}

/// The device, initialized and not started, with the loop's buffers.
struct Opened {
    name: String,
    client: AudioClient,
    out: AudioRenderClient,
    event: Handle,
    rate: u32,
    /// `(store bits, valid bits)`.
    container: (u16, u16),
    /// Device buffer, one period.
    frames: usize,
    samples: Vec<f32>,
    bytes: Vec<u8>,
}

impl Opened {
    fn new(device: &Device, format: Format) -> Result<Opened, Box<dyn Error>> {
        let Format {
            rate,
            bits,
            channels,
        } = format;
        let devices = DeviceEnumerator::new()?;
        let dev = match device {
            Device::Default => devices.get_default_device(&Direction::Render)?,
            Device::Named(n) => devices
                .get_device_collection(&Direction::Render)
                .and_then(|all| all.get_device_with_name(n))
                .map_err(|_| format!("device {n} not found"))?,
        };
        let probe = dev.get_iaudioclient()?;
        let (container, wf) = candidates(bits)
            .find_map(|(store, valid)| {
                let wf = WaveFormat::new(
                    store.into(),
                    valid.into(),
                    &SampleType::Int,
                    rate as usize,
                    channels.into(),
                    None,
                );
                let wf = probe.is_supported_exclusive_with_quirks(&wf).ok()?;
                Some(((store, valid), wf))
            })
            .ok_or_else(|| format!("{rate} Hz / {bits}-bit is not supported in exclusive mode"))?;
        let client = init_exclusive(&dev, &wf)?;
        let event = client.set_get_eventhandle()?;
        let out = client.get_audiorenderclient()?;
        let frames = client.get_buffer_size()? as usize;
        let samples = vec![0.0; frames * usize::from(channels)];
        let bytes = vec![0; samples.len() * usize::from(container.0 / 8)];
        Ok(Opened {
            name: dev.get_friendlyname()?,
            client,
            out,
            event,
            rate,
            container,
            frames,
            samples,
            bytes,
        })
    }

    fn period_ms(&self) -> f64 {
        self.frames as f64 * 1000.0 / f64::from(self.rate)
    }

    fn log(&self) {
        let (store, valid) = self.container;
        let container = if store == valid {
            format!("{store}-bit")
        } else {
            format!("{valid}-in-{store}-bit")
        };
        log::info!(
            "exclusive output: {}, {} Hz, {container}, period {:.1} ms",
            self.name,
            self.rate,
            self.period_ms()
        );
    }

    /// Pre-fills one buffer and starts, then writes one buffer per event. Paused, it stops,
    /// drops what is queued and parks until resumed. Stopped (dropped), it lets the device
    /// play out what was written, then stops.
    fn play(mut self, mut renderer: Renderer, ctl: &Ctl) -> Result<(), WasapiError> {
        loop {
            self.write(Some(&mut renderer))?;
            self.client.start_stream()?;
            loop {
                self.event.wait_for_event(EVENT_TIMEOUT_MS)?;
                if ctl.stop.load(Acquire) {
                    // Silence after the last buffer, so the device plays it out, not stale data.
                    self.write(None)?;
                    let _ = self
                        .event
                        .wait_for_event(self.period_ms().ceil() as u32 + 1);
                    return self.client.stop_stream();
                }
                if ctl.paused.load(Acquire) {
                    break;
                }
                self.write(Some(&mut renderer))?;
            }
            self.client.stop_stream()?;
            self.client.reset_stream()?;
            while ctl.paused.load(Acquire) && !ctl.stop.load(Acquire) {
                thread::park();
            }
            if ctl.stop.load(Acquire) {
                return Ok(());
            }
        }
    }

    /// One device buffer: rendered and packed into the container, or silence (`None`).
    fn write(&mut self, renderer: Option<&mut Renderer>) -> Result<(), WasapiError> {
        match renderer {
            Some(r) => {
                r.render(&mut self.samples);
                pack(
                    &self.samples,
                    &mut self.bytes,
                    usize::from(self.container.0 / 8),
                );
            }
            None => self.bytes.fill(0),
        }
        self.out.write_to_device(self.frames, &self.bytes, None)
    }
}

/// Exclusive and event-driven at the device's default period (aligned to 128 bytes); on
/// AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, once more at the aligned size, as Microsoft documents.
/// (The S2 spike's `init_exclusive`.)
fn init_exclusive(dev: &wasapi::Device, wf: &WaveFormat) -> Result<AudioClient, WasapiError> {
    let mut client = dev.get_iaudioclient()?;
    let (default, _min) = client.get_device_period()?;
    let period = client.calculate_aligned_period_near(default, Some(128), wf)?;
    let mode = |period_hns| StreamMode::EventsExclusive { period_hns };
    match client.initialize_client(wf, &Direction::Render, &mode(period)) {
        Ok(()) => Ok(client),
        Err(WasapiError::Windows(e)) if e.code().0 as u32 == BUFFER_SIZE_NOT_ALIGNED => {
            let frames = client.get_buffer_size()?;
            let period =
                wasapi::calculate_period_100ns(frames.into(), wf.get_samplespersec().into());
            let mut client = dev.get_iaudioclient()?;
            client.initialize_client(wf, &Direction::Render, &mode(period))?;
            Ok(client)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The ring's `f32` for a left-justified integer sample.
    fn sample(i: i32) -> f32 {
        i as f32 / 2_147_483_648.0
    }

    fn packed(i: i32, n: usize) -> Vec<u8> {
        let mut out = vec![0; n];
        pack(&[sample(i)], &mut out, n);
        out
    }

    #[test]
    fn container_bytes() {
        assert_eq!(packed(0x1234_5600, 3), [0x56, 0x34, 0x12], "packed 24");
        assert_eq!(
            packed(0x1234_5600, 4),
            [0x00, 0x56, 0x34, 0x12],
            "24-in-32 and 32"
        );
        assert_eq!(packed(0x1234_0000, 2), [0x34, 0x12], "16");
    }

    #[test]
    fn candidates_by_bits() {
        let all = |bits| candidates(bits).collect::<Vec<_>>();
        assert_eq!(all(16), [(16, 16), (24, 24), (32, 24), (32, 32)]);
        assert_eq!(all(24), [(24, 24), (32, 24), (32, 32)]);
    }

    /// Silence, on the default device and on it by its name (as fastframe lists it).
    #[test]
    #[ignore = "holds the default audio device exclusively (silence, under 1 s)"]
    fn exclusive_opens_the_default_device() {
        let _ = wasapi::initialize_mta();
        let name = DeviceEnumerator::new()
            .and_then(|e| e.get_default_device(&Direction::Render))
            .and_then(|d| d.get_friendlyname())
            .unwrap();
        crate::exclusive::tests::opens_the_default_device(name);
    }
}
