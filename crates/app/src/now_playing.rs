//! The OS media controls (fastframe-now-playing): the Windows media overlay, macOS Now
//! Playing and MPRIS show the current track; media keys and overlay buttons come back as
//! actions, MPRIS Raise and Quit as the tray's Show and Quit, and its OpenUri, a TIDAL link,
//! as Open. Driven from the main thread, where macOS wants the controls started and used.

use crate::model::{Action, PlaybackSnapshot};
use crate::tray::TrayAction;
use fastframe_now_playing::{App, Command, NowPlaying, Playback, State, Track};
use player::PlayState;
use reqwest::Url;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tidal::DiskCache;

const COVER_PX: u32 = 640;

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct MediaControls {
    controls: NowPlaying,
    /// Where the cover URLs point (`tidal::ClientConfig::image_base`).
    image_base: Url,
    wake: Wake,
    /// The images cache, once the backend opened it; until then (or without one) the controls
    /// get only the cover URL.
    cache: Arc<OnceLock<Arc<DiskCache>>>,
    /// The last cover URL whose file was asked for: one download per cover.
    requested: Option<String>,
    landed: (Sender<()>, Receiver<()>),
}

impl MediaControls {
    /// `wake` repaints the window; the next frame's `poll` picks up what arrived. Covers come
    /// from `image_base`; their files go into `cache` once the backend sets it
    /// (`images::open_cache`).
    pub fn start(
        wake: impl Fn() + Send + Sync + 'static,
        cache: Arc<OnceLock<Arc<DiskCache>>>,
        image_base: Url,
    ) -> Self {
        let wake: Wake = Arc::new(wake);
        let mut app = App::new("tidash", "Tidash");
        // The panel (MPRIS) brings the window up, quits, and opens TIDAL links; which of
        // these URIs are ones is `link::parse`'s to say.
        app.can_raise = true;
        app.can_quit = true;
        app.uri_schemes = vec!["tidal".into(), "https".into()];
        let w = wake.clone();
        Self {
            controls: NowPlaying::start(app, move || w()),
            image_base,
            wake,
            cache,
            requested: None,
            landed: channel(),
        }
    }

    /// Shows a playback change; `position` is the player's, now.
    pub fn show(&mut self, pb: &PlaybackSnapshot, position: Duration) {
        let cache = self.cache.get();
        let (art_url, file) = art(pb, &self.image_base, cache.map(Arc::as_ref));
        let on_disk = file.as_ref().is_some_and(|f| f.is_file());
        if let (Some(cache), Some(url)) = (cache, &art_url)
            && !on_disk
            && self.requested.as_ref() != Some(url)
        {
            self.requested = Some(url.clone());
            let (done, wake) = (self.landed.0.clone(), self.wake.clone());
            fetch(url.clone(), cache.clone(), done, wake);
        }
        let art_file = file.filter(|_| on_disk);
        self.controls
            .update(state_for(pb, position, art_url, art_file));
    }

    /// Playback jumped to `at`.
    pub fn seeked(&self, at: Duration) {
        self.controls.seeked(at);
    }

    /// What the desktop asked for since the last call. Also shows a cover that finished
    /// downloading.
    pub fn poll(&mut self, pb: &PlaybackSnapshot, position: Duration) -> Vec<TrayAction> {
        if self.landed.1.try_iter().count() > 0 {
            self.show(pb, position);
        }
        self.controls
            .commands()
            .into_iter()
            .filter_map(|c| action_for(c, pb, position))
            .collect()
    }
}

/// The current track's album cover URL at `COVER_PX`, and its file in `cache`, present or not.
fn art(
    pb: &PlaybackSnapshot,
    base: &Url,
    cache: Option<&DiskCache>,
) -> (Option<String>, Option<PathBuf>) {
    let image = pb.track.as_ref().and_then(|t| t.album.image());
    let url = image.map(|i| i.url(base, COVER_PX));
    let file = url.as_deref().zip(cache).map(|(u, c)| c.path(u));
    (url, file)
}

/// Cover downloads, shared by the cover threads. Blocking, so used only there; a static is
/// never dropped, so never on a runtime thread either.
static COVERS: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// Downloads `url` into `cache` on a thread of its own; `done` and `wake` once it is there.
fn fetch(url: String, cache: Arc<DiskCache>, done: Sender<()>, wake: Wake) {
    let run = move || {
        let save = || -> Result<(), Box<dyn std::error::Error>> {
            let http = match COVERS.get() {
                Some(http) => http,
                None => {
                    let http = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10)) // a cover is ~50 KB
                        .build()?;
                    COVERS.get_or_init(|| http)
                }
            };
            let bytes = http.get(&url).send()?.error_for_status()?.bytes()?;
            // Whole or not at all: the controls never get a half-written file.
            let entry = tidal::Entry {
                etag: None,
                body: bytes.to_vec(),
            };
            cache.write(&url, &entry)?;
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
    if let Err(e) = std::thread::Builder::new()
        .name("cover-file".into())
        .spawn(run)
    {
        log::warn!("cover download thread failed: {e}");
    }
}

fn state_for(
    pb: &PlaybackSnapshot,
    position: Duration,
    art_url: Option<String>,
    art_file: Option<PathBuf>,
) -> State {
    let Some(t) = &pb.track else {
        return State::default();
    };
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

/// Raise, Quit and OpenUri are the app's; the rest the backend's.
fn action_for(command: Command, pb: &PlaybackSnapshot, position: Duration) -> Option<TrayAction> {
    Some(TrayAction::Play(match command {
        Command::Raise => return Some(TrayAction::Show),
        Command::Quit => return Some(TrayAction::Quit),
        Command::OpenUri(uri) => return crate::link::parse(&uri).map(TrayAction::Open),
        Command::Play => Action::Play,
        Command::Pause | Command::Stop => Action::Pause,
        Command::PlayPause => Action::TogglePlay,
        Command::Next => Action::Next,
        Command::Previous => Action::Previous,
        Command::SeekBy(ms) => {
            let by = Duration::from_millis(ms.unsigned_abs());
            Action::Seek(if ms < 0 {
                position.saturating_sub(by)
            } else {
                position + by
            })
        }
        // A request for a track since replaced is dropped.
        Command::SetPosition { track_id, position }
            if pb
                .track
                .as_ref()
                .is_some_and(|t| t.id.to_string() == track_id) =>
        {
            Action::Seek(position)
        }
        _ => return None,
    }))
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
            album: tidal::AlbumRef {
                id: 1,
                title: "Album".into(),
                cover: None,
            },
            stream_ready: true,
            ..Default::default()
        };
        PlaybackSnapshot {
            state: Some(state),
            track: Some(track),
            volume: 1.0,
            output: None,
        }
    }

    #[test]
    fn commands_map_to_actions() {
        let s = Duration::from_secs(1);
        let act = |c, st| format!("{:?}", action_for(c, &snapshot(st), s * 20));
        for st in [PlayState::Playing, PlayState::Paused, PlayState::Stopped] {
            assert_eq!(
                act(Command::PlayPause, st),
                "Some(Play(TogglePlay))",
                "{st:?}"
            );
        }
        assert_eq!(act(Command::Play, PlayState::Playing), "Some(Play(Play))");
        assert_eq!(act(Command::Stop, PlayState::Playing), "Some(Play(Pause))");
        assert_eq!(
            act(Command::SeekBy(-30_000), PlayState::Playing),
            "Some(Play(Seek(0ns)))"
        );
        assert_eq!(
            act(Command::SeekBy(10_000), PlayState::Playing),
            "Some(Play(Seek(30s)))"
        );
        let to = |id: &str| Command::SetPosition {
            track_id: id.into(),
            position: s * 5,
        };
        assert_eq!(act(to("7"), PlayState::Paused), "Some(Play(Seek(5s)))");
        assert_eq!(act(to("8"), PlayState::Paused), "None", "another track's");
        // The app's, not the backend's: MPRIS Raise and Quit.
        assert_eq!(act(Command::Raise, PlayState::Paused), "Some(Show)");
        assert_eq!(act(Command::Quit, PlayState::Paused), "Some(Quit)");
        // A TIDAL link opens; nothing else does.
        let uri = |u: &str| act(Command::OpenUri(u.into()), PlayState::Paused);
        assert_eq!(uri("tidal://track/1"), "Some(Open(Track(1)))");
        assert_eq!(
            uri("https://tidal.com/browse/album/2?u"),
            "Some(Open(Album(2)))"
        );
        assert_eq!(uri("https://evil.com/album/2"), "None");
        assert_eq!(uri("tidal://track/1%2F..%2Fusers"), "None");
    }

    #[test]
    fn state_shows_the_track() {
        let st = state_for(&snapshot(PlayState::Buffering), Duration::ZERO, None, None);
        assert_eq!(st.playback, Playback::Paused);
        let t = st.track.expect("track");
        assert_eq!(
            (t.id.as_str(), t.title.as_str(), t.album.as_str()),
            ("7", "Song", "Album")
        );
        assert_eq!(t.duration, Some(Duration::from_secs(200)));
        assert_eq!(
            state_for(&PlaybackSnapshot::default(), Duration::ZERO, None, None),
            State::default()
        );
    }

    /// The OS gets the images cache's file for the 640 px rendition, downloaded through it.
    #[test]
    fn now_playing_cover_path_is_in_images_cache() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/ab12/cd34/640x640.jpg"))
                .respond_with(ResponseTemplate::new(200).set_body_raw("jpeg", "image/jpeg"))
                .expect(1)
                .mount(&server)
                .await;
            let dir = std::env::temp_dir().join("tidash-now-playing");
            let _ = std::fs::remove_dir_all(&dir);
            let cache = Arc::new(DiskCache::open(dir.clone(), 1 << 20, "jpg").expect("a cache"));
            let mut pb = snapshot(PlayState::Playing);
            if let Some(t) = &mut pb.track {
                t.album.cover = Some("ab12-cd34".into());
            }
            let base = format!("{}/", server.uri()).parse().expect("a URL");
            let (url, file) = art(&pb, &base, Some(&cache));
            let url = url.expect("a cover URL");
            let file = file.expect("a cover file");
            let unopened = art(&pb, &base, None);
            assert_eq!(
                unopened,
                (Some(url.clone()), None),
                "no cache yet: the URL only"
            );
            assert_eq!(file, cache.path(&url));
            assert!(file.starts_with(&dir));
            let (done, landed) = channel();
            fetch(url.clone(), cache.clone(), done, Arc::new(|| {}));
            // The server runs on a thread of its own: blocking here does not stall it.
            landed
                .recv_timeout(Duration::from_secs(5))
                .expect("downloaded");
            assert_eq!(cache.read(&url).map(|e| e.body), Some(b"jpeg".to_vec()));
            assert!(file.is_file());
            server.verify().await;
        });
    }
}
