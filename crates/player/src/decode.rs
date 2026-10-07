//! Bit-exact FLAC-in-fMP4 decoding via symphonia 0.6.

use std::time::Duration;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

/// Interleaved PCM, left-justified in `i32` (sample `<< (32 - bits)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub samples: Vec<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("corrupt: {0}")]
    Corrupt(String),
}

impl From<Error> for DecodeError {
    fn from(e: Error) -> Self {
        match e {
            Error::IoError(e) => DecodeError::Io(e),
            Error::Unsupported(m) => DecodeError::Unsupported(m.to_string()),
            other => DecodeError::Corrupt(other.to_string()),
        }
    }
}

pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    rate: u32,
    channels: u16,
    bits: u16,
}

impl Decoder {
    pub fn new(src: Box<dyn MediaSource>) -> Result<Decoder, DecodeError> {
        let mss = MediaSourceStream::new(src, Default::default());
        let format = symphonia::default::get_probe().probe(
            &Hint::new(),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        let unsupported = |m: &str| DecodeError::Unsupported(m.to_string());
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| unsupported("no audio track"))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| unsupported("not an audio track"))?;
        let bits = params
            .bits_per_sample
            .ok_or_else(|| unsupported("unknown bit depth"))?;
        let rate = params
            .sample_rate
            .ok_or_else(|| unsupported("unknown sample rate"))?;
        let channels = params
            .channels
            .as_ref()
            .ok_or_else(|| unsupported("unknown channels"))?
            .count();
        if !(1..=32).contains(&bits) {
            return Err(unsupported("bit depth out of range"));
        }
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())?;
        let track_id = track.id;
        Ok(Decoder {
            format,
            decoder,
            track_id,
            rate,
            channels: channels as u16,
            bits: bits as u16,
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn bits(&self) -> u16 {
        self.bits
    }

    /// Next decoded packet; `None` at end of stream.
    #[allow(clippy::should_implement_trait)] // fallible, API fixed by the plan
    pub fn next(&mut self) -> Result<Option<Pcm>, DecodeError> {
        loop {
            let Some(packet) = self.format.next_packet()? else {
                return Ok(None);
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = self.decoder.decode(&packet)?;
            // symphonia scales integer samples to the full i32 range, i.e. already left-justified.
            let mut samples = vec![0i32; buf.samples_interleaved()];
            buf.copy_to_slice_interleaved(&mut samples);
            return Ok(Some(Pcm {
                rate: self.rate,
                channels: self.channels,
                bits: self.bits,
                samples,
            }));
        }
    }

    /// Seek (needs a seekable source); returns the actual position, at or before `to`.
    pub fn seek(&mut self, to: Duration) -> Result<Duration, DecodeError> {
        // fMP4 FLAC timebase is 1/sample-rate.
        let ts = (to.as_secs_f64() * f64::from(self.rate)).round() as i64;
        let seeked = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts: Timestamp::new(ts),
                track_id: self.track_id,
            },
        )?;
        self.decoder.reset();
        Ok(Duration::from_secs_f64(
            seeked.actual_ts.get().max(0) as f64 / f64::from(self.rate),
        ))
    }
}
