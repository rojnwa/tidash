//! The user's settings, saved as JSON in the config dir: the audio output and the streams,
//! the look, the TIDAL client, and the integrations.

use crate::scrobble::Service;
use crate::theme::Palette;
use fastframe_theme::CustomTheme;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub audio: Audio,
    pub theme: Theme,
    pub account: Account,
    pub integrations: Integrations,
    pub behavior: Behavior,
    /// A palette file in `themes_dir` by name, over `theme`.
    pub custom_theme: Option<String>,
    /// The last palette read for `custom_theme`, kept when the file goes; a damaged one loads
    /// as `None`.
    #[serde(
        deserialize_with = "fastframe_theme::read_cached_theme",
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_theme_cache: Option<CustomTheme<Palette>>,
    /// The player bar and coverless pages take a tint from the playing cover.
    pub accent_from_art: bool,
    pub sidebar_collapsed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            audio: Audio::default(),
            theme: Theme::default(),
            account: Account::default(),
            integrations: Integrations::default(),
            behavior: Behavior::default(),
            custom_theme: None,
            custom_theme_cache: None,
            accent_from_art: true,
            sidebar_collapsed: false,
        }
    }
}

/// What closing the window and a launch do; both need a tray the desktop shows
/// (`tray::hides_to_tray`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    /// Closing the window keeps the app playing in the tray.
    pub close_to_tray: bool,
    /// A launch opens no window, only the tray item.
    pub start_minimized: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            start_minimized: false,
        }
    }
}

/// Who each scrobbling service is connected as; its secret is in the credential store, never
/// here. The backend sets these, on a connect, a disconnect, or a refused token. Discord is
/// the user's to turn on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Integrations {
    pub lastfm_user: Option<String>,
    pub listenbrainz_user: Option<String>,
    /// Show what plays in the Discord status (`discord::Presence`).
    pub discord: bool,
}

impl Integrations {
    /// The services with a user: the only ones scrobbled to.
    pub fn connected(&self) -> Vec<Service> {
        let users = [
            (Service::LastFm, &self.lastfm_user),
            (Service::ListenBrainz, &self.listenbrainz_user),
        ];
        let with_user = users.into_iter().filter(|(_, user)| user.is_some());
        with_user.map(|(service, _)| service).collect()
    }
}

/// The built-in palettes; System follows the desktop's dark or light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    System,
    #[default]
    Dark,
    Light,
}

/// egui's theme follows the setting: `pick` reads it back.
impl From<Theme> for egui::ThemePreference {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::System => Self::System,
            Theme::Dark => Self::Dark,
            Theme::Light => Self::Light,
        }
    }
}

/// The TIDAL client to log in with; `None` or blank is the built-in one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
}

/// `s` trimmed; `None` if that leaves nothing.
pub fn nonblank(s: &str) -> Option<String> {
    Some(s.trim()).filter(|s| !s.is_empty()).map(str::to_owned)
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

/// The palette files' folder, `themes` beside `path`.
pub fn themes_dir() -> Option<PathBuf> {
    path().map(|p| p.with_file_name("themes"))
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

    /// Saved as `write_atomic` writes. Blocks.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }

    /// The built-in client config with the account's overrides.
    pub fn client_config(&self) -> tidal::ClientConfig {
        let mut cfg = tidal::ClientConfig::default();
        let own = |field: &Option<String>| field.as_deref().and_then(nonblank);
        if let Some(id) = own(&self.account.client_id) {
            cfg.client_id = id;
        }
        if let Some(uri) = own(&self.account.redirect_uri) {
            cfg.redirect_uri = uri;
        }
        cfg
    }
}

/// Writes `bytes` to a `.tmp` beside `path` through to the disk, then renames it over: a crash
/// leaves the old file whole. Creates the directory. Blocks.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file); // closed before it is renamed
    std::fs::rename(&tmp, path)
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
        assert_eq!(defaults.theme, Theme::Dark);
        assert!(defaults.accent_from_art && !defaults.sidebar_collapsed);
        assert_eq!(defaults.account, Account::default());
        assert_eq!(Settings::load(&file), defaults, "missing");
        let saved = Settings {
            audio: Audio {
                device: Some("DAC".into()),
                exclusive: true,
                normalization: Normalization::Album,
                hi_res: false,
            },
            theme: Theme::Light,
            account: Account {
                client_id: Some("id".into()),
                redirect_uri: Some("https://example.test/cb".into()),
            },
            integrations: Integrations {
                lastfm_user: Some("ann".into()),
                listenbrainz_user: Some("rob".into()),
                discord: true,
            },
            behavior: Behavior {
                close_to_tray: false,
                start_minimized: true,
            },
            custom_theme: Some("Nord.json".into()),
            custom_theme_cache: fastframe_theme::presets::themes().nth(2),
            accent_from_art: false,
            sidebar_collapsed: true,
        };
        assert!(saved.custom_theme_cache.is_some());
        saved.save(&file).expect("saved");
        assert_eq!(Settings::load(&file), saved);
        let json = r#"{"audio":{"exclusive":true},"account":{"client_id":"id"}}"#;
        std::fs::write(&file, json).expect("written");
        let partial = Settings {
            audio: Audio {
                exclusive: true,
                ..want
            },
            account: Account {
                client_id: Some("id".into()),
                redirect_uri: None,
            },
            ..defaults.clone()
        };
        assert_eq!(Settings::load(&file), partial, "the rest default");
        std::fs::write(&file, r#"{"theme":"Light"}"#).expect("written");
        assert_eq!(Settings::load(&file).theme, Theme::Light);
        let json = r#"{"custom_theme":"Nord.json","custom_theme_cache":{"filename":7}}"#;
        std::fs::write(&file, json).expect("written");
        let damaged = Settings {
            custom_theme: Some("Nord.json".into()),
            ..defaults.clone()
        };
        assert_eq!(Settings::load(&file), damaged, "a damaged cache is none");
        std::fs::write(&file, "not json {").expect("written");
        assert_eq!(Settings::load(&file), defaults, "garbage");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Close to tray on and start minimized off, also for settings saved before either existed.
    #[test]
    fn behavior_defaults() {
        let b = Settings::default().behavior;
        assert!(b.close_to_tray && !b.start_minimized, "{b:?}");
        let old: Settings = serde_json::from_str(r#"{"theme":"Light"}"#).expect("parsed");
        assert_eq!(old.behavior, b);
        let one: Behavior = serde_json::from_str(r#"{"start_minimized":true}"#).expect("parsed");
        assert!(one.close_to_tray && one.start_minimized, "{one:?}");
    }

    #[test]
    fn client_config_overrides_and_blank_is_builtin() {
        let builtin = tidal::ClientConfig::default();
        let config = |client_id: Option<&str>, redirect_uri: Option<&str>| {
            let account = Account {
                client_id: client_id.map(Into::into),
                redirect_uri: redirect_uri.map(Into::into),
            };
            let c = Settings {
                account,
                ..Settings::default()
            }
            .client_config();
            (c.client_id, c.redirect_uri)
        };
        let own = config(Some(" mine "), Some("https://example.test/cb"));
        assert_eq!(own, ("mine".into(), "https://example.test/cb".into()));
        let want = (builtin.client_id, builtin.redirect_uri);
        for blank in [None, Some(""), Some("  \t")] {
            assert_eq!(config(blank, blank), want, "{blank:?}");
        }
        assert_eq!(nonblank("  "), None);
        assert_eq!(nonblank(" a b "), Some("a b".into()));
    }
}
