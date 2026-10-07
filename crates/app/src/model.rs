//! Plain-data UI state and the messages that move between UI and backend.
// ponytail: the search/album/queue/playback parts are consumed by tasks 18-20.
#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Login,
    Search,
    Album(u64),
}

#[derive(Clone, Debug)]
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
                .push("Login will not be remembered: no system credential store available".into()),
            Event::SearchResults { query, results } => self.search = Some((query, results)),
            Event::Album { album, tracks } => {
                self.albums.insert(album.id, (album, tracks));
            }
            Event::Queue(q) => self.queue = q,
            Event::Playback(p) => self.playback = p,
            Event::Image { .. } => {} // handled by the UI's texture cache
            Event::Error(e) => self.errors.push(e),
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
    fn errors_are_collected() {
        let mut m = Model::default();
        m.apply(Event::Error("x".into()));
        assert_eq!(m.errors, vec!["x".to_string()]);
    }
}
