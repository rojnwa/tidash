//! Plain-data UI state. The messages that move between UI and backend live in `messages`,
//! re-exported here.

use crate::i18n::tr;
pub use crate::messages::{Action, Event, Redirect};
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

#[derive(Clone, Debug, Default)]
pub struct QueueSnapshot {
    pub tracks: Vec<tidal::Track>,
    pub current: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
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
    /// Shown until dismissed by id, so dismissals in one frame each find theirs.
    pub errors: Vec<(u64, String)>,
    /// The last id given to an error; kept across logout, as the errors are.
    error_id: u64,
    /// The app's, not the session's: kept across logout, as the devices are.
    pub settings: Settings,
    /// The output devices' names, as last listed (`Action::ListDevices`).
    pub devices: Vec<String>,
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
            errors: Vec::new(),
            error_id: 0,
            settings: Settings::default(),
            devices: Vec::new(),
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
            Event::Queue(q) => self.queue = q,
            Event::Playback(p) => self.playback = p,
            Event::Image { .. } | Event::ImageFailed { .. } => {} // the UI's texture cache
            Event::Player(_) => {}                                // kept by the UI
            Event::Seeked(_) => {}                                // handled by the media controls
            Event::Settings(s) => self.settings = s,
            Event::Devices(d) => self.devices = d,
            Event::Error(e) => self.error(e),
            Event::Navigate(r) => self.navigate(r),
            Event::Back => {
                self.back();
            }
            Event::Forward => {
                self.forward();
            }
            Event::DismissError(id) => self.errors.retain(|(e, _)| *e != id),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn logged_in() -> Model {
        let mut m = Model::default();
        m.apply(Event::LoggedIn(1));
        m
    }

    fn loaded(m: &mut Model, route: Route) {
        m.apply(Event::Page {
            route,
            data: PageData::Tracks(vec![]),
        });
    }

    fn errors(m: &Model) -> Vec<&str> {
        m.errors.iter().map(|(_, e)| e.as_str()).collect()
    }

    fn set(item: &tidal::Favorite, on: bool) -> Event {
        let item = item.clone();
        Event::FavoriteSet { item, on }
    }

    fn failed(item: &tidal::Favorite, on: bool) -> Event {
        let item = item.clone();
        Event::FavoriteFailed { item, on }
    }

    #[test]
    fn logged_in_starts_at_home() {
        let mut m = Model::default();
        assert_eq!(m.route(), &Route::Login);
        m.apply(Event::LoggedIn(7));
        assert_eq!(m.route(), &Route::Home);
        assert_eq!(m.history, [Route::Home]);
        assert!(m.logged_in);
        assert_eq!(m.user_id, Some(7));
    }

    #[test]
    fn expired_and_logged_out_reset_history_to_login() {
        for ev in [Event::LoggedOut, Event::SessionExpired] {
            let mut m = logged_in();
            m.navigate(Route::Album(1));
            m.navigate(Route::Album(2));
            m.back();
            loaded(&mut m, Route::Album(1));
            m.apply(set(&tidal::Favorite::Album(1), true));
            m.apply(ev);
            assert_eq!(m.route(), &Route::Login);
            assert_eq!((m.history.as_slice(), m.at), ([Route::Login].as_slice(), 0));
            assert!(!m.logged_in);
            assert!(!m.back() && !m.forward(), "no way past the login");
            assert!(m.pages.is_empty());
            assert!(m.pending_favorites.is_empty());
            assert_eq!(m.user_id, None);
        }
    }

    /// The settings and the device list are the app's, not the session's.
    #[test]
    fn settings_and_devices_survive_logout() {
        for out in [Event::LoggedOut, Event::SessionExpired] {
            let mut m = logged_in();
            let mut settings = Settings::default();
            settings.audio.exclusive = true;
            m.apply(Event::Settings(settings.clone()));
            m.apply(Event::Devices(vec!["DAC".into()]));
            m.apply(out);
            assert_eq!(m.settings, settings);
            assert_eq!(m.devices, ["DAC"]);
        }
    }

    #[test]
    fn an_expired_session_keeps_the_queue() {
        let mut m = logged_in();
        m.apply(Event::Queue(QueueSnapshot {
            current: Some(4),
            ..Default::default()
        }));
        m.apply(Event::Playback(PlaybackSnapshot {
            volume: 0.5,
            ..Default::default()
        }));
        m.apply(Event::SessionExpired);
        assert_eq!((m.queue.current, m.playback.volume), (Some(4), 0.5));
        assert_eq!(
            errors(&m),
            ["Your TIDAL session has expired; please log in again"]
        );
    }

    #[test]
    fn back_forward_and_truncate() {
        let mut m = logged_in();
        let (a, b, c) = (Route::Album(1), Route::Artist(2), Route::Mix("c".into()));
        for r in [&a, &b, &c] {
            m.navigate(r.clone());
        }
        assert!(m.back() && m.back());
        assert_eq!(m.route(), &a);
        assert!(m.forward());
        assert_eq!(m.route(), &b);
        assert!(m.back());
        let d = Route::Playlist("d".into());
        m.navigate(d.clone());
        m.navigate(d.clone()); // already there: no new entry
        assert_eq!(m.history, [Route::Home, a, d]);
        assert!(!m.forward(), "the forward entries are gone");
        assert!(m.back() && m.back() && !m.back());
        assert_eq!(m.route(), &Route::Home);
    }

    #[test]
    fn navigate_back_and_forward_events() {
        let mut m = logged_in();
        m.apply(Event::Navigate(Route::Album(7)));
        assert_eq!(m.route(), &Route::Album(7));
        m.apply(Event::Back);
        assert_eq!(m.route(), &Route::Home);
        m.apply(Event::Forward);
        assert_eq!(m.route(), &Route::Album(7));
    }

    #[test]
    fn history_caps_at_60() {
        let mut m = logged_in();
        for i in 0..100 {
            m.navigate(Route::Album(i));
        }
        assert_eq!((m.history.len(), m.at), (60, 59));
        assert_eq!(m.history[0], Route::Album(40));
        assert_eq!(m.route(), &Route::Album(99));
    }

    #[test]
    fn needs_load_rules() {
        let mut m = logged_in();
        let r = Route::Album(1);
        let now = Instant::now();
        let min = Duration::from_secs(60);
        assert!(m.needs_load(&r, now), "none");
        m.apply(Event::Loading(r.clone()));
        assert!(!m.needs_load(&r, now), "loading");
        m.apply(Event::PageFailed(r.clone()));
        assert!(m.needs_load(&r, now), "failed");
        m.apply(Event::Loading(r.clone()));
        loaded(&mut m, r.clone());
        assert!(!m.needs_load(&r, now + min), "fetched 1 min ago");
        assert!(m.needs_load(&r, now + 6 * min), "fetched 6 min ago");
        m.apply(Event::Loading(r.clone()));
        assert!(!m.needs_load(&r, now + 6 * min), "stale, already reloading");
        assert!(m.pages[&r].data.is_some(), "the stale copy stays meanwhile");
        for r in [Route::Search, Route::Login, Route::Settings] {
            assert!(!m.needs_load(&r, now), "{r:?}");
        }
    }

    #[test]
    fn page_evicts_lru_but_not_current() {
        let mut m = logged_in();
        m.navigate(Route::Album(0));
        for i in 0..25 {
            loaded(&mut m, Route::Album(i));
        }
        let held = |m: &Model| m.pages.values().filter(|s| s.data.is_some()).count();
        assert_eq!(held(&m), 24);
        assert!(
            m.pages[&Route::Album(0)].data.is_some(),
            "current, though oldest"
        );
        assert!(
            !m.pages.contains_key(&Route::Album(1)),
            "least recently used"
        );
        // A visit makes a page recent: the next evictions take albums 0 and 3, not 2.
        m.navigate(Route::Album(2));
        m.navigate(Route::Home);
        loaded(&mut m, Route::Album(25));
        loaded(&mut m, Route::Album(26));
        assert_eq!(held(&m), 24);
        assert!(m.pages.contains_key(&Route::Album(2)));
        assert!(!m.pages.contains_key(&Route::Album(0)));
        assert!(!m.pages.contains_key(&Route::Album(3)));
    }

    #[test]
    fn eviction_skips_pages_being_reloaded() {
        let mut m = logged_in();
        loaded(&mut m, Route::Album(0));
        m.apply(Event::Loading(Route::Album(0))); // stale: reloading
        for i in 1..=24 {
            loaded(&mut m, Route::Album(i));
        }
        assert!(
            m.pages.contains_key(&Route::Album(0)),
            "its reload is in flight"
        );
        assert!(!m.pages.contains_key(&Route::Album(1)));
    }

    /// Ruling R8: a page is asked for on arrival (a route change, `LoggedIn`) and on Retry;
    /// a failed one is not asked for again on the next pass.
    #[test]
    fn open_current_on_arrival_and_retry_only() {
        let now = Instant::now();
        let mut m = Model::default();
        assert_eq!(m.open_current(now), None, "the login has no page");
        m.apply(Event::LoggedIn(1));
        assert_eq!(m.open_current(now), Some(Route::Home));
        assert!(m.pages[&Route::Home].loading);
        let r = Route::Album(1);
        m.apply(Event::Navigate(r.clone()));
        assert_eq!(m.open_current(now), Some(r.clone()));
        assert_eq!(m.open_current(now), None, "next pass: asked already");
        m.apply(Event::PageFailed(r.clone()));
        assert_eq!(
            m.open_current(now),
            None,
            "a failed page waits for its Retry"
        );
        m.apply(Event::Navigate(r.clone())); // Retry
        assert_eq!(m.open_current(now), Some(r.clone()));
        assert!(m.pages[&r].loading && !m.pages[&r].failed);
    }

    /// A reply that lands after logout must not refill the store with that user's pages.
    #[test]
    fn replies_after_logout_are_dropped() {
        for out in [Event::LoggedOut, Event::SessionExpired] {
            let mut m = logged_in();
            m.apply(out);
            loaded(&mut m, Route::Collection(CollectionTab::Tracks));
            m.apply(Event::PageFailed(Route::Home));
            m.apply(Event::Favorites(tidal::FavoriteIds {
                tracks: [1].into(),
                ..Default::default()
            }));
            m.apply(Event::Offline(true));
            let item = tidal::Favorite::Track(1);
            m.apply(set(&item, true));
            m.apply(failed(&item, false));
            m.apply(Event::FavoriteSaved { item });
            m.apply(Event::SearchStarted("q".into()));
            let results = Default::default();
            m.apply(Event::SearchResults {
                query: "q".into(),
                results,
            });
            assert!(m.latest_query.is_none() && m.search.is_none());
            assert!(m.pages.is_empty());
            assert_eq!(m.favorites, tidal::FavoriteIds::default());
            assert!(m.pending_favorites.is_empty());
            assert!(!m.offline);
        }
    }

    /// The heart flips at click time; a write that fails flips it back.
    #[test]
    fn favorite_set_is_optimistic_and_failure_reverts() {
        let mut m = logged_in();
        let item = tidal::Favorite::Mix("m1".into());
        for on in [true, false] {
            m.apply(set(&item, on));
            assert_eq!(m.favorites.contains(&item), on, "shown at once");
            m.apply(failed(&item, on));
            assert_eq!(m.favorites.contains(&item), !on, "reverted");
            assert!(m.pending_favorites.is_empty());
        }
    }

    /// Ruling R19: one write per item at a time, so a failure always reverts to what TIDAL
    /// holds. A toggle while one is pending does nothing; saved, the item's tab is stale.
    #[test]
    fn one_favorite_write_per_item_at_a_time() {
        let mut m = logged_in();
        let tab = Route::Collection(CollectionTab::Albums);
        loaded(&mut m, tab.clone());
        let item = tidal::Favorite::Album(3);
        m.apply(set(&item, true));
        m.apply(set(&item, false)); // a second click while the first is pending
        assert!(m.favorites.contains(&item), "the second toggle did nothing");
        assert!(m.pending_favorites.contains(&item));
        // The ids loaded meanwhile predate the write: the heart keeps what was clicked.
        m.apply(Event::Favorites(tidal::FavoriteIds::default()));
        assert!(m.favorites.contains(&item));
        m.apply(failed(&item, true));
        assert!(!m.favorites.contains(&item), "back to what TIDAL holds");
        m.apply(set(&item, true));
        let now = Instant::now();
        assert!(!m.needs_load(&tab, now));
        m.apply(Event::FavoriteSaved { item: item.clone() });
        assert!(m.favorites.contains(&item) && m.pending_favorites.is_empty());
        assert!(m.needs_load(&tab, now), "the albums tab is stale");
    }

    #[test]
    fn offline_flag_follows_events() {
        let mut m = logged_in();
        m.apply(Event::Offline(true));
        assert!(m.offline);
        m.apply(Event::Offline(false));
        assert!(!m.offline);
    }

    #[test]
    fn stale_search_results_are_ignored() {
        let mut m = logged_in();
        let res = |q: &str| Event::SearchResults {
            query: q.into(),
            results: Default::default(),
        };
        m.apply(Event::SearchStarted("a".into()));
        m.apply(Event::SearchStarted("b".into()));
        m.apply(res("a"));
        assert!(m.search.is_none());
        m.apply(res("b"));
        assert_eq!(m.search.as_ref().map(|s| s.0.as_str()), Some("b"));
        m.apply(res("a"));
        assert_eq!(m.search.as_ref().map(|s| s.0.as_str()), Some("b"));
    }

    #[test]
    fn errors_are_collected() {
        let mut m = Model::default();
        m.apply(Event::Error("x".into()));
        assert_eq!(errors(&m), ["x"]);
    }

    /// Ids stay unique across logout, which keeps the errors shown.
    #[test]
    fn dismiss_error_removes_only_that_error() {
        let mut m = Model::default();
        for e in ["a", "b"] {
            m.apply(Event::Error(e.into()));
        }
        m.apply(Event::LoggedOut);
        m.apply(Event::Error("c".into()));
        let a = m.errors[0].0;
        m.apply(Event::DismissError(a));
        m.apply(Event::DismissError(a)); // gone already: nothing else goes
        assert_eq!(errors(&m), ["b", "c"]);
    }
}
