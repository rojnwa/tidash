//! Lyrics and credits of a track.

use crate::{Client, Error, Result};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct LyricLine {
    pub at: Duration,
    /// Empty for an instrumental gap.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lyrics {
    /// Empty when the lyrics are unsynced.
    pub lines: Vec<LyricLine>,
    /// The plain text, `\n`-separated, blank lines between stanzas.
    pub text: String,
    pub rtl: bool,
    /// `MUSIXMATCH` (S5).
    pub provider: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLyrics {
    #[serde(default)]
    lyrics: Option<String>,
    #[serde(default)]
    subtitles: Option<String>,
    #[serde(default)]
    is_right_to_left: bool,
    #[serde(default)]
    lyrics_provider: Option<String>,
}

impl From<RawLyrics> for Lyrics {
    fn from(r: RawLyrics) -> Self {
        Self {
            lines: r.subtitles.as_deref().map(parse_lrc).unwrap_or_default(),
            text: r.lyrics.unwrap_or_default(),
            rtl: r.is_right_to_left,
            provider: r.lyrics_provider,
        }
    }
}

/// The `[mm:ss.xx] text` cues of LRC; any other line is skipped.
pub fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    lrc.lines().filter_map(cue).collect()
}

fn cue(line: &str) -> Option<LyricLine> {
    let (stamp, text) = line.strip_prefix('[')?.split_once(']')?;
    let b = stamp.as_bytes();
    if b.len() != 8 || b[2] != b':' || b[5] != b'.' {
        return None;
    }
    let two =
        |i: usize| Some(char::from(b[i]).to_digit(10)? * 10 + char::from(b[i + 1]).to_digit(10)?);
    let ms = (two(0)? * 60 + two(3)?) * 1000 + two(6)? * 10;
    Some(LyricLine {
        at: Duration::from_millis(ms.into()),
        text: text.strip_prefix(' ').unwrap_or(text).to_owned(),
    })
}

/// One role and who filled it; `role` is TIDAL's (`Producer`, `Mixing Engineer`), shown as is.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Credit {
    #[serde(rename = "type")]
    pub role: String,
    #[serde(default)]
    pub contributors: Vec<Contributor>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Contributor {
    pub name: String,
    #[serde(default)]
    pub id: Option<u64>,
}

impl Client {
    /// `None` when the track has none: a 404, whose body is not TIDAL's error shape.
    pub async fn lyrics(&self, track_id: u64) -> Result<Option<Lyrics>> {
        match self
            .get::<RawLyrics>(&format!("tracks/{track_id}/lyrics"), &[])
            .await
        {
            Ok(raw) => Ok(Some(raw.into())),
            Err(Error::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn track_credits(&self, track_id: u64) -> Result<Vec<Credit>> {
        self.get(
            &format!("tracks/{track_id}/credits"),
            &[
                ("limit", "100".into()),
                ("includeContributors", "true".into()),
            ],
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lrc_centiseconds() {
        assert_eq!(
            parse_lrc("[01:02.50] Hello\n[01:05.00] \nnot a cue"),
            [
                LyricLine {
                    at: Duration::from_millis(62_500),
                    text: "Hello".into(),
                },
                LyricLine {
                    at: Duration::from_secs(65),
                    text: String::new(),
                },
            ]
        );
    }

    #[test]
    fn unsynced_lyrics_have_no_lines() {
        for subtitles in [r#","subtitles":"""#, r#","subtitles":null"#, ""] {
            let raw = format!(r#"{{"lyrics":"la la"{subtitles}}}"#);
            let l = Lyrics::from(serde_json::from_str::<RawLyrics>(&raw).expect("lyrics json"));
            assert!(l.lines.is_empty(), "{raw}");
            assert_eq!(l.text, "la la");
        }
    }
}
