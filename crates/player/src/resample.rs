//! Fixed-ratio resampler (shared-mode output: track rate -> device mix rate).

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

/// Input chunk size hint, in frames. `FixedSync::Both` rounds it up to a whole
/// number of FFT sub-chunks (a multiple of 147 for 44.1 -> 48 kHz).
const CHUNK_HINT: usize = 1024;

/// Interleaved f32 resampler. `from == to` is a bit-exact passthrough.
///
/// Uses rubato's FFT synchronous resampler: for a fixed rational ratio it is
/// high quality, cheaper than sinc, and has a fixed input/output chunk size.
pub struct Resampler {
    inner: Option<Inner>,
}

struct Inner {
    fft: Fft<f32>,
    ch: usize,
    ratio: f64,
    /// Buffered input not yet forming a full chunk (interleaved).
    pending: Vec<f32>,
    out_buf: Vec<f32>,
    in_buf: Vec<f32>,
    /// Startup-delay output frames still to drop.
    skip: usize,
    in_frames: u64,
    out_frames: u64,
}

impl Resampler {
    #[must_use]
    pub fn new(from: u32, to: u32, channels: u16) -> Resampler {
        let ch = usize::from(channels.max(1));
        if from == to {
            return Resampler { inner: None };
        }
        let Ok(fft) = Fft::<f32>::new(from as usize, to as usize, CHUNK_HINT, ch, FixedSync::Both)
        else {
            // ponytail: only a zero rate can fail here; release falls back to passthrough.
            debug_assert!(false, "invalid sample rates {from} -> {to}");
            return Resampler { inner: None };
        };
        let in_buf = vec![0.0; fft.input_frames_max() * ch];
        let out_buf = vec![0.0; fft.output_frames_max() * ch];
        Resampler {
            inner: Some(Inner {
                skip: fft.output_delay(),
                ratio: f64::from(to) / f64::from(from),
                pending: Vec::with_capacity(in_buf.len() * 2),
                fft,
                ch,
                in_buf,
                out_buf,
                in_frames: 0,
                out_frames: 0,
            }),
        }
    }

    #[must_use]
    pub fn is_passthrough(&self) -> bool {
        self.inner.is_none()
    }

    /// Append resampled frames for `input` (interleaved) to `out`; partial
    /// chunks are buffered until the next call.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let Some(s) = &mut self.inner else {
            out.extend_from_slice(input);
            return;
        };
        debug_assert!(
            input.len().is_multiple_of(s.ch),
            "input not a whole number of frames"
        );
        s.pending.extend_from_slice(input);
        s.in_frames += (input.len() / s.ch) as u64;
        loop {
            let need = s.fft.input_frames_next() * s.ch;
            if s.pending.len() < need {
                break;
            }
            s.in_buf[..need].copy_from_slice(&s.pending[..need]);
            s.pending.drain(..need);
            if !s.run(None, out) {
                break;
            }
        }
    }

    /// Emit the buffered tail so total output is ~ input frames * to/from.
    /// The resampler is left drained; do not reuse it for another stream.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let Some(s) = &mut self.inner else { return };
        let target = (s.in_frames as f64 * s.ratio).round() as u64;
        let ch = s.ch;
        let mut left = s.pending.len() / ch;
        let base = out.len();
        // Zero-padded final chunk(s); enough of them to push the filter delay out.
        // Iterations are capped at what the buffered input plus remaining delay can
        // need, so a zero-output chunk or an unreachable target can never spin.
        let (in_next, out_next) = (
            s.fft.input_frames_next().max(1),
            s.fft.output_frames_next().max(1),
        );
        let max_iters = left.div_ceil(in_next) + s.skip.div_ceil(out_next) + 3;
        for _ in 0..max_iters {
            if s.out_frames >= target {
                break;
            }
            let n = left.min(s.fft.input_frames_next());
            s.in_buf[..n * ch].copy_from_slice(&s.pending[..n * ch]);
            s.pending.drain(..n * ch);
            left -= n;
            if !s.run(Some(n), out) {
                break; // rubato error: stop rather than spin
            }
        }
        let extra = s.out_frames.saturating_sub(target);
        out.truncate(out.len().saturating_sub(extra as usize * ch).max(base));
        s.out_frames = s.out_frames.min(target);
    }
}

impl Inner {
    /// Resample one chunk from `in_buf`, dropping startup delay, appending to `out`.
    fn run(&mut self, partial: Option<usize>, out: &mut Vec<f32>) -> bool {
        let ch = self.ch;
        let in_frames = self.in_buf.len() / ch;
        let out_frames = self.out_buf.len() / ch;
        let (Ok(inp), Ok(mut outp)) = (
            InterleavedSlice::new(&self.in_buf, ch, in_frames),
            InterleavedSlice::new_mut(&mut self.out_buf, ch, out_frames),
        ) else {
            return false;
        };
        let idx = partial.map(|n| Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: Some(n),
            active_channels_mask: None,
        });
        let Ok((_, produced)) = self.fft.process_into_buffer(&inp, &mut outp, idx.as_ref()) else {
            return false;
        };
        let skipped = self.skip.min(produced);
        self.skip -= skipped;
        out.extend_from_slice(&self.out_buf[skipped * ch..produced * ch]);
        self.out_frames += (produced - skipped) as u64;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, rate: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin();
                [s, s]
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn passthrough_is_identity() {
        let mut r = Resampler::new(48_000, 48_000, 2);
        assert!(r.is_passthrough());
        let input = sine(1000, 48_000.0);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn ratio_44100_to_48000_length() {
        let mut r = Resampler::new(44_100, 48_000, 2);
        assert!(!r.is_passthrough());
        let mut out = Vec::new();
        r.process(&sine(44_100, 44_100.0), &mut out);
        r.flush(&mut out);
        let frames = out.len() / 2;
        assert!((47_520..=48_480).contains(&frames), "frames = {frames}");
    }

    #[test]
    fn sine_amplitude_preserved() {
        let input = sine(44_100, 44_100.0);
        let mut r = Resampler::new(44_100, 48_000, 2);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        let mid = |x: &[f32]| x[x.len() / 4..x.len() * 3 / 4].to_vec();
        let (a, b) = (rms(&mid(&input)), rms(&mid(&out)));
        assert!((b / a - 1.0).abs() < 0.01, "rms ratio = {}", b / a);
    }

    #[test]
    fn chunked_equals_whole() {
        let input = sine(44_100, 44_100.0);
        let mut whole = Vec::new();
        let mut r = Resampler::new(44_100, 48_000, 2);
        r.process(&input, &mut whole);
        r.flush(&mut whole);

        let mut chunked = Vec::new();
        let mut r = Resampler::new(44_100, 48_000, 2);
        let n = input.len() / 2;
        // 37 chunks of odd frame counts; last takes the rest.
        let mut pos = 0;
        for i in 0..37 {
            let frames = if i == 36 { n - pos } else { 1001 + 2 * i };
            r.process(&input[pos * 2..(pos + frames) * 2], &mut chunked);
            pos += frames;
        }
        r.flush(&mut chunked);
        let diff = (whole.len() as i64 - chunked.len() as i64).abs() / 2;
        assert!(
            diff <= 1024,
            "whole {} vs chunked {}",
            whole.len() / 2,
            chunked.len() / 2
        );
    }

    #[test]
    fn channels_stay_separate() {
        let input: Vec<f32> = sine(44_100, 44_100.0)
            .chunks(2)
            .flat_map(|f| [f[0], 0.0])
            .collect();
        let mut r = Resampler::new(44_100, 48_000, 2);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        let ch =
            |x: &[f32], c: usize| -> Vec<f32> { x.iter().skip(c).step_by(2).copied().collect() };
        let (l_in, l_out, r_out) = (ch(&input, 0), ch(&out, 0), ch(&out, 1));
        assert!(rms(&r_out) < 1e-4, "R rms = {}", rms(&r_out));
        assert!((rms(&l_out) / rms(&l_in) - 1.0).abs() < 0.01);
    }

    #[test]
    fn flush_terminates_when_empty_or_tiny() {
        let mut out = Vec::new();
        let mut r = Resampler::new(44_100, 48_000, 2);
        r.flush(&mut out);
        assert!(out.is_empty());
        let mut r = Resampler::new(44_100, 48_000, 2);
        r.process(&[0.1; 20], &mut out); // 10 frames
        r.flush(&mut out);
        assert_eq!(out.len() / 2, 11); // round(10 * 48000/44100)
    }

    #[test]
    fn flush_stops_when_target_unreachable() {
        let mut r = Resampler::new(44_100, 48_000, 2);
        let mut out = Vec::new();
        r.process(&[0.1; 20], &mut out);
        // Corrupt bookkeeping so the target can never be reached.
        if let Some(i) = r.inner.as_mut() {
            i.in_frames = u64::MAX / 4;
        }
        r.flush(&mut out); // must terminate without panic or underflow
        assert!(out.len() < 100_000);
    }
}
