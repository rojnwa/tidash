//! Plain-data UI state and the messages that move between UI and backend.
// ponytail: the search/album/queue/playback parts are consumed by tasks 18-20.
#![allow(dead_code)]

use crate::i18n::tr;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Login,
    Search,
    Album(u64),
}

#[derive(Clone)]
pub enum Action {
    BeginLogin,
    FinishLogin(String),
    Logout,
    Search(String),
    OpenAlbum(u64),
    PlayTracks { tracks: Vec<tidal::Track>, start: usize },
    Play,
    Pause,
    Next,
    Previous,
    Seek(Duration),
    SetVolume(f32),
    LoadImage { cover: String, size: u32 },
}

// Debug is manual: `FinishLogin` carries the redirect URL with the authorization code.
impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeginLogin => f.write_str("BeginLogin"),
            Self::FinishLogin(_) => f.write_str("FinishLogin(<redacted>)"),
            Self::Logout => f.write_str("Logout"),
            Self::Search(q) => f.debug_tuple("Search").field(q).finish(),
            Self::OpenAlbum(id) => f.debug_tuple("OpenAlbum").field(id).finish(),
            Self::PlayTracks { tracks, start } => {
                f.debug_struct("PlayTracks").field("tracks", tracks).field("start", start).finish()
            }
            Self::Play => f.write_str("Play"),
            Self::Pause => f.write_str("Pause"),
            Self::Next => f.write_str("Next"),
            Self::Previous => f.write_str("Previous"),
            Self::Seek(d) => f.debug_tuple("Seek").field(d).finish(),
            Self::SetVolume(v) => f.debug_tuple("SetVolume").field(v).finish(),
            Self::LoadImage { cover, size } => {
                f.debug_struct("LoadImage").field("cover", cover).field("size", size).finish()
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    LoginUrl(String),
    LoggedIn,
    LoggedOut,
    CredentialsNotPersistent,
    SearchResults { query: String, results: tidal::SearchResults },
    Album { album: tidal::Album, tracks: Vec<tidal::Track> },
    Queue(QueueSnapshot),
    Playback(PlaybackSnapshot),
    Image { cover: String, size: u32, image: egui::ColorImage },
    Error(String),
    /// Dismiss the error at this index (out of range is ignored).
    DismissError(usize),
    Navigate(Route),
    Back,
}

#[derive(Clone, Debug, Default)]
pub struct QueueSnapshot {
    pub tracks: Vec<tidal::Track>,
    pub current: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct PlaybackSnapshot {
    pub state: Option<player::PlayState>,
    pub track: Option<tidal::Track>,
    pub volume: f32,
}

pub struct Model {
    pub route: Route,
    pub history: Vec<Route>,
    pub logged_in: bool,
    pub login_url: Option<String>,
    pub search: Option<(String, tidal::SearchResults)>,
    pub albums: HashMap<u64, (tidal::Album, Vec<tidal::Track>)>,
    pub queue: QueueSnapshot,
    pub playback: PlaybackSnapshot,
    pub errors: Vec<String>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            route: Route::Login,
            history: Vec::new(),
            logged_in: false,
            login_url: None,
            search: None,
            albums: HashMap::new(),
            queue: QueueSnapshot::default(),
            playback: PlaybackSnapshot::default(),
            errors: Vec::new(),
        }
    }
}

impl Model {
    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::LoginUrl(u) => self.login_url = Some(u),
            Event::LoggedIn => {
                self.logged_in = true;
                self.login_url = None;
                self.history.clear();
                self.route = Route::Search;
            }
            Event::LoggedOut => {
                *self = Self { errors: std::mem::take(&mut self.errors), ..Self::default() };
            }
            Event::CredentialsNotPersistent => self
                .errors
                .push(tr("Login will not be remembered: no system credential store available").into_owned()),
            Event::SearchResults { query, results } => self.search = Some((query, results)),
            Event::Album { album, tracks } => {
                self.albums.insert(album.id, (album, tracks));
            }
            Event::Queue(q) => self.queue = q,
            Event::Playback(p) => self.playback = p,
            Event::Image { .. } => {} // handled by the UI's texture cache
            Event::Error(e) => self.errors.push(e),
            Event::Navigate(r) => self.navigate(r),
            Event::Back => {
                self.back();
            }
            Event::DismissError(i) => {
                if i < self.errors.len() {
                    self.errors.remove(i);
                }
            }
        }
    }

    pub fn navigate(&mut self, r: Route) {
        if r != self.route {
            self.history.push(self.route);
            self.route = r;
        }
    }

    pub fn back(&mut self) -> bool {
        match self.history.pop() {
            Some(r) => {
                self.route = r;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_in_moves_to_search() {
        let mut m = Model::default();
        assert_eq!(m.route, Route::Login);
        m.apply(Event::LoggedIn);
        assert_eq!(m.route, Route::Search);
        assert!(m.logged_in);
    }

    #[test]
    fn logged_out_returns_to_login() {
        let mut m = Model::default();
        m.apply(Event::LoggedIn);
        m.apply(Event::LoggedOut);
        assert_eq!(m.route, Route::Login);
        assert!(!m.logged_in);
    }

    #[test]
    fn navigate_and_back() {
        let mut m = Model::default();
        m.apply(Event::LoggedIn);
        m.navigate(Route::Album(1));
        m.navigate(Route::Album(2));
        assert!(m.back());
        assert!(m.back());
        assert_eq!(m.route, Route::Search);
        assert!(!m.back());
    }

    #[test]
    fn navigate_and_back_events() {
        let mut m = Model::default();
        m.apply(Event::LoggedIn);
        m.apply(Event::Navigate(Route::Album(7)));
        assert_eq!(m.route, Route::Album(7));
        m.apply(Event::Back);
        assert_eq!(m.route, Route::Search);
    }

    #[test]
    fn errors_are_collected() {
        let mut m = Model::default();
        m.apply(Event::Error("x".into()));
        assert_eq!(m.errors, vec!["x".to_string()]);
    }

    #[test]
    fn dismiss_error_removes_only_that_error() {
        let mut m = Model::default();
        for e in ["a", "b", "c"] {
            m.apply(Event::Error(e.into()));
        }
        m.apply(Event::DismissError(1));
        m.apply(Event::DismissError(9));
        assert_eq!(m.errors, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn finish_login_debug_hides_the_code() {
        let a = Action::FinishLogin("https://tidal.com/android/login/auth?code=SECRET123&state=s".into());
        assert!(!format!("{a:?}").contains("SECRET123"));
    }
}
