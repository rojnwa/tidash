//! The messages that move between UI and backend.

use crate::model::{Load, PageData, PlaybackSnapshot, QueueSnapshot, Route};
use crate::queue::Repeat;
use std::time::Duration;

/// The login redirect URL. It carries the authorization code: `Debug` hides it.
#[derive(Clone)]
pub struct Redirect(pub String);

impl std::fmt::Debug for Redirect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    BeginLogin,
    FinishLogin(Redirect),
    Logout,
    Search(String),
    Open(Route),
    PlayTracks {
        tracks: Vec<tidal::Track>,
        start: usize,
    },
    Play,
    Pause,
    /// Pause if playing or loading, else play: the backend decides, from its own state.
    TogglePlay,
    Next,
    Previous,
    Seek(Duration),
    SetVolume(f32),
    /// Right after the current track.
    PlayNext(Vec<tidal::Track>),
    AddToQueue(Vec<tidal::Track>),
    /// `to` is a slot, `0..=len`, counted before the move.
    MoveInQueue {
        from: usize,
        to: usize,
    },
    /// Removing the current track: the next one takes its place, playing or paused as it was.
    RemoveFromQueue(usize),
    PlayQueueIndex(usize),
    SetShuffle(bool),
    SetRepeat(Repeat),
    /// `image` at most `px` wide.
    LoadImage {
        image: tidal::Image,
        px: u32,
    },
    /// Adds (`on`) or removes a favorite; the UI has applied `Event::FavoriteSet` already.
    SetFavorite {
        item: tidal::Favorite,
        on: bool,
    },
    /// Saves these settings and works by them from now on; the UI has applied
    /// `Event::Settings` already. Another client ID logs out.
    SetSettings(crate::settings::Settings),
    /// Lists the output devices (`Event::Devices`).
    ListDevices,
    /// Fetches this track's lyrics (`Event::Lyrics`), for the side panel.
    LoadLyrics(u64),
    /// Fetches this track's credits (`Event::Credits`), for the side panel.
    LoadCredits(u64),
}

#[derive(Clone, Debug)]
pub enum Event {
    LoginUrl(String),
    /// Logged in as this user id.
    LoggedIn(u64),
    LoggedOut,
    /// The session is dead (its token refresh failed): log in again; the queue stays.
    SessionExpired,
    CredentialsNotPersistent,
    /// A search was submitted; results for any other query are stale.
    SearchStarted(String),
    SearchResults {
        query: String,
        results: tidal::SearchResults,
    },
    /// The app asked the backend for this page.
    Loading(Route),
    Page {
        route: Route,
        data: PageData,
    },
    PageFailed(Route),
    Offline(bool),
    Favorites(tidal::FavoriteIds),
    /// The heart shows `on` at once, and stays until the write (`Action::SetFavorite`) is done.
    FavoriteSet {
        item: tidal::Favorite,
        on: bool,
    },
    /// The write is done: the item's collection tab is fetched again on the next visit.
    FavoriteSaved {
        item: tidal::Favorite,
    },
    /// Setting the favorite to `on` failed: the heart goes back.
    FavoriteFailed {
        item: tidal::Favorite,
        on: bool,
    },
    Queue(QueueSnapshot),
    Playback(PlaybackSnapshot),
    /// Playback jumped to this position (for the OS media controls).
    Seeked(Duration),
    /// The artwork of `tidal::Image::key` `key`, at most `px` wide.
    Image {
        key: String,
        px: u32,
        image: egui::ColorImage,
    },
    /// The artwork could not be loaded; it may be requested again.
    ImageFailed {
        key: String,
        px: u32,
    },
    /// The audio output is open; the UI reads the position from it.
    Player(player::Player),
    /// The settings in force: read at start; on a change, from the UI at once and from the
    /// backend once saved.
    Settings(crate::settings::Settings),
    /// The output devices' names; empty when they could not be listed.
    Devices(Vec<String>),
    /// This track's lyrics: `Loading` from the UI as it asks, then the backend's reply, which
    /// a newer ask for another track makes stale.
    Lyrics(u64, Load<Option<tidal::Lyrics>>),
    /// This track's credits, as `Lyrics`.
    Credits(u64, Load<Vec<tidal::Credit>>),
    Error(String),
    /// Dismiss the error with this id (`Model::errors`); one already gone is ignored.
    DismissError(u64),
    Navigate(Route),
    Back,
    Forward,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_login_debug_hides_the_code() {
        let a = Action::FinishLogin(Redirect(
            "https://tidal.com/android/login/auth?code=SECRET123&state=s".into(),
        ));
        assert!(!format!("{a:?}").contains("SECRET123"));
    }
}
