//! The user's settings, saved as JSON in the config dir: the audio output and the streams.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub audio: Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// An output device by name; `None` is the system default.
    pub device: Option<String>,
    /// Bit-perfect output (`player::OutputConfig::exclusive`).
    pub exclusive: bool,
    pub normalization: Normalization,
    /// Ask for Hi-Res streams; else Lossless (16-bit, 44.1 kHz).
    pub hi_res: bool,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            device: None,
            exclusive: false,
            normalization: Normalization::Off,
            hi_res: true,
        }
    }
}

/// Which ReplayGain values level the volume, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Normalization {
    #[default]
    Off,
    Track,
    Album,
}

/// `settings.json` in the per-user config dir; `None` without a home directory.
pub fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "tidash").map(|p| p.config_dir().join("settings.json"))
}

impl Settings {
    /// The saved settings; the defaults when there are none or they cannot be read. Blocks.
    pub fn load(path: &Path) -> Settings {
        let read = std::fs::read(path).and_then(|b| Ok(serde_json::from_slice(&b)?));
        read.unwrap_or_else(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                log::info!("no settings saved yet: the defaults");
            } else {
                log::warn!(
                    "settings at {} unreadable, the defaults instead: {e}",
                    path.display()
                );
            }
            Settings::default()
        })
    }

    /// Writes a `.tmp` beside `path`, then renames it over: a crash leaves the old file whole.
    /// Blocks.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

impl Audio {
    /// The player output these settings ask for.
    pub fn output(&self) -> player::OutputConfig {
        player::OutputConfig {
            device: self.device.clone().map_or(
                fastframe_audio::Device::Default,
                fastframe_audio::Device::Named,
            ),
            exclusive: self.exclusive,
        }
    }
}

/// The linear gain `mode` gives a track with `info`'s ReplayGain values, never above what
/// brings its peak to full scale. Album falls back to each track value it lacks; without a
/// gain, 1.0.
pub fn replay_gain(mode: Normalization, info: &tidal::StreamInfo) -> f32 {
    let (gain, peak) = match mode {
        Normalization::Off => return 1.0,
        Normalization::Track => (info.track_gain, info.track_peak),
        Normalization::Album => (
            info.album_gain.or(info.track_gain),
            info.album_peak.or(info.track_peak),
        ),
    };
    let Some(db) = gain else { return 1.0 };
    let linear = 10f32.powf(db / 20.0);
    match peak {
        Some(p) if p > 0.0 => linear.min(1.0 / p),
        _ => linear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(gains: [Option<f32>; 4]) -> tidal::StreamInfo {
        let [track_gain, track_peak, album_gain, album_peak] = gains;
        tidal::StreamInfo {
            track_id: 1,
            urls: vec![],
            codec: "flac".into(),
            quality: "LOSSLESS".into(),
            bit_depth: None,
            sample_rate: None,
            track_gain,
            track_peak,
            album_gain,
            album_peak,
        }
    }

    fn near(got: f32, want: f32) -> bool {
        (got - want).abs() < 1e-4
    }

    #[test]
    fn replay_gain_track_album_off() {
        let i = info([Some(-6.0206), Some(0.5), Some(-3.0), Some(0.9)]);
        assert_eq!(replay_gain(Normalization::Off, &i), 1.0);
        let track = replay_gain(Normalization::Track, &i);
        assert!(near(track, 0.5), "{track}");
        let album = replay_gain(Normalization::Album, &i);
        assert!(near(album, 0.7079), "{album}");
    }

    #[test]
    fn replay_gain_never_clips() {
        let i = info([Some(6.0), Some(0.9), None, None]);
        let track = replay_gain(Normalization::Track, &i);
        assert!(near(track, 1.0 / 0.9), "{track}");
    }

    #[test]
    fn replay_gain_falls_back_to_track_values() {
        let i = info([Some(-6.0206), Some(0.5), None, None]);
        let (track, album) = (Normalization::Track, Normalization::Album);
        assert_eq!(replay_gain(album, &i), replay_gain(track, &i));
        assert!(near(replay_gain(album, &i), 0.5));
        let none = info([None; 4]);
        for mode in [Normalization::Off, track, album] {
            assert_eq!(replay_gain(mode, &none), 1.0, "{mode:?}");
        }
    }

    #[test]
    fn settings_load_save_and_defaults() {
        let dir = std::env::temp_dir().join(format!("tidash-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("settings.json");
        let defaults = Settings::default();
        let want = Audio {
            device: None,
            exclusive: false,
            normalization: Normalization::Off,
            hi_res: true,
        };
        assert_eq!(defaults.audio, want);
        assert_eq!(Settings::load(&file), defaults, "missing");
        let saved = Settings {
            audio: Audio {
                device: Some("DAC".into()),
                exclusive: true,
                normalization: Normalization::Album,
                hi_res: false,
            },
        };
        saved.save(&file).expect("saved");
        assert_eq!(Settings::load(&file), saved);
        std::fs::write(&file, r#"{"audio":{"exclusive":true}}"#).expect("written");
        let partial = Audio {
            exclusive: true,
            ..want
        };
        assert_eq!(Settings::load(&file).audio, partial, "the rest default");
        std::fs::write(&file, "not json {").expect("written");
        assert_eq!(Settings::load(&file), defaults, "garbage");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
