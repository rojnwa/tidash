//! Playback: the segment reader, play accounting, and the backend glue between the queue,
//! TIDAL stream info and the player.

use crate::backend::Emitter;
use crate::i18n::tr;
use crate::model::{Action, Event, PlaybackSnapshot};
use crate::queue::Queue;
use player::{PlayState, Player, PlayerCmd, PlayerEvent, Source};
use std::io::{self, Read};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tidal::{Client, Failure, PlaybackReport, Quality, StreamInfo, classify};
use tokio::sync::mpsc::UnboundedSender;

type Body = Box<dyn Read + Send>;
type Open = Box<dyn FnMut(&str) -> io::Result<Body> + Send>;

/// Reads each URL's body in order, as one stream; the next is opened when the current ends.
pub struct SegmentReader {
    urls: std::vec::IntoIter<String>,
    open: Open,
    body: Option<Body>,
}

impl SegmentReader {
    pub fn new(
        urls: Vec<String>,
        open: impl FnMut(&str) -> io::Result<Body> + Send + 'static,
    ) -> SegmentReader {
        SegmentReader {
            urls: urls.into_iter(),
            open: Box::new(open),
            body: None,
        }
    }
}

impl Read for SegmentReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(body) = &mut self.body {
                let n = body.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
                self.body = None;
            }
            let Some(url) = self.urls.next() else {
                return Ok(0);
            };
            self.body = Some((self.open)(&url)?);
        }
    }
}

/// Segment downloads. Blocking, so it is built in `spawn_blocking` and used only on the
/// player's download thread; a static is never dropped, so never on a runtime thread either.
static SEGMENTS: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// The player source for a track's segment URLs. Call off the async runtime.
fn segment_source(id: u64, urls: Vec<String>) -> io::Result<Source> {
    let http = match SEGMENTS.get() {
        Some(http) => http,
        None => {
            let http = reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                // The blocking client applies this to the send and to each read, not the whole body.
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(io::Error::other)?;
            SEGMENTS.get_or_init(|| http)
        }
    };
    // Segment URLs are pre-signed (S1): no Authorization header.
    let open = move |url: &str| -> io::Result<Body> {
        // The URL carries its signature: keep it out of errors, which get logged.
        let resp = http
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| io::Error::other(e.without_url()))?;
        Ok(Box::new(resp))
    };
    Ok(Source {
        id,
        reader: Box::new(SegmentReader::new(urls, open)),
        len: None,
    })
}

/// How a play ended.
#[derive(Clone, Copy, Debug)]
pub enum End {
    /// The track played to its end.
    Natural,
    /// Skipped, stopped or failed with the player at this position.
    At(Duration),
}

/// One play of one track, from its first audio until it ends: what its report needs.
pub struct PlayAccounting {
    track_id: u64,
    album_id: u64,
    duration_s: u32,
    quality: String,
    session_id: String,
    started_at_ms: u64,
    start_position: Duration,
    /// Time spent in `PlayState::Playing`, not counting the running stretch.
    played: Duration,
    playing_since: Option<Instant>,
}

impl PlayAccounting {
    pub fn new(
        track: &tidal::Track,
        quality: String,
        start_position: Duration,
        now_ms: u64,
    ) -> PlayAccounting {
        PlayAccounting {
            track_id: track.id,
            album_id: track.album.id,
            duration_s: track.duration,
            quality,
            session_id: tidal::new_session_id(),
            started_at_ms: now_ms,
            start_position,
            played: Duration::ZERO,
            playing_since: None,
        }
    }

    /// Call on every state change: only wall-clock time while playing counts.
    pub fn set_playing(&mut self, playing: bool, now: Instant) {
        match (playing, self.playing_since) {
            (true, None) => self.playing_since = Some(now),
            (false, Some(since)) => {
                self.played += now.saturating_duration_since(since);
                self.playing_since = None;
            }
            _ => {}
        }
    }

    fn played(&self, now: Instant) -> Duration {
        self.played
            + self
                .playing_since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }
}

const MIN_REPORTED: Duration = Duration::from_secs(30);

/// The report for a play ending now, or `None` under 30 s of playing (S1 `## Play events`).
pub fn report_for(
    play: &PlayAccounting,
    end: End,
    now: Instant,
    now_ms: u64,
) -> Option<PlaybackReport> {
    if play.played(now) < MIN_REPORTED {
        return None;
    }
    let duration = f64::from(play.duration_s);
    Some(PlaybackReport {
        track_id: play.track_id,
        quality: play.quality.clone(),
        streaming_session_id: play.session_id.clone(),
        started_at_ms: play.started_at_ms,
        ended_at_ms: now_ms,
        start_position_s: play.start_position.as_secs_f64(),
        end_position_s: match end {
            End::Natural => duration,
            End::At(pos) => pos.as_secs_f64().min(duration),
        },
        // R26: M1 plays come from albums or search; both are attributed to the track's album.
        source_type: "ALBUM".into(),
        source_id: play.album_id.to_string(),
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// What reaches the backend besides actions.
pub enum Msg {
    Player(PlayerEvent),
    /// A `start_current` resolved its stream (source and quality) or failed.
    Loaded {
        seq: u64,
        result: Result<(Source, String), String>,
    },
}

/// `stream_info`, retried once after 2 s unless the failure is terminal.
async fn stream_info(c: &Client, id: u64) -> tidal::Result<StreamInfo> {
    match c.stream_info(id, Quality::Lossless).await {
        Err(e) if classify(&e) == Failure::Retryable => {
            log::debug!("stream info for {id}: {e}; retrying");
            tokio::time::sleep(Duration::from_secs(2)).await;
            c.stream_info(id, Quality::Lossless).await
        }
        r => r,
    }
}

/// Backend-side playback: the queue, the player, and the current play's report.
pub struct Playback {
    player: Player,
    emit: Arc<Emitter>,
    msgs: UnboundedSender<Msg>,
    queue: Queue,
    /// The current load's generation, used as the player `Source` id: older results and
    /// player events are stale.
    seq: u64,
    /// The current track's stream info is being fetched.
    loading: bool,
    /// Play, not pause, once the current track is loaded.
    want_play: bool,
    /// The current track was restarted after a player error already.
    retried: bool,
    resume_at: Option<Duration>,
    quality: String,
    state: PlayState,
    volume: f32,
    /// Position sampled while playing: a player error resets the player's own clock.
    last_pos: Duration,
    /// Tracks failed in a row.
    failures: u32,
    play: Option<PlayAccounting>,
}

impl Playback {
    pub fn new(player: Player, emit: Arc<Emitter>, msgs: UnboundedSender<Msg>) -> Playback {
        let p = Playback {
            player,
            emit,
            msgs,
            queue: Queue::default(),
            seq: 0,
            loading: false,
            want_play: false,
            retried: false,
            resume_at: None,
            quality: String::new(),
            state: PlayState::Stopped,
            volume: 1.0,
            last_pos: Duration::ZERO,
            failures: 0,
            play: None,
        };
        p.publish();
        p
    }

    pub fn is_playing(&self) -> bool {
        self.state == PlayState::Playing
    }

    /// Samples the position; called every second while playing.
    pub fn tick(&mut self) {
        self.position();
    }

    pub fn handle(&mut self, c: Option<&Arc<Client>>, action: Action) {
        match action {
            Action::PlayTracks { tracks, start } => {
                log::debug!("play {} tracks from {start}", tracks.len());
                self.end_here(c);
                self.failures = 0;
                if self.queue.replace(tracks, start).is_some() {
                    self.want_play = true;
                    self.start_current(c, None);
                } else {
                    self.stop();
                }
                self.publish_queue();
            }
            Action::Play => {
                self.want_play = true;
                if self.state == PlayState::Stopped && !self.loading {
                    self.start_current(c, None); // after the queue's end or a failure
                } else {
                    self.player.send(PlayerCmd::Play);
                }
            }
            Action::Pause => {
                self.want_play = false;
                self.player.send(PlayerCmd::Pause);
            }
            Action::Next => {
                if self.queue.next().is_some() {
                    self.change_track(c);
                }
            }
            Action::Previous => {
                let active = self.state != PlayState::Stopped;
                if active && self.player.position() > Duration::from_secs(3) {
                    self.seek(Duration::ZERO);
                } else if self.queue.previous().is_some() || !active {
                    self.change_track(c); // stopped with none before: (re)load this one
                } else {
                    self.seek(Duration::ZERO); // the first track
                }
            }
            Action::Seek(to) => self.seek(to),
            Action::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.player.send(PlayerCmd::SetVolume(self.volume));
                self.publish();
            }
            _ => {}
        }
    }

    pub fn on_msg(&mut self, c: Option<&Arc<Client>>, msg: Msg) {
        match &msg {
            Msg::Player(ev) => log::debug!(
                "player {ev:?} at {:?} (current load {})",
                self.player.position(),
                self.seq
            ),
            Msg::Loaded { seq, result } => log::debug!("stream {seq} resolved: {}", result.is_ok()),
        }
        match msg {
            Msg::Loaded { seq, result } if seq == self.seq => {
                self.loading = false;
                match result {
                    Ok((source, quality)) => {
                        self.quality = quality;
                        self.state = PlayState::Buffering; // what the Load turns the player to
                        self.player.send(PlayerCmd::Load(source));
                        if let Some(at) = self.resume_at.take() {
                            self.player.send(PlayerCmd::Seek(at));
                        }
                        if self.want_play {
                            self.player.send(PlayerCmd::Play);
                        }
                    }
                    Err(e) => {
                        log::warn!("stream for track {}: {e}", self.track_id());
                        self.fail_track(c);
                    }
                }
            }
            Msg::Loaded { .. } => {} // superseded
            Msg::Player(PlayerEvent::State(s)) => {
                self.state = s;
                if let Some(p) = &mut self.play {
                    p.set_playing(s == PlayState::Playing, Instant::now());
                }
                self.publish();
            }
            Msg::Player(PlayerEvent::TrackStarted(id)) if id == self.seq => {
                self.failures = 0;
                // A restart after an error continues the same play.
                if self.play.is_none()
                    && let Some(t) = self.queue.current()
                {
                    self.play = Some(PlayAccounting::new(
                        t,
                        self.quality.clone(),
                        self.player.position(),
                        now_ms(),
                    ));
                }
            }
            Msg::Player(PlayerEvent::TrackEnded(id)) if id == self.seq => {
                self.end_play(c, End::Natural);
                if self.queue.next().is_some() {
                    self.publish_queue();
                    self.start_current(c, None);
                } // else the player stopped itself; the queue stays on its last track
            }
            Msg::Player(PlayerEvent::Error { id, message }) if id == self.seq => {
                log::warn!("playback of track {}: {message}", self.track_id());
                if self.retried {
                    self.fail_track(c);
                } else {
                    self.start_current(c, Some(self.last_pos));
                }
            }
            Msg::Player(PlayerEvent::OutputLost) => self.emit.send(Event::Error(
                tr("Audio output lost; playback resumes when it is back").into_owned(),
            )),
            Msg::Player(_) => {} // an earlier load's
        }
    }

    /// Logout: report and stop, empty queue.
    pub fn reset(&mut self, c: Option<&Arc<Client>>) {
        self.end_here(c);
        self.queue = Queue::default();
        self.failures = 0;
        self.stop();
        self.publish_queue();
    }

    /// App shutdown: report the current play, waiting briefly for the send.
    pub async fn shutdown(&mut self, c: Option<&Arc<Client>>) {
        if let Some(sent) = self.end_here(c) {
            let _ = tokio::time::timeout(Duration::from_secs(2), sent).await;
        }
    }

    /// The player's position while it has a track; else the last one known.
    fn position(&mut self) -> Duration {
        if self.state != PlayState::Stopped {
            self.last_pos = self.player.position();
        }
        self.last_pos
    }

    /// While a stream resolves, the seek waits for its load instead of hitting the old track.
    fn seek(&mut self, to: Duration) {
        self.last_pos = to;
        self.emit.send(Event::Seeked(to));
        if self.loading {
            self.resume_at = Some(to);
        } else {
            self.player.send(PlayerCmd::Seek(to));
        }
    }

    fn track_id(&self) -> u64 {
        self.queue.current().map_or(0, |t| t.id)
    }

    /// A user-chosen track change: the current play ends, the queue's current one starts.
    fn change_track(&mut self, c: Option<&Arc<Client>>) {
        self.end_here(c);
        self.want_play = true;
        self.publish_queue();
        self.start_current(c, None);
    }

    /// Fetches the current track's stream and loads it; `resume_at` restarts it after an error.
    fn start_current(&mut self, c: Option<&Arc<Client>>, resume_at: Option<Duration>) {
        let (Some(c), Some(track)) = (c, self.queue.current()) else {
            return self.stop();
        };
        self.seq += 1;
        let (seq, id, c, msgs) = (self.seq, track.id, c.clone(), self.msgs.clone());
        self.player.send(PlayerCmd::Stop); // the old track goes silent while this one resolves
        self.loading = true;
        self.retried = resume_at.is_some();
        self.resume_at = resume_at;
        self.last_pos = resume_at.unwrap_or_default();
        tokio::spawn(async move {
            let result = match stream_info(&c, id).await {
                Ok(info) => {
                    let quality = info.quality;
                    match tokio::task::spawn_blocking(move || segment_source(seq, info.urls)).await
                    {
                        Ok(Ok(source)) => Ok((source, quality)),
                        Ok(Err(e)) => Err(e.to_string()),
                        Err(e) => Err(e.to_string()),
                    }
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = msgs.send(Msg::Loaded { seq, result }); // backend gone = app closing
        });
        self.publish();
    }

    /// The current track cannot play: tell the user and go on, or stop after three in a row.
    fn fail_track(&mut self, c: Option<&Arc<Client>>) {
        let title = self
            .queue
            .current()
            .map(|t| t.title.clone())
            .unwrap_or_default();
        self.emit.send(Event::Error(
            tr("{title} is unavailable").replace("{title}", &title),
        ));
        self.end_play(c, End::At(self.last_pos));
        self.failures += 1;
        if self.failures >= 3 {
            self.failures = 0;
            self.stop();
            self.emit.send(Event::Error(
                tr("Playback stopped after repeated errors").into_owned(),
            ));
        } else if self.queue.next().is_some() {
            self.publish_queue();
            self.start_current(c, None);
        } else {
            self.stop();
        }
    }

    fn stop(&mut self) {
        self.seq += 1; // drops in-flight loads and the old track's events
        self.loading = false;
        self.player.send(PlayerCmd::Stop);
        self.publish();
    }

    /// Ends the current play where the player is now.
    fn end_here(&mut self, c: Option<&Arc<Client>>) -> Option<tokio::task::JoinHandle<()>> {
        let pos = self.position();
        self.end_play(c, End::At(pos))
    }

    /// Ends the current play; sends its report (fire-and-forget) if it qualifies.
    fn end_play(
        &mut self,
        c: Option<&Arc<Client>>,
        end: End,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let report = report_for(&self.play.take()?, end, Instant::now(), now_ms())?;
        let c = c?.clone();
        Some(tokio::spawn(async move {
            match c.report_playback(&report).await {
                Ok(()) => log::debug!("play report sent for track {}", report.track_id),
                Err(e) => log::debug!("play report for track {}: {e}", report.track_id),
            }
        }))
    }

    fn publish(&self) {
        self.emit.send(Event::Playback(PlaybackSnapshot {
            // A resolving stream shows as buffering, not as the Stop that silenced the old track.
            state: Some(if self.loading {
                PlayState::Buffering
            } else {
                self.state
            }),
            track: self.queue.current().cloned(),
            volume: self.volume,
        }));
    }

    fn publish_queue(&self) {
        self.emit.send(Event::Queue(self.queue.snapshot()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    /// A reader over `segments` (url -> bytes) whose `open` logs each URL and fails on `bad`.
    fn reader(
        segments: &[(&str, &[u8])],
        bad: Option<&'static str>,
    ) -> (SegmentReader, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let data: Vec<(String, Vec<u8>)> = segments
            .iter()
            .map(|(u, b)| (u.to_string(), b.to_vec()))
            .collect();
        let urls = data.iter().map(|(u, _)| u.clone()).collect();
        let opened = log.clone();
        let r = SegmentReader::new(urls, move |url| {
            opened.lock().expect("log").push(url.to_string());
            if Some(url) == bad {
                return Err(io::Error::other("403"));
            }
            let bytes = data
                .iter()
                .find(|(u, _)| u == url)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            Ok(Box::new(Cursor::new(bytes)) as Body)
        });
        (r, log)
    }

    #[test]
    fn segment_reader_concatenates_in_order() {
        let (mut r, log) = reader(&[("init", b"ab"), ("1", b"cde"), ("2", b"f")], None);
        assert!(
            log.lock().expect("log").is_empty(),
            "nothing opened before the first read"
        );
        let mut first = [0u8; 1];
        assert_eq!(r.read(&mut first).expect("read"), 1);
        assert_eq!(*log.lock().expect("log"), ["init"], "opened lazily");
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).expect("read_to_end");
        assert_eq!([&first[..], &rest].concat(), b"abcdef");
        assert_eq!(*log.lock().expect("log"), ["init", "1", "2"]);
    }

    #[test]
    fn segment_reader_surfaces_open_error() {
        let (mut r, _) = reader(&[("init", b"ab"), ("1", b"cd"), ("2", b"ef")], Some("1"));
        let mut got = Vec::new();
        assert!(r.read_to_end(&mut got).is_err());
        assert_eq!(
            got, b"ab",
            "the first segment's bytes come before the error"
        );
    }

    fn track() -> tidal::Track {
        tidal::Track {
            id: 7,
            title: "t".into(),
            duration: 200,
            track_number: 1,
            volume_number: 1,
            artists: vec![],
            album: tidal::AlbumRef {
                id: 42,
                title: "a".into(),
                cover: None,
            },
            stream_ready: true,
        }
    }

    const S: Duration = Duration::from_secs(1);

    /// A play that started at `t0` and was playing for `secs` seconds by `t0 + secs`.
    fn played(t0: Instant, secs: u32) -> PlayAccounting {
        let mut p = PlayAccounting::new(&track(), "LOSSLESS".into(), Duration::ZERO, 1_000);
        p.set_playing(true, t0);
        p.set_playing(false, t0 + S * secs);
        p
    }

    #[test]
    fn short_play_is_not_reported() {
        let t0 = Instant::now();
        assert!(report_for(&played(t0, 29), End::Natural, t0 + S * 29, 30_000).is_none());
    }

    #[test]
    fn natural_end_reports_the_duration() {
        let t0 = Instant::now();
        let r = report_for(&played(t0, 199), End::Natural, t0 + S * 199, 200_000).expect("report");
        assert_eq!(r.end_position_s, 200.0);
        assert_eq!(
            (
                r.track_id,
                r.start_position_s,
                r.started_at_ms,
                r.ended_at_ms
            ),
            (7, 0.0, 1_000, 200_000)
        );
        assert_eq!(
            (
                r.source_type.as_str(),
                r.source_id.as_str(),
                r.quality.as_str()
            ),
            ("ALBUM", "42", "LOSSLESS")
        );
        assert_eq!(r.streaming_session_id.len(), 36);
    }

    #[test]
    fn skip_reports_the_position_clamped_to_the_duration() {
        let t0 = Instant::now();
        let p = played(t0, 95);
        let at = |secs| {
            report_for(&p, End::At(S * secs), t0 + S * 95, 2)
                .expect("report")
                .end_position_s
        };
        assert_eq!(at(95), 95.0);
        assert_eq!(at(250), 200.0);
    }

    /// Device-less player, no client: every load stops at once, so no runtime is needed.
    fn playback() -> (Playback, std::sync::mpsc::Receiver<Event>) {
        let (player, _events, _renderer) = Player::spawn_manual();
        let (tx, rx) = std::sync::mpsc::channel();
        let emit = Arc::new(Emitter {
            tx,
            waker: Box::new(|| {}),
        });
        let (msgs, _) = tokio::sync::mpsc::unbounded_channel();
        (Playback::new(player, emit, msgs), rx)
    }

    fn tracks(n: u64) -> Vec<tidal::Track> {
        (0..n)
            .map(|id| tidal::Track {
                id,
                title: format!("t{id}"),
                ..track()
            })
            .collect()
    }

    fn errors(rx: &std::sync::mpsc::Receiver<Event>) -> Vec<String> {
        rx.try_iter()
            .filter_map(|e| {
                if let Event::Error(e) = e {
                    Some(e)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn three_failures_in_a_row_stop_and_a_new_queue_starts_over() {
        let (mut p, rx) = playback();
        p.handle(
            None,
            Action::PlayTracks {
                tracks: tracks(9),
                start: 0,
            },
        );
        p.fail_track(None);
        p.fail_track(None);
        p.handle(
            None,
            Action::PlayTracks {
                tracks: tracks(9),
                start: 0,
            },
        );
        assert_eq!(p.failures, 0, "a new queue starts over");
        for _ in 0..3 {
            p.fail_track(None);
        }
        let errs = errors(&rx);
        assert_eq!(errs.len(), 6);
        assert_eq!(errs[0], "t0 is unavailable");
        assert_eq!(errs[5], "Playback stopped after repeated errors");
        p.fail_track(None);
        p.reset(None);
        assert_eq!(p.failures, 0, "logout starts over");
    }

    #[test]
    fn a_lost_output_is_shown() {
        let (mut p, rx) = playback();
        p.on_msg(None, Msg::Player(PlayerEvent::OutputLost));
        assert_eq!(
            errors(&rx),
            ["Audio output lost; playback resumes when it is back"]
        );
    }

    #[test]
    fn previous_on_the_first_track_seeks_instead_of_reloading() {
        let (mut p, _rx) = playback();
        p.queue.replace(tracks(3), 0);
        p.state = PlayState::Playing;
        let seq = p.seq;
        p.handle(None, Action::Previous);
        assert_eq!(p.seq, seq, "no reload");
        p.queue.next();
        p.handle(None, Action::Previous);
        assert_eq!(p.queue.current().map(|t| t.id), Some(0));
        assert_ne!(p.seq, seq, "the previous track loads");
    }

    #[test]
    fn seek_while_loading_waits_for_the_load() {
        let (mut p, _rx) = playback();
        p.loading = true;
        p.handle(None, Action::Seek(S * 5));
        assert_eq!(p.resume_at, Some(S * 5));
        p.loading = false;
        p.handle(None, Action::Seek(S * 7));
        assert_eq!(p.resume_at, Some(S * 5), "loaded: goes to the player");
    }

    #[test]
    fn pause_time_is_excluded() {
        let t0 = Instant::now();
        let mut p = PlayAccounting::new(&track(), "LOSSLESS".into(), Duration::ZERO, 0);
        p.set_playing(true, t0);
        p.set_playing(false, t0 + S * 20); // paused for 100 s
        p.set_playing(true, t0 + S * 120);
        assert!(
            report_for(&p, End::At(S * 25), t0 + S * 125, 0).is_none(),
            "25 s played"
        );
        assert!(
            report_for(&p, End::At(S * 30), t0 + S * 130, 0).is_some(),
            "30 s played, still running"
        );
    }
}
