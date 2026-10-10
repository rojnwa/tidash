//! Discord Rich Presence: "Listening to Tidash" with the track, its artists, its cover and
//! its progress. Driven from the UI thread like the media controls; the IPC blocks without
//! timeouts, so it runs on a thread of its own.

use crate::i18n::tr;
use crate::model::PlaybackSnapshot;
use discord_rich_presence::activity::{Activity, ActivityType, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use player::PlayState;
use reqwest::Url;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The Discord application, whose name is the "Listening to …"; without it, no presence and
/// no toggle.
const APP_ID: Option<&str> = option_env!("TIDASH_DISCORD_APP_ID");
const COVER_PX: u32 = 640;
/// While there is something to show and Discord is not there, it is looked for this often.
const RETRY: Duration = Duration::from_secs(30);
/// A position this far (ms) from where the last one sent would be by now is a seek.
const JUMP_MS: i64 = 2_000;

/// `APP_ID`, unless blank.
pub fn app_id() -> Option<&'static str> {
    APP_ID.map(str::trim).filter(|id| !id.is_empty())
}

/// What the presence shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Rich {
    details: String,
    state: String,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    image: Option<String>,
    album: String,
    url: String,
}

pub struct Presence {
    image_base: Url,
    send: Box<dyn FnMut(Option<Rich>)>,
    /// What was last sent; `None` is nothing, or a clear.
    last: Option<Rich>,
}

impl Presence {
    /// Spawns the `discord-presence` thread, which talks to Discord as `app_id`. Covers come
    /// from `image_base`. The thread ends once this is dropped.
    pub fn start(app_id: &'static str, image_base: Url) -> Self {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("discord-presence".into())
            .spawn(move || run(app_id, &rx));
        if let Err(e) = spawned {
            log::warn!("Discord presence thread failed: {e}");
        }
        Self::new(image_base, move |rich| {
            let _ = tx.send(rich);
        })
    }

    fn new(image_base: Url, send: impl FnMut(Option<Rich>) + 'static) -> Self {
        Self {
            image_base,
            send: Box::new(send),
            last: None,
        }
    }

    /// Shows a playback change, or a jump to `position` (the player's, now); clears the
    /// presence unless `enabled`. Sends only what Discord would show differently.
    pub fn show(&mut self, pb: &PlaybackSnapshot, position: Duration, enabled: bool) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let now_ms = i64::try_from(now.as_millis()).unwrap_or(i64::MAX);
        let mut rich = activity_for(pb, position, now_ms, &self.image_base).filter(|_| enabled);
        // Its timestamps run on by themselves: on time, the last ones stand.
        if let (Some(new), Some(old)) = (&mut rich, &self.last)
            && let (Some(a), Some(b)) = (new.start_ms, old.start_ms)
            && (a - b).abs() <= JUMP_MS
        {
            (new.start_ms, new.end_ms) = (old.start_ms, old.end_ms);
        }
        if rich != self.last {
            self.last.clone_from(&rich);
            (self.send)(rich);
        }
    }
}

/// The presence for `pb` at `position`, `now_ms` being the Unix time in ms; `None` clears it.
pub fn activity_for(
    pb: &PlaybackSnapshot,
    position: Duration,
    now_ms: i64,
    image_base: &Url,
) -> Option<Rich> {
    let t = pb.track.as_ref()?;
    let playing = match pb.state? {
        PlayState::Playing => true,
        PlayState::Paused | PlayState::Buffering => false,
        PlayState::Stopped => return None,
    };
    let at = i64::try_from(position.as_millis()).unwrap_or(i64::MAX);
    let start = now_ms.saturating_sub(at);
    let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
    Some(Rich {
        details: text(&t.title),
        state: text(&artists.join(", ")),
        start_ms: playing.then_some(start),
        end_ms: playing.then(|| start + i64::from(t.duration) * 1000),
        image: t.album.image().map(|i| i.url(image_base, COVER_PX)),
        album: text(&t.album.title),
        url: format!("https://tidal.com/browse/track/{}", t.id),
    })
}

/// `s` within Discord's 2 to 128: cut on a char boundary at 128 UTF-16 units (at most 128
/// however Discord counts), padded with zero-width spaces, which do not show.
fn text(s: &str) -> String {
    let mut units = 0;
    let fits = |c: &char| {
        units += c.len_utf16();
        units <= 128
    };
    let mut t: String = s.chars().take_while(fits).collect();
    while t.chars().count() < 2 {
        t.push('\u{200B}');
    }
    t
}

/// The thread: shows the latest of what `rx` brings, connecting while there is something to
/// show; once `rx` closes, clears it and ends.
fn run(app_id: &str, rx: &Receiver<Option<Rich>>) {
    let mut client = DiscordIpcClient::new(app_id);
    let (mut connected, mut want, mut dirty) = (false, None, false);
    loop {
        match rx.recv_timeout(RETRY) {
            Ok(rich) => (want, dirty) = (rx.try_iter().last().unwrap_or(rich), true),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !dirty || (!connected && want.is_none()) {
            dirty = false; // nothing shown on a closed connection: nothing to clear
            continue;
        }
        if !connected {
            // `reconnect` would fail on a client never connected.
            match client.connect() {
                Ok(()) => connected = true,
                Err(e) => {
                    log::debug!("Discord not reached: {e}");
                    continue;
                }
            }
        }
        let sent = match &want {
            Some(rich) => client.set_activity(activity(rich)),
            None => client.clear_activity(),
        };
        // Discord answers each command; read, it never piles up.
        match sent.and_then(|()| client.recv()) {
            Ok((_, reply)) if reply["evt"] == "ERROR" => {
                log::warn!("Discord refused the presence: {}", reply["data"]);
                dirty = false;
            }
            Ok(_) => dirty = false,
            Err(e) => {
                log::debug!("Discord presence not sent, reconnecting: {e}");
                connected = false;
            }
        }
    }
    if connected {
        let _ = client.clear_activity();
        let _ = client.close();
    }
}

fn activity(r: &Rich) -> Activity<'static> {
    let mut assets = Assets::new().large_text(r.album.clone());
    if let Some(image) = &r.image {
        assets = assets.large_image(image.clone());
    }
    let button = Button::new(tr("Open in TIDAL"), r.url.clone());
    let activity = Activity::new()
        .activity_type(ActivityType::Listening)
        .details(r.details.clone())
        .state(r.state.clone())
        .assets(assets)
        .buttons(vec![button]);
    match r.start_ms.zip(r.end_ms) {
        Some((start, end)) => activity.timestamps(Timestamps::new().start(start).end(end)),
        None => activity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const NOW: i64 = 1_800_000_000_000;

    fn base() -> Url {
        "https://resources.tidal.com/images/"
            .parse()
            .expect("a URL")
    }

    fn artist(name: &str) -> tidal::Artist {
        tidal::Artist {
            id: 1,
            name: name.into(),
            picture: None,
            selected_album_cover_fallback: None,
            mixes: Default::default(),
        }
    }

    fn snapshot(state: PlayState) -> PlaybackSnapshot {
        let track = tidal::Track {
            id: 7,
            title: "Song".into(),
            duration: 200,
            artists: vec![artist("Ann"), artist("Rob")],
            album: tidal::AlbumRef {
                id: 1,
                title: "Album".into(),
                cover: Some("ab12-cd34".into()),
            },
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
    fn activity_maps_track_and_timestamps() {
        let pb = snapshot(PlayState::Playing);
        let got = activity_for(&pb, Duration::from_secs(30), NOW, &base());
        let start = NOW - 30_000;
        let want = Rich {
            details: "Song".into(),
            state: "Ann, Rob".into(),
            start_ms: Some(start),
            end_ms: Some(start + 200_000),
            image: Some("https://resources.tidal.com/images/ab12/cd34/640x640.jpg".into()),
            album: "Album".into(),
            url: "https://tidal.com/browse/track/7".into(),
        };
        assert_eq!(got, Some(want));
    }

    #[test]
    fn paused_has_no_timestamps() {
        for state in [PlayState::Paused, PlayState::Buffering] {
            let pb = snapshot(state);
            let got = activity_for(&pb, Duration::from_secs(30), NOW, &base()).expect("shown");
            assert_eq!((got.start_ms, got.end_ms), (None, None), "{state:?}");
            assert_eq!(got.details, "Song");
        }
    }

    #[test]
    fn texts_are_padded_and_cut() {
        let mut pb = snapshot(PlayState::Playing);
        if let Some(t) = &mut pb.track {
            t.title = "x".repeat(200);
            t.artists = vec![artist("A")];
            t.album.title = "é".repeat(200);
        }
        let got = activity_for(&pb, Duration::ZERO, NOW, &base()).expect("shown");
        assert_eq!(got.details, "x".repeat(128));
        assert_eq!(got.state, "A\u{200B}");
        assert_eq!(got.album, "é".repeat(128), "cut on a char boundary");
        if let Some(t) = &mut pb.track {
            t.title = "🎵".repeat(100); // two UTF-16 units each
            t.artists = vec![];
        }
        let got = activity_for(&pb, Duration::ZERO, NOW, &base()).expect("shown");
        assert_eq!(got.details, "🎵".repeat(64));
        assert_eq!(got.state, "\u{200B}\u{200B}");
    }

    /// Stopped or without a track there is nothing to show; turned off, the presence clears.
    #[test]
    fn stopped_or_disabled_clears() {
        let at = Duration::ZERO;
        let stopped = snapshot(PlayState::Stopped);
        assert_eq!(activity_for(&stopped, at, NOW, &base()), None);
        let mut none = snapshot(PlayState::Playing);
        none.state = None;
        assert_eq!(activity_for(&none, at, NOW, &base()), None);
        let empty = PlaybackSnapshot::default();
        assert_eq!(activity_for(&empty, at, NOW, &base()), None);
        let (mut presence, sent) = recording();
        let playing = snapshot(PlayState::Playing);
        presence.show(&playing, at, true);
        presence.show(&playing, at, false);
        let sent = sent.borrow();
        assert!(matches!(sent[..], [Some(_), None]), "{sent:?}");
    }

    /// A presence whose sends land in the returned list.
    fn recording() -> (Presence, Rc<RefCell<Vec<Option<Rich>>>>) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let log = sent.clone();
        let presence = Presence::new(base(), move |r| log.borrow_mut().push(r));
        (presence, sent)
    }

    /// Only a new track, play or pause, or a jump of more than 2 s is sent.
    #[test]
    fn show_sends_only_on_change() {
        let (mut p, sent) = recording();
        let s = Duration::from_secs(1);
        let playing = snapshot(PlayState::Playing);
        let count = || sent.borrow().len();
        p.show(&PlaybackSnapshot::default(), s, true);
        assert_eq!(count(), 0, "nothing shown yet: nothing to clear");
        p.show(&playing, s * 10, true);
        p.show(&playing, s * 10, true);
        p.show(&playing, s * 11, true); // a 1 s jump: as good as expected
        assert_eq!(count(), 1, "the same play");
        p.show(&playing, s * 40, true);
        assert_eq!(count(), 2, "a seek");
        let paused = snapshot(PlayState::Paused);
        p.show(&paused, s * 40, true);
        p.show(&paused, s * 90, true);
        assert_eq!(count(), 3, "paused, wherever");
        p.show(&playing, s * 90, true);
        assert_eq!(count(), 4, "resumed");
        let mut next = snapshot(PlayState::Playing);
        if let Some(t) = &mut next.track {
            t.id = 8;
        }
        p.show(&next, s * 90, true);
        assert_eq!(count(), 5, "another track");
        p.show(&next, s * 90, false);
        p.show(&next, s * 90, false);
        assert_eq!(count(), 6, "turned off: one clear");
        let sent = sent.borrow();
        let tracks: Vec<_> = sent
            .iter()
            .map(|r| r.as_ref().map(|r| &r.url[..]))
            .collect();
        let (t7, t8) = (
            Some("https://tidal.com/browse/track/7"),
            Some("https://tidal.com/browse/track/8"),
        );
        assert_eq!(tracks, [t7, t7, t7, t7, t8, None]);
        let starts: Vec<_> = sent
            .iter()
            .map(|r| r.as_ref().and_then(|r| r.start_ms))
            .collect();
        assert!(starts[2].is_none() && starts[3].is_some(), "{starts:?}");
    }
}
