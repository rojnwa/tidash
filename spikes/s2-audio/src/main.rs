//! S2 spike (throwaway): WASAPI exclusive output, gapless a->b, fMP4 FLAC decode with symphonia.
//!
//!   cargo run --release -- formats
//!   cargo run --release -- play <a.flac> <b.flac> <full.flac>
//!   cargo run --release -- decode <file> [reference [base_pts]]

use std::collections::VecDeque;
use std::fs::File;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;
use wasapi::{
    AudioClient, Device, DeviceEnumerator, Direction, SampleType, StreamMode, WasapiError,
    WaveFormat,
};

// ---------- decode ----------

struct Src {
    reader: Box<dyn FormatReader>,
    dec: Box<dyn AudioDecoder>,
    track: u32,
}

/// `seekable = false` wraps the file in a ReadOnlySource: one forward-only byte stream, like a network body.
fn open(path: &str, seekable: bool) -> Src {
    let f = File::open(path).expect(path);
    let mss = if seekable {
        MediaSourceStream::new(Box::new(f), Default::default())
    } else {
        MediaSourceStream::new(Box::new(ReadOnlySource::new(f)), Default::default())
    };
    let reader = symphonia::default::get_probe()
        .probe(&Hint::new(), mss, FormatOptions::default(), MetadataOptions::default())
        .expect("probe");
    let t = reader.default_track(TrackType::Audio).expect("no audio track");
    let p = t.codec_params.as_ref().and_then(|c| c.audio()).expect("audio params").clone();
    println!(
        "  open {path}: format={} codec={:?} rate={:?} bits={:?} ch={:?} time_base={:?} num_frames={:?}",
        reader.format_info().short_name,
        p.codec,
        p.sample_rate,
        p.bits_per_sample,
        p.channels.as_ref().map(|c| c.count()),
        t.time_base,
        t.num_frames
    );
    let dec = symphonia::default::get_codecs()
        .make_audio_decoder(&p, &AudioDecoderOptions::default())
        .expect("decoder");
    Src { track: t.id, reader, dec }
}

struct Chunk {
    pts: i64,
    rate: u32,
    ch: usize,
    samples: Vec<i32>, // interleaved, left-justified (symphonia FLAC shifts N-bit samples to the MSBs)
}

/// Next decoded packet. None = end of stream. Err = demux error (fatal) or decode error (packet skipped).
fn next(s: &mut Src) -> Option<Result<Chunk, String>> {
    loop {
        let pkt = match s.reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => return None,
            Err(e) => return Some(Err(format!("demux: {e}"))),
        };
        if pkt.track_id != s.track {
            continue;
        }
        return Some(match s.dec.decode(&pkt) {
            Ok(buf) => {
                let mut samples = Vec::new();
                buf.copy_to_vec_interleaved(&mut samples);
                Ok(Chunk { pts: pkt.pts.get(), rate: buf.spec().rate(), ch: buf.spec().channels().count(), samples })
            }
            Err(e) => Err(format!("decode @pts {}: {e}", pkt.pts.get())),
        });
    }
}

struct Decoded {
    samples: Vec<i32>,
    first_pts: i64,
    rate: u32,
    ch: usize,
    errors: Vec<String>,
}

fn decode_all(path: &str, seekable: bool) -> Decoded {
    let mut s = open(path, seekable);
    let mut d = Decoded { samples: Vec::new(), first_pts: -1, rate: 0, ch: 0, errors: Vec::new() };
    while let Some(r) = next(&mut s) {
        match r {
            Ok(c) => {
                if d.first_pts < 0 {
                    (d.first_pts, d.rate, d.ch) = (c.pts, c.rate, c.ch);
                }
                d.samples.extend(c.samples);
            }
            Err(e) => {
                let fatal = e.starts_with("demux");
                d.errors.push(e);
                if fatal {
                    break;
                }
            }
        }
    }
    d
}

fn first_diff(a: &[i32], b: &[i32]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y).or((a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Seek `s` to `ts` (frames), decode one packet, compare it with the linear decode at the same position.
fn seek_check(s: &mut Src, lin: &Decoded, ts: i64, label: &str) {
    let r = s.reader.seek(SeekMode::Accurate, SeekTo::Timestamp { ts: Timestamp::new(ts), track_id: s.track });
    s.dec.reset();
    match r {
        Err(e) => println!("  seek {label} to ts {ts}: ERROR {e}"),
        Ok(to) => match next(s) {
            Some(Ok(c)) => {
                let off = (c.pts - lin.first_pts) as usize * lin.ch;
                let want = lin.samples.get(off..off + c.samples.len());
                println!(
                    "  seek {label} to ts {ts}: required={} actual={} first packet pts={} frames={} matches linear decode={}",
                    to.required_ts.get(),
                    to.actual_ts.get(),
                    c.pts,
                    c.samples.len() / lin.ch,
                    want == Some(&c.samples[..])
                );
            }
            other => println!("  seek {label} to ts {ts}: seek ok but next packet = {:?}", other.map(|r| r.err())),
        },
    }
}

fn decode_cmd(path: &str, reference: Option<&str>, base_pts: i64) {
    println!("decode {path} as ONE forward-only byte stream (ReadOnlySource):");
    let lin = decode_all(path, false);
    let frames = lin.samples.len() / lin.ch.max(1);
    println!(
        "  rate={} ch={} decoded frames={} ({:.3} s) first_pts={} decode errors={} {:?}",
        lin.rate,
        lin.ch,
        frames,
        frames as f64 / lin.rate.max(1) as f64,
        lin.first_pts,
        lin.errors.len(),
        lin.errors.first()
    );
    let low_bits_zero = lin.samples.iter().all(|s| s & 0xff == 0);
    println!("  all samples have low 8 bits zero (<=24 significant bits): {low_bits_zero}");
    // FNV-1a 64 over the s32le bytes; compare with the same hash of `ffmpeg -f s32le` output.
    let fnv = lin.samples.iter().flat_map(|s| s.to_le_bytes()).fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    println!("  FNV-1a64 of s32le samples: {fnv:016x}");
    if let Some(r) = reference {
        let rf = decode_all(r, true);
        // A stream that starts mid-track (init + segment k) is compared against the reference from
        // `base_pts` on: symphonia 0.6.1 ignores tfdt, so its pts restart at 0 (the caller supplies tfdt).
        let off = (lin.first_pts + base_pts - rf.first_pts) as usize * rf.ch;
        let d = first_diff(&lin.samples, &rf.samples[off..]);
        println!(
            "  vs reference {r} from pts {}: frames {} vs {}, SAMPLE-EQUAL={}{}",
            lin.first_pts + base_pts,
            frames,
            (rf.samples.len() - off) / rf.ch.max(1),
            d.is_none(),
            d.map(|i| format!(" (first diff at sample {i})")).unwrap_or_default()
        );
        assert!(d.is_none(), "fMP4 decode differs from reference");
    }
    let (mid, early) = (lin.first_pts + frames as i64 / 2, lin.first_pts + frames as i64 / 10);
    println!("seek tests, seekable File source (forward to 50%, then backward to 10%):");
    let mut s = open(path, true);
    seek_check(&mut s, &lin, mid, "fwd");
    seek_check(&mut s, &lin, early, "back");
    println!("seek tests, forward-only ReadOnlySource:");
    let mut s = open(path, false);
    seek_check(&mut s, &lin, mid, "fwd");
    seek_check(&mut s, &lin, early, "back");
}

// ---------- WASAPI ----------

#[derive(Clone, Copy, Debug)]
enum Fmt {
    Packed24,
    In32Valid24,
    Int32,
    Int16, // probed in `formats` only (TIDAL LOSSLESS is 16/44.1); never chosen for playback
}

impl Fmt {
    const ALL: [Fmt; 3] = [Fmt::Packed24, Fmt::In32Valid24, Fmt::Int32];
    fn wave(self, rate: usize) -> WaveFormat {
        let (store, valid) = match self {
            Fmt::Packed24 => (24, 24),
            Fmt::In32Valid24 => (32, 24),
            Fmt::Int32 => (32, 32),
            Fmt::Int16 => (16, 16),
        };
        WaveFormat::new(store, valid, &SampleType::Int, rate, 2, None)
    }
    /// Left-justified i32 -> device bytes. Lossless for <=24-bit content (low byte is zero).
    fn push(self, s: i32, out: &mut Vec<u8>) {
        let b = s.to_le_bytes();
        match self {
            Fmt::Packed24 => out.extend_from_slice(&b[1..]),
            Fmt::In32Valid24 | Fmt::Int32 => out.extend_from_slice(&b),
            Fmt::Int16 => out.extend_from_slice(&b[2..]),
        }
    }
}

fn default_device() -> Device {
    wasapi::initialize_mta().ok().expect("COM init");
    DeviceEnumerator::new().expect("enumerator").get_default_device(&Direction::Render).expect("default render device")
}

/// Exclusive, event-driven init at the device default period (128-byte aligned), retrying once on
/// AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED as Microsoft documents. Returns the client and period (100 ns).
fn init_exclusive(dev: &Device, wf: &WaveFormat) -> Result<(AudioClient, i64), WasapiError> {
    let mut c = dev.get_iaudioclient()?;
    let (def, _min) = c.get_device_period()?;
    let period = c.calculate_aligned_period_near(def, Some(128), wf)?;
    match c.initialize_client(wf, &Direction::Render, &StreamMode::EventsExclusive { period_hns: period }) {
        Ok(()) => Ok((c, period)),
        Err(WasapiError::Windows(e)) if e.code().0 as u32 == 0x8889_0019 => {
            let frames = c.get_buffer_size()?;
            let period = wasapi::calculate_period_100ns(frames as i64, wf.get_samplespersec() as i64);
            let mut c = dev.get_iaudioclient()?;
            c.initialize_client(wf, &Direction::Render, &StreamMode::EventsExclusive { period_hns: period })?;
            Ok((c, period))
        }
        Err(e) => Err(e),
    }
}

fn describe(wf: &WaveFormat) -> String {
    format!(
        "{} Hz, {} ch, container {} bit, valid {} bit, blockalign {}, mask {:#x}, subformat {:?}",
        wf.get_samplespersec(),
        wf.get_nchannels(),
        wf.get_bitspersample(),
        wf.get_validbitspersample(),
        wf.get_blockalign(),
        wf.get_dwchannelmask(),
        wf.get_subformat().ok()
    )
}

fn formats_cmd() {
    let dev = default_device();
    println!("default render device: {}", dev.get_friendlyname().unwrap_or_default());
    let c = dev.get_iaudioclient().expect("client");
    println!("shared mix format: {}", c.get_mixformat().map(|f| describe(&f)).unwrap_or_default());
    println!("device period default/min (100ns): {:?}", c.get_device_period().ok());
    if let Ok(col) = DeviceEnumerator::new().and_then(|e| e.get_device_collection(&Direction::Render)) {
        for i in 0..col.get_nbr_devices().unwrap_or(0) {
            if let Ok(d) = col.get_device_at_index(i) {
                println!("  other render endpoint: {}", d.get_friendlyname().unwrap_or_default());
            }
        }
    }
    for rate in [44100, 48000, 88200, 96000, 176400, 192000] {
        for f in Fmt::ALL.into_iter().chain([Fmt::Int16]) {
            let res = match c.is_supported_exclusive_with_quirks(&f.wave(rate)) {
                Err(_) => "REFUSED by IsFormatSupported(EXCLUSIVE)".to_string(),
                Ok(wf) => match init_exclusive(&dev, &wf) {
                    Ok((cl, p)) => format!(
                        "ACCEPTED, Initialize ok (period {p} hns, buffer {} frames) [{}]",
                        cl.get_buffer_size().unwrap_or(0),
                        describe(&wf)
                    ),
                    Err(e) => format!("IsFormatSupported ok but Initialize FAILED: {e}"),
                },
            };
            println!("{rate:>6} Hz {f:<12?} {res}");
        }
    }
}

/// Pull from the decoder channel until `q` holds `need` samples. Returns true once the decoder is finished.
fn top_up(rx: &Receiver<Vec<i32>>, q: &mut VecDeque<i32>, need: usize, block: bool) -> bool {
    while q.len() < need {
        let r = if block { rx.recv().map_err(|_| TryRecvError::Disconnected) } else { rx.try_recv() };
        match r {
            Ok(v) => q.extend(v),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return true,
        }
    }
    false
}

fn play_cmd(a: &str, b: &str, full: &str) {
    println!("reference decode:");
    let expected = decode_all(full, true);
    assert!(expected.errors.is_empty(), "{:?}", expected.errors);
    assert!(expected.samples.iter().all(|s| s & 0xff == 0), "source has more than 24 significant bits");
    let (rate, ch) = (expected.rate as usize, expected.ch);
    assert_eq!(ch, 2);

    let dev = default_device();
    println!("device: {}", dev.get_friendlyname().unwrap_or_default());
    let probe = dev.get_iaudioclient().expect("client");
    let mut chosen = None;
    for f in Fmt::ALL {
        match probe.is_supported_exclusive_with_quirks(&f.wave(rate)) {
            Ok(wf) if chosen.is_none() => chosen = Some((f, wf)),
            Ok(_) => println!("  {f:?} also accepted"),
            Err(_) => println!("  {f:?} REFUSED at {rate} Hz"),
        }
    }
    let (fmt, wf) = chosen.expect("device accepts no 24-bit-capable exclusive format at this rate");
    let (client, period) = init_exclusive(&dev, &wf).expect("exclusive init");
    let buf_frames = client.get_buffer_size().expect("buffer size") as usize;
    let period_dur = Duration::from_nanos(period as u64 * 100);
    println!("NEGOTIATED exclusive event-driven: {fmt:?} = {}", describe(&wf));
    println!("  period {period} hns ({period_dur:?}), buffer {buf_frames} frames");
    let h = client.set_get_eventhandle().expect("event handle");
    let render = client.get_audiorenderclient().expect("render client");
    let clock = client.get_audioclock().expect("clock");

    // Decoder thread: a then b into a small bounded queue (4 packets), so b is opened while the device is
    // at most ~4 packets ahead -- the real player's shape.
    let (tx, rx) = sync_channel::<Vec<i32>>(4);
    let paths = [a.to_string(), b.to_string()];
    let dec = std::thread::spawn(move || {
        let mut open_b = Duration::ZERO;
        for (i, p) in paths.iter().enumerate() {
            let t = Instant::now();
            let mut s = open(p, true);
            if i == 1 {
                open_b = t.elapsed();
            }
            while let Some(c) = next(&mut s) {
                tx.send(c.expect("decode").samples).expect("send");
            }
        }
        open_b
    });

    let need = buf_frames * ch;
    let block_bytes = buf_frames * wf.get_blockalign() as usize;
    let mut q = VecDeque::new();
    let mut written: Vec<i32> = Vec::with_capacity(expected.samples.len());
    let mut bytes = Vec::with_capacity(block_bytes);
    let (mut fin, mut starved, mut late, mut timeouts, mut events, mut tail) = (false, 0, 0, 0, 0, 0);
    let (mut late_wakes, mut late_wake_at, mut early) = (0, Vec::new(), Vec::new());
    let (mut startup, mut clock_running, mut max_interval, mut max_service) =
        (Vec::new(), false, Duration::ZERO, Duration::ZERO);
    let (mut pad_min, mut pad_max) = (u32::MAX, 0);
    let late_wake_limit = period_dur * 3 / 2;
    let mut blocks = 0usize; // device buffers handed to WASAPI (real data + silence pad)

    // One device buffer from the queue; silence-pad only after the last real sample.
    let mut write_block = |q: &mut VecDeque<i32>, fin: bool, starved: &mut u32| {
        blocks += 1;
        let have = q.len().min(need);
        if have < need && !fin {
            *starved += 1;
        }
        bytes.clear();
        for s in q.drain(..have) {
            written.push(s);
            fmt.push(s, &mut bytes);
        }
        bytes.resize(block_bytes, 0);
        render.write_to_device(buf_frames, &bytes, None).expect("write");
    };

    fin |= top_up(&rx, &mut q, need, true); // prefill before Start, per MS "Rendering a Stream"
    write_block(&mut q, fin, &mut starved);
    client.start_stream().expect("start");
    let t0 = Instant::now();
    let mut last = t0;
    loop {
        if h.wait_for_event(1000).is_err() {
            timeouts += 1;
            break;
        }
        let woke = Instant::now();
        events += 1;
        let interval = woke - last;
        last = woke;
        // An interval is mid-stream only if the device clock was already running at the previous wake.
        // Before that it contains device start-up: Start signals event 1 at once (fill the second half of
        // the ping-pong) and the USB device needs a few ms before it clocks the first frame.
        let running_before = clock_running;
        if !clock_running || events <= 3 {
            let (p, _) = clock.get_position().expect("clock position");
            let f = clock.get_frequency().expect("clock frequency");
            clock_running = p > 0;
            if events <= 3 {
                early.push((events, interval, (p as f64 / f as f64 * rate as f64).round() as i64));
            }
        }
        if running_before {
            max_interval = max_interval.max(interval);
            if interval > late_wake_limit {
                late_wakes += 1;
                late_wake_at.push((events, interval));
            }
        } else {
            startup.push((events, interval));
        }
        // Informational only: in exclusive event mode this reads the full buffer at every wake.
        let pad = client.get_current_padding().expect("padding");
        (pad_min, pad_max) = (pad_min.min(pad), pad_max.max(pad));
        fin |= top_up(&rx, &mut q, need, false);
        let data_remains = !(fin && q.is_empty());
        if !data_remains {
            tail += 1; // let the last real buffer drain, then stop
            if tail > 2 {
                break;
            }
        }
        write_block(&mut q, fin, &mut starved);
        let service = woke.elapsed();
        max_service = max_service.max(service);
        if service > period_dur {
            late += 1;
        }
    }
    let wall = t0.elapsed();
    client.stop_stream().expect("stop");
    // After Stop the clock and the queue are frozen: frames the device actually clocked out must equal
    // frames handed to it minus frames still queued. Any replayed/inserted frames (a glitch) would show
    // up as a positive delta.
    let (pos, _) = clock.get_position().expect("clock position");
    let freq = clock.get_frequency().expect("clock frequency");
    let queued = client.get_current_padding().expect("padding at stop") as i64;
    let played = (pos as f64 / freq as f64 * rate as f64).round() as i64;
    let handed = (blocks * buf_frames) as i64;
    let real = (written.len() / ch) as i64;
    let open_b = dec.join().expect("decoder thread");

    let underruns = starved + late + late_wakes + timeouts;
    let diff = first_diff(&written, &expected.samples);
    println!("RESULT {rate} Hz {fmt:?}:");
    let startup_late = startup.iter().filter(|(e, i)| *e > 1 && *i > late_wake_limit).count();
    println!("  events {events}, wall {wall:?}");
    println!(
        "  start-up wakes, before the device clock ran (event, interval since previous wake/Start): {startup:?}; \
         of these after event 1 and > 1.5 x period: {startup_late} (not counted as underruns)"
    );
    println!(
        "  device clock {:.4} s = {played} frames; handed to device {handed} (real {real} = {:.4} s, silence pad {}); \
         queued at stop {queued}; clock - (handed - queued) = {} frames",
        pos as f64 / freq as f64,
        real as f64 / rate as f64,
        handed - real,
        played - (handed - queued)
    );
    println!("  opening b (probe + decoder) took {open_b:?} on the decoder thread");
    println!("  first wakes (event, interval since previous/Start, device clock frames at wake): {early:?}");
    println!(
        "  mid-stream (device clock running) interval max {max_interval:?}; late wakes (> {late_wake_limit:?} = 1.5 x period) {late_wakes} at (event, interval) {late_wake_at:?}"
    );
    println!("  service time max {max_service:?} (deadline = period {period_dur:?})");
    println!("  padding at wake (informational, = buffer in exclusive event mode): min {pad_min} max {pad_max} frames");
    println!(
        "  UNDERRUNS = {underruns} (starved {starved}, service > period {late}, late wakes {late_wakes}, event timeouts {timeouts})"
    );
    println!(
        "  frames written {} vs full.flac {}; SAMPLE-EQUAL={}{}",
        written.len() / ch,
        expected.samples.len() / ch,
        diff.is_none(),
        diff.map(|i| format!(" (first diff at sample {i})")).unwrap_or_default()
    );
    assert!(diff.is_none(), "written stream differs from full.flac");
    assert_eq!(underruns, 0, "underruns");
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["formats"] => formats_cmd(),
        ["play", x, y, full] => play_cmd(x, y, full),
        ["decode", f] => decode_cmd(f, None, 0),
        ["decode", f, r] => decode_cmd(f, Some(r), 0),
        ["decode", f, r, base] => decode_cmd(f, Some(r), base.parse().expect("base pts")),
        _ => eprintln!("usage: formats | play <a> <b> <full> | decode <file> [reference [base_pts]]"),
    }
}
