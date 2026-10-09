//! Playback: the segment reader, play accounting, and the backend glue between the queue,
//! TIDAL stream info and the player.

use crate::backend::{Emitter, session_expired};
use crate::i18n::tr;
use crate::model::{Action, Event, PlaybackSnapshot};
use crate::queue::Queue;
use crate::settings::{self, replay_gain};
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

/// The player source for a track's segment URLs, at `gain` (linear). Call off the async
/// runtime.
fn segment_source(id: u64, urls: Vec<String>, gain: f32) -> io::Result<Source> {
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
        gain,
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
    /// A `request` for the load or preload `seq` resolved its stream with `client`, or failed.
    Loaded {
        seq: u64,
        client: Arc<Client>,
        result: Result<Resolved, Box<dyn std::error::Error + Send + Sync>>,
    },
    /// This client's session is dead (`backend::session_expired`).
    SessionExpired(Arc<Client>),
}

/// `stream_info`, retried once after 2 s unless the failure is terminal.
async fn stream_info(c: &Client, id: u64, quality: Quality) -> tidal::Result<StreamInfo> {
    match c.stream_info(id, quality).await {
        Err(e) if classify(&e) == Failure::Retryable => {
            log::debug!("stream info for {id}: {e}; retrying");
            tokio::time::sleep(Duration::from_secs(2)).await;
            c.stream_info(id, quality).await
        }
        r => r,
    }
}

/// A track's stream, ready for the player.
pub struct Resolved {
    source: Source,
    quality: String,
    codec: String,
}

/// A player error this far from where the track was last restarted is a new hiccup.
const RECOVERED: Duration = Duration::from_secs(10);

/// The next track's stream is requested this long before the current one ends.
const PRELOAD_LEAD: Duration = Duration::from_secs(20);

/// The next track's stream, requested to follow the current one gaplessly.
struct Preload {
    /// Its player source id.
    id: u64,
    track_id: u64,
    /// Handed to the player.
    sent: bool,
    quality: String,
    codec: String,
}

/// Backend-side playback: the queue, the player, and the current play's report.
pub struct Playback {
    player: Player,
    emit: Arc<Emitter>,
    msgs: UnboundedSender<Msg>,
    queue: Queue,
    /// The current track's player `Source` id: other results and player events are stale.
    seq: u64,
    /// The last id `fresh_id` gave out.
    last_id: u64,
    /// The current track's stream info is being fetched.
    loading: bool,
    /// The latest stream request's task, a load's or a preload's; a superseded one is aborted.
    load: Option<tokio::task::JoinHandle<()>>,
    preload: Option<Preload>,
    /// A sent preload taken back, which the player may have gone on to before the cancel.
    cancelled: Option<u64>,
    /// Play, not pause, once the current track is loaded.
    want_play: bool,
    /// Where the current track was last restarted after a player error, if it was.
    restarted_at: Option<Duration>,
    resume_at: Option<Duration>,
    quality: String,
    /// The current track's codec ("flac"), as TIDAL names it.
    #[allow(dead_code, reason = "shown from Task 10 on")]
    codec: String,
    state: PlayState,
    volume: f32,
    /// Position sampled while playing: a player error resets the player's own clock.
    last_pos: Duration,
    /// Tracks failed in a row.
    failures: u32,
    play: Option<PlayAccounting>,
    /// The output in use; the quality and normalization of the next stream request.
    audio: settings::Audio,
}

impl Playback {
    /// `player` plays on `audio.output()` already.
    pub fn new(
        player: Player,
        emit: Arc<Emitter>,
        msgs: UnboundedSender<Msg>,
        audio: settings::Audio,
    ) -> Playback {
        let p = Playback {
            player,
            emit,
            msgs,
            queue: Queue::default(),
            seq: 0,
            last_id: 0,
            loading: false,
            load: None,
            preload: None,
            cancelled: None,
            want_play: false,
            restarted_at: None,
            resume_at: None,
            quality: String::new(),
            codec: String::new(),
            state: PlayState::Stopped,
            volume: 1.0,
            last_pos: Duration::ZERO,
            failures: 0,
            play: None,
            audio,
        };
        p.publish();
        p
    }

    /// A new output (device or exclusive mode) takes over at once; the quality and the
    /// normalization apply from the next stream request: the current track plays on.
    pub fn set_audio(&mut self, audio: settings::Audio) {
        if audio.output() != self.audio.output() {
            self.player.send(PlayerCmd::SetOutput(audio.output()));
        }
        self.audio = audio;
    }

    pub fn is_playing(&self) -> bool {
        self.state == PlayState::Playing
    }

    /// Samples the position, and near the end preloads the next track; called every second
    /// while playing.
    pub fn tick(&mut self, c: Option<&Arc<Client>>) {
        let position = self.position();
        if let Some(c) = c {
            self.maybe_preload(c, position);
        }
    }

    /// At `position`, `PRELOAD_LEAD` or less before the current track's end, requests the
    /// track `advance` would go to, once.
    fn maybe_preload(&mut self, c: &Arc<Client>, position: Duration) {
        let (Some(current), Some(next)) = (self.queue.current(), self.queue.peek_advance()) else {
            return;
        };
        let left = Duration::from_secs(current.duration.into()).saturating_sub(position);
        if self.state != PlayState::Playing
            || self.loading
            || self.preload.is_some()
            || left > PRELOAD_LEAD
        {
            return;
        }
        let track_id = next.id;
        let id = self.fresh_id();
        self.load = Some(self.request(c, track_id, id));
        self.preload = Some(Preload {
            id,
            track_id,
            sent: false,
            quality: String::new(),
            codec: String::new(),
        });
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
            Action::TogglePlay => {
                let busy = matches!(self.shown(), PlayState::Playing | PlayState::Buffering);
                self.handle(c, if busy { Action::Pause } else { Action::Play });
            }
            Action::PlayNext(tracks) => self.edit_queue(|q| q.insert_next(tracks)),
            Action::AddToQueue(tracks) => self.edit_queue(|q| q.append(tracks)),
            Action::MoveInQueue { from, to } => self.edit_queue(|q| q.move_item(from, to)),
            Action::SetShuffle(on) => self.edit_queue(|q| q.set_shuffle(on)),
            Action::SetRepeat(r) => self.edit_queue(|q| q.set_repeat(r)),
            Action::RemoveFromQueue(i) => {
                let active = self.loading || self.state != PlayState::Stopped;
                let was_current = self.queue.remove(i);
                self.drop_stale_preload();
                self.publish_queue();
                self.publish();
                if was_current && active {
                    // The next track takes its place, playing or paused as it was.
                    self.end_here(c);
                    self.start_current(c, None);
                }
            }
            Action::PlayQueueIndex(i) => {
                if self.queue.jump(i).is_some() {
                    self.change_track(c);
                }
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
            Msg::Loaded { seq, result, .. } => {
                log::debug!("stream {seq} resolved: {}", result.is_ok());
            }
            Msg::SessionExpired(_) => {}
        }
        if let Msg::Player(PlayerEvent::TrackStarted(id) | PlayerEvent::Error { id, .. }) = &msg {
            self.went_on(c, *id);
        }
        match msg {
            Msg::Loaded {
                seq,
                client,
                result,
            } if seq == self.seq => {
                self.loading = false;
                match result {
                    Ok(r) => {
                        (self.quality, self.codec) = (r.quality, r.codec);
                        self.state = PlayState::Buffering; // what the Load turns the player to
                        self.player.send(PlayerCmd::Load(r.source));
                        if let Some(at) = self.resume_at.take() {
                            self.player.send(PlayerCmd::Seek(at));
                        }
                        if self.want_play {
                            self.player.send(PlayerCmd::Play);
                        }
                    }
                    // Not the track's fault: it waits, with the queue, for the next login.
                    Err(e) if session_expired(&*e, &client, &self.msgs) => self.stop(),
                    Err(e) => {
                        log::warn!("stream for track {}: {e}", self.track_id());
                        self.fail_track(c);
                    }
                }
            }
            Msg::Loaded {
                seq,
                client,
                result,
            } if let Some(pre) = self.preload.as_mut().filter(|pre| pre.id == seq) => {
                match result {
                    Ok(r) => {
                        (pre.sent, pre.quality, pre.codec) = (true, r.quality, r.codec);
                        self.player.send(PlayerCmd::Preload(Some(r.source)));
                    }
                    Err(e) if session_expired(&*e, &client, &self.msgs) => {}
                    // Kept, so not retried: the track loads at the end instead.
                    Err(e) => log::debug!("preload of track {}: {e}", pre.track_id),
                }
            }
            Msg::Loaded { .. } => {}     // superseded
            Msg::SessionExpired(_) => {} // the backend's
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
                    let mut play = PlayAccounting::new(
                        t,
                        self.quality.clone(),
                        self.player.position(),
                        now_ms(),
                    );
                    // A gapless start sends no State event: the player was playing all along.
                    play.set_playing(self.state == PlayState::Playing, Instant::now());
                    self.play = Some(play);
                }
            }
            Msg::Player(PlayerEvent::TrackEnded(id)) if id == self.seq => {
                // Without a next track the player stopped itself; the queue stays on its last.
                self.natural_end(c);
            }
            Msg::Player(PlayerEvent::Error { id, message }) if id == self.seq => {
                log::warn!("playback of track {}: {message}", self.track_id());
                // One restart per hiccup. Failing again near where the last restart resumed
                // (a corrupt frame, say) would restart forever: the track is skipped instead.
                match self.restarted_at {
                    Some(at) if self.last_pos.abs_diff(at) < RECOVERED => self.fail_track(c),
                    _ => self.start_current(c, Some(self.last_pos)),
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

    /// Session expired: stop, keeping the queue for the next login. The play ends without a
    /// report: there is no session left to send it with.
    pub fn session_lost(&mut self) {
        self.end_here(None);
        self.state = PlayState::Stopped; // shown at once, not when the player confirms
        self.stop();
    }

    /// Logout and expiry: the stream load in flight stops, and is waited for, before the
    /// session's cache is cleared. `reset` or `session_lost` then supersedes it as ever.
    pub async fn abort_load(&mut self) {
        if let Some(load) = self.load.take() {
            load.abort();
            let _ = load.await;
        }
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
        let (Some(c), Some(track_id)) = (c, self.queue.current().map(|t| t.id)) else {
            return self.stop();
        };
        self.supersede();
        self.player.send(PlayerCmd::Stop); // the old track goes silent while this one resolves
        self.loading = true;
        self.restarted_at = resume_at;
        self.resume_at = resume_at;
        self.last_pos = resume_at.unwrap_or_default();
        self.load = Some(self.request(c, track_id, self.seq));
        self.publish();
    }

    /// Resolves the track's stream, at the quality and normalization set, as player source
    /// `id`; the result comes back as `Msg::Loaded`.
    fn request(&self, c: &Arc<Client>, track_id: u64, id: u64) -> tokio::task::JoinHandle<()> {
        let (c, msgs) = (c.clone(), self.msgs.clone());
        let quality = if self.audio.hi_res {
            Quality::HiResLossless
        } else {
            Quality::Lossless
        };
        let normalization = self.audio.normalization;
        tokio::spawn(async move {
            // Typed: a dead session (`tidal::Error::Auth`) is told apart from a failed track.
            let result = async {
                let info = stream_info(&c, track_id, quality).await?;
                let gain = replay_gain(normalization, &info);
                let (quality, codec) = (info.quality, info.codec);
                let source =
                    tokio::task::spawn_blocking(move || segment_source(id, info.urls, gain))
                        .await??;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(Resolved {
                    source,
                    quality,
                    codec,
                })
            }
            .await;
            let loaded = Msg::Loaded {
                seq: id,
                client: c,
                result,
            };
            let _ = msgs.send(loaded); // backend gone = app closing
        })
    }

    /// The player started, or failed on, `id`. The sent preload: the current track ended there
    /// and the preloaded one is current from now on. The cancelled one: the player went on to
    /// it before the cancel landed, so the current track ended and the queue's next one loads.
    fn went_on(&mut self, c: Option<&Arc<Client>>, id: u64) {
        if let Some(pre) = self.preload.take_if(|pre| pre.sent && pre.id == id) {
            self.end_play(c, End::Natural);
            self.queue.advance();
            (self.seq, self.quality, self.codec) = (pre.id, pre.quality, pre.codec);
            (self.last_pos, self.restarted_at, self.resume_at) = (Duration::ZERO, None, None);
            self.publish_queue();
            self.publish();
        } else if self.cancelled == Some(id) && !self.natural_end(c) {
            self.stop(); // nothing follows: what it went on to stops
        }
    }

    /// The current play ended at the track's end and the queue's next one loads; false, the
    /// queue staying put, when there is none.
    fn natural_end(&mut self, c: Option<&Arc<Client>>) -> bool {
        self.end_play(c, End::Natural);
        let next = self.queue.advance().is_some();
        if next {
            self.publish_queue();
            self.start_current(c, None);
        }
        next
    }

    /// After a queue change, a preload no longer next is dropped, and taken back if sent.
    fn drop_stale_preload(&mut self) {
        let next = self.queue.peek_advance().map(|t| t.id);
        let Some(pre) = self.preload.take_if(|pre| Some(pre.track_id) != next) else {
            return;
        };
        if pre.sent {
            self.player.send(PlayerCmd::Preload(None));
            self.cancelled = Some(pre.id);
        } else if let Some(load) = self.load.take() {
            load.abort(); // its request, still resolving
        }
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
        self.supersede();
        self.loading = false;
        self.player.send(PlayerCmd::Stop);
        self.publish();
    }

    /// A new load generation: older results and the old track's events are dropped, and the
    /// request still running stops spending network and CPU on them. The player drops its
    /// preload at the Load or Stop that comes with this.
    fn supersede(&mut self) {
        self.seq = self.fresh_id();
        (self.preload, self.cancelled) = (None, None);
        if let Some(load) = self.load.take() {
            load.abort();
        }
    }

    /// Loads and preloads share these ids, so none repeats.
    fn fresh_id(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
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

    /// The state shown: a resolving stream is buffering, not the Stop that silenced the old track.
    fn shown(&self) -> PlayState {
        if self.loading {
            PlayState::Buffering
        } else {
            self.state
        }
    }

    fn publish(&self) {
        self.emit.send(Event::Playback(PlaybackSnapshot {
            state: Some(self.shown()),
            track: self.queue.current().cloned(),
            volume: self.volume,
        }));
    }

    fn publish_queue(&self) {
        self.emit.send(Event::Queue(self.queue.snapshot()));
    }

    /// Changes the queue and shows it; an empty queue's added track shows as current.
    fn edit_queue(&mut self, edit: impl FnOnce(&mut Queue)) {
        edit(&mut self.queue);
        self.drop_stale_preload();
        self.publish_queue();
        self.publish();
    }
}

#[cfg(test)]
#[path = "playback/tests.rs"]
mod tests;
