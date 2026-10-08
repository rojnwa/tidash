//! The OS media controls (fastframe-now-playing): the Windows media overlay, macOS Now
//! Playing and MPRIS show the current track; media keys and overlay buttons come back as
//! actions. Driven from the UI thread, where macOS wants the controls started and used.

use crate::model::{Action, PlaybackSnapshot};
use fastframe_now_playing::{App, Command, NowPlaying, Playback, State, Track};
use player::PlayState;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

const COVER_PX: u32 = 640;

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct MediaControls {
    controls: NowPlaying,
    cfg: tidal::ClientConfig,
    wake: Wake,
    /// The last cover whose file was asked for: one download per cover.
    requested: Option<String>,
    landed: (Sender<()>, Receiver<()>),
}

impl MediaControls {
    /// `wake` repaints the window; the next frame's `poll` picks up what arrived.
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let wake: Wake = Arc::new(wake);
        let mut app = App::new("tidash", "Tidash");
        // Nothing raises or quits the app from the panel (MPRIS) yet.
        app.can_raise = false;
        app.can_quit = false;
        let w = wake.clone();
        Self {
            controls: NowPlaying::start(app, move || w()),
            cfg: tidal::ClientConfig::default(),
            wake,
            requested: None,
            landed: channel(),
        }
    }

    /// Shows a playback change; `position` is the player's, now.
    pub fn show(&mut self, pb: &PlaybackSnapshot, position: Duration) {
        let cover = pb.track.as_ref().and_then(|t| t.album.cover.as_deref());
        let art_url = cover.map(|c| tidal::image_url(&self.cfg, c, COVER_PX));
        let file = cover.and_then(cover_file);
        let on_disk = file.as_ref().is_some_and(|f| f.is_file());
        if let (Some(f), Some(url)) = (&file, &art_url)
            && !on_disk
            && self.requested.as_deref() != cover
        {
            self.requested = cover.map(str::to_owned);
            fetch(url.clone(), f.clone(), self.landed.0.clone(), self.wake.clone());
        }
        let art_file = file.filter(|_| on_disk);
        self.controls.update(state_for(pb, position, art_url, art_file));
    }

    /// Playback jumped to `at`.
    pub fn seeked(&self, at: Duration) {
        self.controls.seeked(at);
    }

    /// What the desktop asked for since the last call, as actions. Also shows a cover
    /// that finished downloading.
    pub fn poll(&mut self, pb: &PlaybackSnapshot, position: Duration) -> Vec<Action> {
        if self.landed.1.try_iter().count() > 0 {
            self.show(pb, position);
        }
        self.controls.commands().into_iter().filter_map(|c| action_for(c, pb, position)).collect()
    }
}

/// Where a cover's file goes. Cover ids are UUIDs; anything else gets no file.
// ponytail: covers pile up in the temp dir (~100 KB per album played), left to the OS's
// temp cleanup; move to an evicting cache dir once the app has one.
fn cover_file(cover: &str) -> Option<PathBuf> {
    let safe = !cover.is_empty() && cover.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    safe.then(|| std::env::temp_dir().join("tidash-covers").join(format!("{cover}.jpg")))
}

/// Downloads `url` to `path` on a thread of its own; `done` and `wake` once it is there.
fn fetch(url: String, path: PathBuf, done: Sender<()>, wake: Wake) {
    let run = move || {
        let save = || -> Result<(), Box<dyn std::error::Error>> {
            let bytes = reqwest::blocking::get(&url)?.error_for_status()?.bytes()?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Whole or not at all: the controls never get a half-written file.
            let part = path.with_extension(format!("{:?}.part", std::thread::current().id()));
            std::fs::write(&part, &bytes)?;
            std::fs::rename(&part, &path)?;
            Ok(())
        };
        match save() {
            Ok(()) => {
                let _ = done.send(());
                wake();
            }
            Err(e) => log::debug!("cover {url}: {e}"),
        }
    };
    if let Err(e) = std::thread::Builder::new().name("cover-file".into()).spawn(run) {
        log::warn!("cover download thread failed: {e}");
    }
}

fn state_for(pb: &PlaybackSnapshot, position: Duration, art_url: Option<String>, art_file: Option<PathBuf>) -> State {
    let Some(t) = &pb.track else { return State::default() };
    State {
        playback: match pb.state {
            Some(PlayState::Playing) => Playback::Playing,
            // Still loading reads as paused: Play resumes it.
            Some(PlayState::Paused | PlayState::Buffering) => Playback::Paused,
            Some(PlayState::Stopped) | None => Playback::Stopped,
        },
        position,
        track: Some(Track {
            id: t.id.to_string(),
            title: t.title.clone(),
            artists: t.artists.iter().map(|a| a.name.clone()).collect(),
            album: t.album.title.clone(),
            duration: Some(Duration::from_secs(t.duration.into())),
            art_file,
            art_url,
            ..Track::default()
        }),
        ..State::default()
    }
}

fn action_for(command: Command, pb: &PlaybackSnapshot, position: Duration) -> Option<Action> {
    // As the player bar's button: loading counts as playing.
    let busy = matches!(pb.state, Some(PlayState::Playing | PlayState::Buffering));
    Some(match command {
        Command::Play => Action::Play,
        Command::Pause | Command::Stop => Action::Pause,
        Command::PlayPause if busy => Action::Pause,
        Command::PlayPause => Action::Play,
        Command::Next => Action::Next,
        Command::Previous => Action::Previous,
        Command::SeekBy(ms) => {
            let by = Duration::from_millis(ms.unsigned_abs());
            Action::Seek(if ms < 0 { position.saturating_sub(by) } else { position + by })
        }
        // A request for a track since replaced is dropped.
        Command::SetPosition { track_id, position } if pb.track.as_ref().is_some_and(|t| t.id.to_string() == track_id) => {
            Action::Seek(position)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(state: PlayState) -> PlaybackSnapshot {
        let track = tidal::Track {
            id: 7,
            title: "Song".into(),
            duration: 200,
            track_number: 1,
            volume_number: 1,
            artists: vec![],
            album: tidal::AlbumRef { id: 1, title: "Album".into(), cover: None },
            stream_ready: true,
        };
        PlaybackSnapshot { state: Some(state), track: Some(track), volume: 1.0 }
    }

    #[test]
    fn commands_map_to_actions() {
        let s = Duration::from_secs(1);
        let act = |c, st| format!("{:?}", action_for(c, &snapshot(st), s * 20));
        assert_eq!(act(Command::PlayPause, PlayState::Playing), "Some(Pause)");
        assert_eq!(act(Command::PlayPause, PlayState::Buffering), "Some(Pause)");
        assert_eq!(act(Command::PlayPause, PlayState::Paused), "Some(Play)");
        assert_eq!(act(Command::PlayPause, PlayState::Stopped), "Some(Play)");
        assert_eq!(act(Command::Stop, PlayState::Playing), "Some(Pause)");
        assert_eq!(act(Command::SeekBy(-30_000), PlayState::Playing), "Some(Seek(0ns))");
        assert_eq!(act(Command::SeekBy(10_000), PlayState::Playing), "Some(Seek(30s))");
        let to = |id: &str| Command::SetPosition { track_id: id.into(), position: s * 5 };
        assert_eq!(act(to("7"), PlayState::Paused), "Some(Seek(5s))");
        assert_eq!(act(to("8"), PlayState::Paused), "None", "another track's");
        assert_eq!(act(Command::Raise, PlayState::Paused), "None");
    }

    #[test]
    fn state_shows_the_track() {
        let st = state_for(&snapshot(PlayState::Buffering), Duration::ZERO, None, None);
        assert_eq!(st.playback, Playback::Paused);
        let t = st.track.expect("track");
        assert_eq!((t.id.as_str(), t.title.as_str(), t.album.as_str()), ("7", "Song", "Album"));
        assert_eq!(t.duration, Some(Duration::from_secs(200)));
        assert_eq!(state_for(&PlaybackSnapshot::default(), Duration::ZERO, None, None), State::default());
    }

    #[test]
    fn only_uuid_like_covers_get_a_file() {
        assert!(cover_file("ab12-cd34").is_some_and(|p| p.ends_with("ab12-cd34.jpg")));
        assert!(cover_file("../evil").is_none());
        assert!(cover_file("").is_none());
    }
}
