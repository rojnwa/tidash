//! Plain-data UI state. The messages that move between UI and backend live in `messages`,
//! re-exported here.

use crate::i18n::tr;
pub use crate::messages::{Action, Event, PlaylistEdit, Redirect, UpdateState};
use crate::queue::Repeat;
use crate::settings::Settings;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Entries kept for back and forward.
const HISTORY: usize = 60;
/// Pages whose data stays in memory.
const PAGES: usize = 24;
/// After this a shown page is fetched again.
const STALE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Login,
    Home,
    Search,
    Collection(CollectionTab),
    Album(u64),
    Artist(u64),
    Playlist(String),
    Mix(String),
    /// A v1 `apiPath`; Explore is `pages/explore`.
    Page(String),
    ViewAll(String),
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CollectionTab {
    Tracks,
    Albums,
    Artists,
    Playlists,
    Mixes,
}

impl CollectionTab {
    /// The tab that lists `item`.
    pub fn of(item: &tidal::Favorite) -> Self {
        match item {
            tidal::Favorite::Track(_) => Self::Tracks,
            tidal::Favorite::Album(_) => Self::Albums,
            tidal::Favorite::Artist(_) => Self::Artists,
            tidal::Favorite::Playlist(_) => Self::Playlists,
            tidal::Favorite::Mix(_) => Self::Mixes,
        }
    }
}

#[derive(Clone, Debug)]
pub enum PageData {
    Shelves(tidal::Page),
    Album {
        album: tidal::Album,
        tracks: Vec<tidal::Track>,
    },
    Artist(tidal::ArtistPage),
    Playlist {
        playlist: tidal::Playlist,
        tracks: Vec<tidal::Track>,
        /// Each track's position in the playlist, as edits address it.
        positions: Vec<u32>,
        /// The guard of edits by position; without it they are not offered.
        etag: Option<String>,
    },
    Mix {
        mix: tidal::Mix,
        tracks: Vec<tidal::Track>,
    },
    Tracks(Vec<tidal::Track>),
    Items(Vec<tidal::Item>),
}

/// One route's page: what was fetched last, and whether a fetch is under way or failed.
#[derive(Default)]
pub struct Slot {
    pub data: Option<PageData>,
    pub loading: bool,
    pub failed: bool,
    pub fetched: Option<Instant>,
    /// When last shown or filled, on the model's clock; the least recent goes first.
    pub used: u64,
}

/// Where playback started (spec §3.9): the page's route, and its title.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub route: Route,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct QueueSnapshot {
    pub tracks: Vec<tidal::Track>,
    pub current: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub source: Option<Source>,
}

/// A fetch for the side panel: the current track's lyrics or credits.
#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed,
}

/// What is heard and how: the stream's codec ("FLAC"), the track's format, and the output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputInfo {
    pub codec: String,
    pub format: player::Format,
    /// The device is held at the track's format: bit-perfect, volume fixed.
    pub exclusive: bool,
    pub device_rate: u32,
}

#[derive(Clone, Debug, Default)]
pub struct PlaybackSnapshot {
    pub state: Option<player::PlayState>,
    pub track: Option<tidal::Track>,
    pub volume: f32,
    /// Known once the current track is heard.
    pub output: Option<OutputInfo>,
}

pub struct Model {
    /// Never empty; `history[at]` is the current route.
    pub history: Vec<Route>,
    pub at: usize,
    pub pages: HashMap<Route, Slot>,
    /// Ticks on every visit or fill, for `Slot::used`.
    clock: u64,
    /// The current route was arrived at (or retried) since `open_current` last looked.
    arrived: bool,
    pub favorites: tidal::FavoriteIds,
    /// Favorites whose write is under way; one at a time per item, so a failure reverts the
    /// heart to what TIDAL holds (ruling R19).
    pub pending_favorites: HashSet<tidal::Favorite>,
    pub offline: bool,
    pub logged_in: bool,
    pub user_id: Option<u64>,
    pub login_url: Option<String>,
    pub search: Option<(String, tidal::SearchResults)>,
    pub latest_query: Option<String>,
    pub queue: QueueSnapshot,
    pub playback: PlaybackSnapshot,
    /// The lyrics and the credits of the track they were last asked for, the current one.
    pub lyrics: Option<(u64, Load<Option<tidal::Lyrics>>)>,
    pub credits: Option<(u64, Load<Vec<tidal::Credit>>)>,
    /// Shown until dismissed by id, so dismissals in one frame each find theirs.
    pub errors: Vec<(u64, String)>,
    /// The last id given to an error; kept across logout, as the errors are.
    error_id: u64,
    /// The app's, not the session's: kept across logout, as the devices are.
    pub settings: Settings,
    /// The output devices' names, as last listed (`Action::ListDevices`).
    pub devices: Vec<String>,
    /// The library tree: each listed folder's items, by folder id (`"root"`: the top level).
    pub folders: HashMap<String, Vec<tidal::FolderItem>>,
    /// The playlists the user made, last updated first.
    pub own_playlists: Vec<tidal::Playlist>,
    /// A Last.fm connect waits for the user to allow Tidash; kept across logout, as the
    /// settings are.
    pub lastfm_waiting: bool,
    /// The palette files the theme picker lists, and what to say under it (`Event::Themes`);
    /// kept across logout, as the settings are.
    pub themes: Vec<String>,
    pub theme_status: Option<fastframe_theme::Status>,
    /// Where an update stands; `None` until a check is asked for or finds one. Kept across
    /// logout, as the settings are.
    pub update: Option<UpdateState>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            history: vec![Route::Login],
            at: 0,
            pages: HashMap::new(),
            clock: 0,
            arrived: false,
            favorites: tidal::FavoriteIds::default(),
            pending_favorites: HashSet::new(),
            offline: false,
            logged_in: false,
            user_id: None,
            login_url: None,
            search: None,
            latest_query: None,
            queue: QueueSnapshot::default(),
            playback: PlaybackSnapshot::default(),
            lyrics: None,
            credits: None,
            errors: Vec::new(),
            error_id: 0,
            settings: Settings::default(),
            devices: Vec::new(),
            folders: HashMap::new(),
            own_playlists: Vec::new(),
            lastfm_waiting: false,
            themes: Vec::new(),
            theme_status: None,
            update: None,
        }
    }
}

impl Model {
    pub fn apply(&mut self, ev: Event) {
        // A reply that lands after logout must not refill the store with that user's pages.
        let reply = matches!(
            ev,
            Event::Page { .. }
                | Event::PageFailed(_)
                | Event::Favorites(_)
                | Event::FavoriteSet { .. }
                | Event::FavoriteSaved { .. }
                | Event::FavoriteFailed { .. }
                | Event::Offline(_)
                | Event::SearchStarted(_)
                | Event::SearchResults { .. }
                | Event::Lyrics(..)
                | Event::Credits(..)
                | Event::Folder { .. }
                | Event::OwnPlaylists(_)
                | Event::PlaylistChanged(_)
                | Event::PlaylistDeleted(_)
        );
        if reply && !self.logged_in {
            return;
        }
        match ev {
            Event::LoginUrl(u) => self.login_url = Some(u),
            Event::LoggedIn(user) => {
                self.logged_in = true;
                self.user_id = Some(user);
                self.login_url = None;
                self.history = vec![Route::Home];
                self.at = 0;
                self.touch();
            }
            Event::LoggedOut => {
                *self = Self {
                    errors: std::mem::take(&mut self.errors),
                    error_id: self.error_id,
                    settings: std::mem::take(&mut self.settings),
                    devices: std::mem::take(&mut self.devices),
                    lastfm_waiting: self.lastfm_waiting,
                    themes: std::mem::take(&mut self.themes),
                    theme_status: self.theme_status,
                    update: self.update.take(),
                    ..Self::default()
                };
            }
            Event::SessionExpired => {
                *self = Self {
                    queue: std::mem::take(&mut self.queue),
                    playback: std::mem::take(&mut self.playback),
                    errors: std::mem::take(&mut self.errors),
                    error_id: self.error_id,
                    settings: std::mem::take(&mut self.settings),
                    devices: std::mem::take(&mut self.devices),
                    lastfm_waiting: self.lastfm_waiting,
                    themes: std::mem::take(&mut self.themes),
                    theme_status: self.theme_status,
                    update: self.update.take(),
                    ..Self::default()
                };
                self.error(tr("Your TIDAL session has expired; please log in again").into_owned());
            }
            Event::CredentialsNotPersistent => self.error(
                tr("Login will not be remembered: no system credential store available")
                    .into_owned(),
            ),
            Event::SearchStarted(q) => self.latest_query = Some(q),
            Event::SearchResults { query, results } => {
                if self.latest_query.as_deref() == Some(query.as_str()) {
                    self.search = Some((query, results));
                }
            }
            Event::Loading(r) => {
                let slot = self.pages.entry(r).or_default();
                slot.loading = true;
                slot.failed = false;
            }
            Event::Page { route, data } => {
                self.clock += 1;
                let slot = self.pages.entry(route).or_default();
                *slot = Slot {
                    data: Some(data),
                    fetched: Some(Instant::now()),
                    used: self.clock,
                    ..Slot::default()
                };
                self.evict();
            }
            Event::PageFailed(r) => {
                let slot = self.pages.entry(r).or_default();
                slot.loading = false;
                slot.failed = true;
            }
            Event::Offline(o) => self.offline = o,
            Event::Favorites(mut f) => {
                // Loaded before these writes landed: their hearts stay as clicked.
                for item in &self.pending_favorites {
                    f.set(item, self.favorites.contains(item));
                }
                self.favorites = f;
            }
            Event::FavoriteSet { item, on } => {
                if !self.pending_favorites.contains(&item) {
                    self.favorites.set(&item, on);
                    self.pending_favorites.insert(item);
                }
            }
            Event::FavoriteSaved { item } => {
                let tab = Route::Collection(CollectionTab::of(&item));
                if let Some(slot) = self.pages.get_mut(&tab) {
                    slot.fetched = None;
                }
                self.pending_favorites.remove(&item);
            }
            Event::FavoriteFailed { item, on } => {
                self.favorites.set(&item, !on);
                self.pending_favorites.remove(&item);
            }
            Event::Lyrics(id, load) => fill(&mut self.lyrics, id, load),
            Event::Credits(id, load) => fill(&mut self.credits, id, load),
            Event::Folder { id, items } => {
                self.folders.insert(id, items);
            }
            Event::OwnPlaylists(p) => self.own_playlists = p,
            Event::PlaylistChanged(uuid) => {
                let r = Route::Playlist(uuid);
                if let Some(slot) = self.pages.get_mut(&r) {
                    slot.fetched = None;
                }
                self.arrived |= *self.route() == r;
            }
            Event::PlaylistDeleted(uuid) => {
                let r = Route::Playlist(uuid);
                if *self.route() == r {
                    if self.back() {
                        self.history.remove(self.at + 1); // no Forward to it
                    } else {
                        self.history[self.at] = Route::Home;
                        self.touch();
                    }
                }
                self.pages.remove(&r);
            }
            Event::Queue(q) => self.queue = q,
            Event::Playback(p) => self.playback = p,
            Event::Image { .. } | Event::ImageFailed { .. } => {} // the UI's texture cache
            Event::Player(_) => {}                                // kept by the UI
            Event::Seeked(_) => {}                                // handled by the media controls
            Event::Settings(s) => self.settings = s,
            Event::Devices(d) => self.devices = d,
            Event::LastfmWaiting(w) => self.lastfm_waiting = w,
            Event::Update(s) => self.update = Some(s),
            Event::Themes { names, status } => (self.themes, self.theme_status) = (names, status),
            Event::RescanThemes => {} // the app's (main.rs)
            Event::Error(e) => self.error(e),
            Event::Navigate(r) => self.navigate(r),
            Event::Back => {
                self.back();
            }
            Event::Forward => {
                self.forward();
            }
            Event::DismissError(id) => self.errors.retain(|(e, _)| *e != id),
            Event::Quit => {} // the app's (main.rs)
        }
    }

    fn error(&mut self, e: String) {
        self.error_id += 1;
        self.errors.push((self.error_id, e));
    }

    pub fn route(&self) -> &Route {
        &self.history[self.at]
    }

    /// Goes to `r`, dropping the forward entries; the oldest entry goes past `HISTORY`.
    /// Going to the current route asks for its page again (`open_current`): Retry does that.
    pub fn navigate(&mut self, r: Route) {
        if r == *self.route() {
            self.arrived = true;
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(r);
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.at = self.history.len() - 1;
        self.touch();
    }

    pub fn back(&mut self) -> bool {
        let moved = self.at > 0;
        if moved {
            self.at -= 1;
            self.touch();
        }
        moved
    }

    pub fn forward(&mut self) -> bool {
        let moved = self.at + 1 < self.history.len();
        if moved {
            self.at += 1;
            self.touch();
        }
        moved
    }

    /// Whether the app should ask for `r`: never fetched, failed, or stale and not reloading.
    /// Login, Search and Settings have no page.
    pub fn needs_load(&self, r: &Route, now: Instant) -> bool {
        if matches!(r, Route::Login | Route::Search | Route::Settings) {
            return false;
        }
        self.pages.get(r).is_none_or(|s| {
            let stale = s
                .fetched
                .is_none_or(|t| now.saturating_duration_since(t) > STALE);
            s.failed || !s.loading && (s.data.is_none() || stale)
        })
    }

    /// The route the app must send `Action::Open` for, now marked loading: the current one,
    /// once per arrival or Retry, if it `needs_load`. Between them nothing is asked for, so a
    /// failed page waits for its Retry instead of failing again every pass (ruling R8).
    pub fn open_current(&mut self, now: Instant) -> Option<Route> {
        let r = self.route().clone();
        if !std::mem::take(&mut self.arrived) || !self.needs_load(&r, now) {
            return None;
        }
        self.apply(Event::Loading(r.clone()));
        Some(r)
    }

    /// Arrived at the current route: its page is just used, and opened if need be.
    fn touch(&mut self) {
        self.arrived = true;
        self.clock += 1;
        let r = self.route().clone();
        self.pages.entry(r).or_default().used = self.clock;
    }

    /// Drops the least recently used pages beyond `PAGES`, never the current one nor one
    /// whose reload is in flight.
    fn evict(&mut self) {
        while self.pages.values().filter(|s| s.data.is_some()).count() > PAGES {
            let oldest = self
                .pages
                .iter()
                .filter(|(r, s)| s.data.is_some() && !s.loading && *r != self.route())
                .min_by_key(|(_, s)| s.used)
                .map(|(r, _)| r.clone());
            let Some(r) = oldest else { break };
            self.pages.remove(&r);
        }
    }
}

/// Track `id`'s `load` into `slot`: a `Loading` takes it; a reply lands only on its own track's.
fn fill<T>(slot: &mut Option<(u64, Load<T>)>, id: u64, load: Load<T>) {
    if matches!(load, Load::Loading) || slot.as_ref().is_some_and(|(at, _)| *at == id) {
        *slot = Some((id, load));
    }
}

#[cfg(test)]
#[path = "model/tests.rs"]
mod tests;
