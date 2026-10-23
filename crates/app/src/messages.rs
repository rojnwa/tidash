//! The messages that move between UI and backend.

use crate::model::{Load, PageData, PlaybackSnapshot, QueueSnapshot, Route, Source};
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

/// A token as the user typed it: `Debug` hides it.
#[derive(Clone)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
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
    /// A track link: goes to the track's album (`Event::Navigate`).
    OpenTrack(u64),
    PlayTracks {
        tracks: Vec<tidal::Track>,
        start: usize,
        /// The page they are played from; none without one to go back to.
        source: Option<Source>,
    },
    /// Fetches the tracks of `Source::route` (an album, playlist or mix) and plays them from
    /// the start, from that page. A newer play supersedes it while it fetches.
    PlaySource(Source),
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
    /// Makes the edit, then reloads the library tree and the own playlists.
    EditPlaylist(PlaylistEdit),
    /// Lists this folder for the library tree (`Event::Folder`).
    LoadFolder(String),
    /// Checks this ListenBrainz user token, then keeps it and sets the user in the settings;
    /// a token refused is an `Error`.
    ConnectListenBrainz(Secret),
    /// Forgets the token, the user, and the listens still queued for ListenBrainz.
    DisconnectListenBrainz,
    /// Opens the Last.fm page where the user allows Tidash and waits for them
    /// (`Event::LastfmWaiting`); then keeps the session key and sets the user in the settings.
    /// A failure is an `Error`.
    ConnectLastfm,
    /// Stops waiting for the user to allow Tidash on Last.fm.
    CancelLastfm,
    /// Forgets the session key, the user, and the scrobbles still queued for Last.fm.
    DisconnectLastfm,
    /// Creates the themes folder if need be and opens it in the file manager.
    OpenThemesFolder,
    /// Checks for a newer release now and downloads it (`Event::Update`), whatever the
    /// Automatic updates setting says.
    CheckForUpdates,
    /// Hands the downloaded update to the helper, then quits (`Event::Quit`).
    InstallUpdate,
}

/// Where an update stands, for Settings › Updates (`Model::update`).
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    /// A check the user asked for.
    Checking,
    /// The check the user asked for found nothing newer.
    UpToDate,
    /// A newer release downloading, `percent` of it so far.
    Downloading { version: String, percent: u8 },
    /// Downloaded and verified: `Action::InstallUpdate` installs it.
    Ready { version: String },
    /// A newer release this copy cannot install itself; `reason` says how to get it instead.
    Unsupported {
        version: String,
        reason: fastframe_update::Unsupported,
    },
    /// A failed check is reported only when the user asked for it; a failed download (an
    /// automatic one too) or install always is.
    Failed(UpdateStep),
}

/// What an update was doing when it failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStep {
    Check,
    Download,
    Install,
}

/// A change to the user's playlists or folders. `folder` and `parent` are folder ids, `None`
/// for the top level.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistEdit {
    /// The tracks go in once it exists.
    Create {
        title: String,
        description: String,
        folder: Option<String>,
        tracks: Vec<u64>,
    },
    Edit {
        uuid: String,
        title: String,
        description: String,
    },
    Delete {
        uuid: String,
    },
    /// Tracks already in are skipped.
    Add {
        uuid: String,
        tracks: Vec<u64>,
    },
    /// `positions` and `etag` come from one listing (`PageData::Playlist`).
    Remove {
        uuid: String,
        etag: String,
        positions: Vec<u32>,
    },
    /// `to` is a slot, `0..=len`, counted before the move; `etag` as for `Remove`.
    Move {
        uuid: String,
        etag: String,
        from: u32,
        to: u32,
    },
    CreateFolder {
        name: String,
        parent: Option<String>,
    },
    RenameFolder {
        id: String,
        name: String,
    },
    DeleteFolder {
        id: String,
    },
    MoveToFolder {
        uuid: String,
        folder: Option<String>,
    },
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
    /// The settings in force after a change: from the UI at once and from the backend once
    /// saved. `main` reads them at start.
    Settings(crate::settings::Settings),
    /// The output devices' names; empty when they could not be listed.
    Devices(Vec<String>),
    /// This track's lyrics: `Loading` from the UI as it asks, then the backend's reply, which
    /// a newer ask for another track makes stale.
    Lyrics(u64, Load<Option<tidal::Lyrics>>),
    /// This track's credits, as `Lyrics`.
    Credits(u64, Load<Vec<tidal::Credit>>),
    /// Folder `id`'s items (`"root"`: the top level), for the library tree.
    Folder {
        id: String,
        items: Vec<tidal::FolderItem>,
    },
    /// The playlists the user made, last updated first.
    OwnPlaylists(Vec<tidal::Playlist>),
    /// This playlist changed: its page is fetched again (now, if shown).
    PlaylistChanged(String),
    /// This playlist is gone: so is its page, which, if shown, goes back.
    PlaylistDeleted(String),
    /// A Last.fm connect waits for the user to allow Tidash, or no longer.
    LastfmWaiting(bool),
    /// From the update thread (`update`).
    Update(UpdateState),
    /// What the theme picker lists (the catalog's palette files, by filename) and what to say
    /// under it; `main` applies it whenever either changes.
    Themes {
        names: Vec<String>,
        status: Option<fastframe_theme::Status>,
    },
    /// List the themes folder again (Settings opened): `main`'s.
    RescanThemes,
    Error(String),
    /// A short confirmation of a write with no visible result: the UI's (`UiState::apply`).
    Toast(String),
    /// Dismiss the error with this id (`Model::errors`); one already gone is ignored.
    DismissError(u64),
    Navigate(Route),
    Back,
    Forward,
    /// End the app (Ctrl+Q): the window closes and nothing stays in the tray.
    Quit,
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
        let a = Action::ConnectListenBrainz(Secret("SECRET456".into()));
        assert!(!format!("{a:?}").contains("SECRET456"));
    }
}
