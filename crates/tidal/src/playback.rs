//! Playback info: track id to DASH segment URLs, and playback-failure classification.

use crate::{Client, Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Lossless,
    HiResLossless,
}

impl Quality {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lossless => "LOSSLESS",
            Self::HiResLossless => "HI_RES_LOSSLESS",
        }
    }
}

#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub track_id: u64,
    /// Init segment first, then media segments in order.
    pub urls: Vec<String>,
    pub codec: String,
    pub quality: String,
    pub bit_depth: Option<u16>,
    pub sample_rate: Option<u32>,
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackInfo {
    track_id: u64,
    audio_quality: String,
    manifest_mime_type: String,
    manifest: String,
    bit_depth: Option<u16>,
    sample_rate: Option<u32>,
    track_replay_gain: Option<f32>,
    track_peak_amplitude: Option<f32>,
    album_replay_gain: Option<f32>,
    album_peak_amplitude: Option<f32>,
}

impl Client {
    pub async fn stream_info(&self, track_id: u64, quality: Quality) -> Result<StreamInfo> {
        let q = [
            ("audioquality", quality.as_str().to_owned()),
            ("playbackmode", "STREAM".to_owned()),
            ("assetpresentation", "FULL".to_owned()),
        ];
        let p: PlaybackInfo = self
            .get(&format!("tracks/{track_id}/playbackinfo"), &q)
            .await?;
        if p.manifest_mime_type != "application/dash+xml" {
            return Err(Error::Manifest(format!(
                "unsupported manifest type {}",
                p.manifest_mime_type
            )));
        }
        let (codec, urls) = dash_segments(&p.manifest)?;
        Ok(StreamInfo {
            track_id: p.track_id,
            urls,
            codec,
            quality: p.audio_quality,
            bit_depth: p.bit_depth,
            sample_rate: p.sample_rate,
            track_gain: p.track_replay_gain,
            track_peak: p.track_peak_amplitude,
            album_gain: p.album_replay_gain,
            album_peak: p.album_peak_amplitude,
        })
    }
}

fn is_named(n: &roxmltree::Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}

fn bad(msg: &str) -> Error {
    Error::Manifest(msg.into())
}

/// Decode a base64 MPD into (codec, [init url, media urls...]).
pub fn dash_segments(manifest_b64: &str) -> Result<(String, Vec<String>)> {
    let xml = STANDARD
        .decode(manifest_b64.trim())
        .map_err(|e| Error::Manifest(format!("bad base64: {e}")))?;
    let xml = String::from_utf8(xml).map_err(|e| Error::Manifest(format!("bad utf-8: {e}")))?;
    let doc = roxmltree::Document::parse(&xml).map_err(|e| Error::Manifest(e.to_string()))?;
    if doc.descendants().any(|n| is_named(&n, "ContentProtection")) {
        return Err(bad("encrypted stream"));
    }
    let rep = doc
        .descendants()
        .find(|n| is_named(n, "Representation"))
        .ok_or_else(|| bad("no Representation"))?;
    fn tmpl_in<'a, 'i>(p: roxmltree::Node<'a, 'i>) -> Option<roxmltree::Node<'a, 'i>> {
        p.children().find(|n| is_named(n, "SegmentTemplate"))
    }
    let tmpl = tmpl_in(rep)
        .or_else(|| rep.parent().and_then(tmpl_in))
        .ok_or_else(|| bad("no SegmentTemplate"))?;
    let attr = |name| {
        tmpl.attribute(name)
            .ok_or_else(|| bad("incomplete SegmentTemplate"))
    };
    let (init, media) = (attr("initialization")?, attr("media")?);
    if media.replace("$Number$", "").contains('$') {
        return Err(bad("unsupported $identifier$ in media template"));
    }
    let start: u64 = match tmpl.attribute("startNumber") {
        Some(s) => s.parse().map_err(|_| bad("bad startNumber"))?,
        None => 1,
    };
    let timeline = tmpl
        .children()
        .find(|n| is_named(n, "SegmentTimeline"))
        .ok_or_else(|| bad("no SegmentTimeline"))?;
    let mut count = 0u64;
    for s in timeline.children().filter(|n| is_named(n, "S")) {
        let r: u64 = match s.attribute("r") {
            Some(r) => r.parse().map_err(|_| bad("bad repeat count"))?,
            None => 0,
        };
        count = count.saturating_add(1 + r);
    }
    if count > 100_000 {
        return Err(bad("implausible segment count"));
    }
    let urls = std::iter::once(init.to_owned())
        .chain((start..start + count).map(|n| media.replace("$Number$", &n.to_string())))
        .collect();
    Ok((rep.attribute("codecs").unwrap_or_default().to_owned(), urls))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Skip the track.
    Terminal,
    /// Not a reason to skip the track.
    Retryable,
}

pub fn classify(err: &Error) -> Failure {
    match err {
        Error::Status {
            sub_status: Some(4005 | 4010 | 4030 | 4031 | 4032 | 4034 | 4035),
            ..
        }
        | Error::Manifest(_) => Failure::Terminal,
        _ => Failure::Retryable,
    }
}
