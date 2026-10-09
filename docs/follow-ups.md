# Follow-ups carried out of M0–M3

Parked by task reviews and the final reviews. Pick each up in the milestone that touches its code.

## App (UI, playback, caches)

- Previous on the first track while its stream loads fetches the stream again.
- Windows shows Buffering as Paused; the overlay shows the old position briefly on a track change.
- `ClientConfig::default()` built separately in `now_playing.rs` and `backend.rs`.
- `open::that_detached` runs inside an async handler (move to `spawn_blocking`).
- `TogglePlay` cannot resume while a stream resolves; adding to a queue that has ended replays the
  finished track.
- Favorites: a failed write reverts to `!on`, not to the ids loaded meanwhile; an ids load that
  answered before a write but arrives after it overwrites the heart.
- A cache write already inside `spawn_blocking` can still land after Logout's cache clear (one entry).
- The Offline banner can outlast the outage until the next successful page load; without a disk copy
  a failed revalidation reports one error per page.
- The home cache key includes `timeOffset` (a DST change makes the offline Home a miss).
- `now_playing` caches undecoded cover bytes under the artwork key (a captive-portal page would be
  retried every 30 s); the old `<cache>/covers` directory is left on disk.
- View-all and search results are cloned into rows each frame (bounded: ≤ 500 / ≤ 90).
- Queue rows expose no AccessKit "current" state; the track context menu has no keyboard opener.
- A captive portal (a non-JSON 200) during a refresh shows a per-page error, not the Offline banner.
- A failed `favorite_ids` load at login is only logged and not retried in-session (hearts stay empty).
- Empty page-store slots accumulate, one per visited route (`evict` counts only slots with data).
- The home-feed cursor loop (`browse.rs`) has no repeated-cursor guard (capped at 5 requests, so a stuck
  cursor repeats the shelves up to 5 times).
- The login paste box keeps its undo history across logout (the pasted code is single-use and
  PKCE-bound); clear its `TextEditState` like the search box's.
- Changing Hi-Res or normalization keeps a preload made under the old settings, so the next track
  plays at the old quality and gain; drop the preload record in `set_audio` (`playback.rs`).
- A refusal after Exclusive off/on, or on another refusing device, shows no second notice; reset
  `fallback_shown` in `set_audio` when exclusive or the device changes.
- The volume slider is enabled while no output is known (briefly on exclusive loads); a fast double
  click on a settings checkbox can be lost (the UI toggles from the model's last value).
- Settings `save` renames without `sync_all` (its comment claims crash safety); `LoggedOut` and
  `SessionExpired` repeat the list of kept fields (`model.rs`).
- The badge hover joins the msgid fragment "resampled to {rate} kHz" in code (whole sentences for
  translators).
- Untested: the `Error` of a cancelled preload and its stop-when-nothing-follows branch, the stale
  drop in `RemoveFromQueue`, a normal load not counting play time while Buffering (the `playing()`
  helper sets Playing early), `start_current` clearing the output and gapless keeping it.

## Player

Closed in M3: the stale-decoder seek and the time base (Task 3), the writer panic and EINTR on read
(Task 1), the resampler's flush and exact lengths (Task 2), the backward-seek cost (Task 11: 126–149 ms
on a 25-minute 24/192 track, warm page cache, spec §10).

- After a fallback (a refused format, or a lost device that comes back), gapless same-format tracks
  stay shared until the next load or format change: `seamless()` tests the config, not the open sink
  (`engine.rs`); retry exclusive on gapless tracks if users ask.
- `PlayerEvent::Error`'s doc comment ("a preload that fails before it is heard sends none") holds only
  on the gapless path: after a tail-then-start, a preload whose first packet fails sends
  `Error { id: next }` and no `TrackEnded` (`engine.rs`; the app handles it).

- Linux: DACs that take only S24_3LE (packed 24-bit) fall back to shared; the `alsa` crate can drive
  them if users ask.
- Exclusive volume is fixed at 100%; use the hardware volume where there is one (WASAPI endpoint,
  CoreAudio, ALSA mixer).
- The exclusive device stays held while paused; release it after a long pause, as shared output
  does after 5 minutes.
- Exclusive on Linux with the Default device always fails as "device … not found" (the ALSA default
  is never `hw:`), so it falls back with a misleading reason and the hardware test's doc cannot be met
  there (`cpal.rs`); on Windows, `Named` reports every lookup error as "not found" (`wasapi.rs`).
- macOS: the physical format stays changed after hog mode ends (other apps then run at it); a matched
  physical format with another or any rate is set as is (check the nominal rate after the build, or
  it can time out or resample silently); `set_physical` stops at the first failed set instead of
  trying the next; hog mode leaks if `toggle_hog_mode` fails reading it back after setting it (heals
  on the next open).
- Windows: an init finishing after the 2 s open timeout holds the device briefly before its thread
  drops it; a device unplugged while paused is noticed only on resume, after one period; `wasapi`'s
  `trace!` in `write_to_device` runs in the render loop (set `log`'s `release_max_level_debug` in the
  app so it compiles out).
- Engine races at the track boundary (rare): a superseded seek in the preload-error arm undoes the
  switch without a flush (B's leftover frames are heard until the next flush); the drain path has a
  theoretical race when B is shorter than a device period and `Preload(C)` lands in the window; a
  seek interrupting `begin` of an after-tail preload (the drained path) still drops it and ends A
  (`engine.rs`). `Shared::heard` retries when a switch lands between its reads, but the renderer
  writes the new clock before it bumps `switches`, so a read in those few instructions can still mix
  the two tracks' clocks (an odd/even count would close it, `render.rs`).
- `SWITCH_WAIT` polls every 1 ms; 5 ms cuts the wakes five-fold (CPU budget, `engine.rs`).
- A failed positional write stores its `Interrupted` kind as the download error, and `read_exact`
  retries that forever: map it to `Other`, or retry in `write_all_at` (`stream.rs`).
- A long Hi-Res track is a large temp file: 954 MB for 25 minutes of 24/192, two during a preload;
  `/tmp` is RAM (tmpfs) on many Linux distributions. Create it with `tempfile::tempfile_in(dir)`
  (`stream.rs`), `dir` passed in by the app (its cache directory).
- Tests that miss their path: `a_lost_exclusive_device_falls_back_and_continues` renders nothing
  after `maintain`; the preload-fails-before-heard test checks events, not A's tail;
  `preload_after_the_end_is_dropped` cannot fail on its filter; the eof-branch late preload relies on
  a 200 ms sleep; the WASAPI hardware test runs 1.03–1.05 s (limit 1 s).

## tidal crate

- Return `Status { 401 }`, not `Auth`, when the retry after a successful refresh still gets a 401, so
  only a failed refresh logs the user out.
- `Error::Status { status: 0 }` sentinel for client-side errors → own variant; `read_timeout` is an
  idle timeout, not a total deadline; truncate the raw body in `Status.message`.
- A revoked refresh token makes each waiter retry the token call in turn.
- A corrupt `session` chunk count orphans chunks on clear/save.
- Persistent play-report retry queue, if reports go missing in practice.
- `reqwest`'s `rustls` feature pulls aws-lc-sys (cmake, NASM); `ring` is the lighter provider.
- `paged_all` waits for every concurrent page before returning the first error, and trusts
  `totalNumberOfItems` without a bound; a cursor cycle longer than one page runs to the 200-page cap.
- Writes go through `send`, so 429/5xx are retried (the official client never retries writes).

## CI and dependencies

- Add an MSRV 1.98 job (CI runs stable only).
- `fastframe-fonts` is pinned to the unmerged crmne/fastframe#17; return to a release tag once it
  ships.
- fastframe-audio v0.4.1 allocates in the render callback (`typed()`'s `scratch.resize`); the upstream
  issue is drafted (M3 Task 11) and waits for the human's go-ahead to file.

## M4 / M5

- Client ID override setting (README promises it as planned).
- Re-test "first WM_CLOSE ignored after minimize → restore" on a desktop without GlazeWM (M5).
- The Windows media overlay shows "Unknown app" (exe version resource / AUMID at packaging, M5).
- macOS RAM with the fonts pin (M5 performance-budget pass).
- glow presents through `Composed: Copy with GPU GDI` (~20 ms to display, 0–4 dropped frames per
  20 s scroll); revisit if users report judder (flip model needs wgpu, whose idle spin would need a
  fix first).
- Before the repo goes public: the test fixtures keep the account's personal mix ids and artist-based
  mix subtitles (listening taste); replace them if that matters.
