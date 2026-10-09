//! A loaded track: its download, decoder and resampler, and the samples not yet in the ring.

use std::time::{Duration, Instant};

use crate::{DecodeError, Decoder, Pcm, ProgressiveFile, Resampler};

pub(crate) struct Track {
    pub(crate) id: u64,
    /// The download; the decoder reads a forward-only view of it.
    pub(crate) file: ProgressiveFile,
    pub(crate) decoder: Decoder,
    /// Source frames the decoder has gone through: where its next packet starts.
    pub(crate) decoded: u64,
    /// A decoder call failed (e.g. a read was interrupted): symphonia may be left mid-atom
    /// (its `next_header` drops `skip_atom`'s error), so it is rebuilt before any further use.
    pub(crate) stale: bool,
    pub(crate) resampler: Resampler,
    pub(crate) channels: usize,
    /// Source frames still to drop after a seek (it lands at or before the target).
    pub(crate) skip: usize,
    /// Resampled samples that did not fit in the ring yet.
    pub(crate) pending: Vec<f32>,
    pub(crate) eof: bool,
    pub(crate) started: bool,
}

impl Track {
    /// The decoder only reads forward: going back (or on from a stale decoder) starts a new
    /// one at byte 0 (downloaded already, so local) and seeks forward. Returns where it landed.
    pub(crate) fn seek(&mut self, to: Duration) -> Result<Duration, DecodeError> {
        let rate = f64::from(self.decoder.rate());
        let rebuilt = (self.stale || self.behind(to)).then(Instant::now);
        if rebuilt.is_some() {
            self.decoder = Decoder::new(Box::new(self.file.forward()?))?;
            (self.decoded, self.stale) = (0, false);
        }
        let at = self.decoder.seek(to).inspect_err(|_| self.stale = true)?;
        self.decoded = (at.as_secs_f64() * rate).round() as u64;
        if let Some(started) = rebuilt {
            let elapsed = started.elapsed();
            log::debug!("seek to {to:?} rebuilt the decoder in {elapsed:?}");
        }
        Ok(at)
    }

    /// `to` lies before what the decoder went through already.
    pub(crate) fn behind(&self, to: Duration) -> bool {
        to.as_secs_f64() * f64::from(self.decoder.rate()) < self.decoded as f64
    }

    /// The next packet. A stale decoder is first rebuilt back at `decoded`; the frames it
    /// decodes again up to there are skipped.
    pub(crate) fn next(&mut self) -> Result<Option<Pcm>, DecodeError> {
        if self.stale {
            let was = self.decoded;
            self.seek(Duration::from_secs_f64(
                was as f64 / f64::from(self.decoder.rate()),
            ))?;
            self.skip += usize::try_from(was.saturating_sub(self.decoded)).unwrap_or(usize::MAX);
        }
        let pcm = self.decoder.next().inspect_err(|_| self.stale = true)?;
        if let Some(p) = &pcm {
            self.decoded += (p.samples.len() / self.channels) as u64;
        }
        Ok(pcm)
    }
}
