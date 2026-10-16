# Follow-ups carried out of M0–M4 and the design system

Parked by task reviews and the final reviews. Pick each up in the milestone that touches its code.

## App (UI, playback, caches)

- Previous on the first track while its stream loads fetches the stream again.
- Windows shows Buffering as Paused; the overlay shows the old position briefly on a track change.
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
- `LoggedOut` and `SessionExpired` repeat the list of kept fields (`model.rs`).
- Untested: the `Error` of a cancelled preload and its stop-when-nothing-follows branch, the stale
  drop in `RemoveFromQueue`, a normal load not counting play time while Buffering (the `playing()`
  helper sets Playing early), `start_current` clearing the output and gapless keeping it.

Parked in M4:

- Not built: an album credits page (v1 `albums/{id}/items/credits`, S5; only track credits show, in
  the side panel); collaborative playlists (only the user's own are edited).
- The lyrics auto-scroll follows every line change; pause it after a manual scroll (`ui/lyrics.rs`).
- Lyrics: a late Failed for the same track can replace Ready lyrics (track 1 → 2 → 1, the first
  request timing out last; Failed should never overwrite Ready, `model.rs`); the lyrics and credits
  scroll offset carries over between tracks (`id_salt` not keyed by the track); the highlight lags a
  line change by up to 250 ms; a blank synced line shows "…" outside `tr` (`ui/lyrics.rs`).
- "Added n tracks" and "Already in this playlist" go out through the error list, warn-coloured (R17);
  a styled info notice (`library.rs`).
- The "Added n tracks to {playlist}" notice reads the title through the cached GET, so a just-renamed
  playlist can show its old title once (`library.rs`).
- Settings: a stale echo of edit A after B can make a third edit C build on A (B lost), and two quick
  edits can flicker once (the theme, say) until the second echo; a pending counter like
  `pending_favorites` would close both (`model.rs`).
- A Client ID or Redirect URL typed but not applied is discarded on leaving Settings
  (`ui/settings.rs`).
- A client change while logged out leaves the old login URL on the login page (its redirect now asks
  to start again) (`model.rs`).
- A refused refresh shows both "session expired" and the refused-client message; a redirect-only
  change says "the client ID changed"; `Msg::SessionExpired(Arc<Client>, bool)` wants named fields
  (`backend.rs`, `model.rs`).
- Shortcut list: the key names "Space" and "Alt" skip `tr`; Ctrl+, does nothing on the login page
  though the list names it (`ui/settings.rs`, `ui/mod.rs`); Ctrl+Shift+Q also quits (`consume_key`
  ignores an extra Shift, as for the other shortcuts).
- Library tree: a failed root or folder load at login (5xx) is only logged and not retried (the tree
  stays empty until the next edit or login); deleted folders stay in the backend's `folders` set and
  later edits GET them (drop them when `DeleteFolder` succeeds); concurrent edits' reloads can land out
  of order (`backend.rs`, `library.rs`).
- "Move to folder": `destinations` dedupes only adjacent entries, so a folder can show twice; its
  AccessKit label carries egui's "⏵" ("Add to playlist" overrides it) (`ui/library.rs`).
- A click outside a dialog closes it and discards what was typed (egui `Modal`'s default,
  `ui/library.rs`); check in the acceptance session.
- Row menus: a reload landing on the right-click pass races for one frame; an empty popup frame shows
  for one pass on close; `tracks::show` does not enforce "edit only without discs"
  (`edit.filter(|_| !multi)`); `tracks` imports `dragging` from `queue_panel`, which imports `ROW_H`
  from `tracks` (`rows.rs` is the natural home) (`ui/tracks.rs`, `ui/queue_panel.rs`).
- Scrobbling: a 401 in flight across a disconnect and reconnect can clear the new user or delete the
  new secret; `forget` compares the cache before the store delete, so a racing connect's secret could
  go (the UI cannot reach that) (`scrobble.rs`).
- Scrobbling: a connected service with no stored token reads the store again on every send (a prompt
  on a locked Linux Secret Service); a wiped keyring with the user still set shows "Connected as …"
  while listens wait; settings lost with a saved queue send the backlog to whichever account connects
  next (`scrobble.rs`).
- Scrobbling: a ListenBrainz 4xx drops a whole import and keeps dropping (a systematic 403/404 wipes
  the backlog), and a Last.fm batch refused outright (code 6, say) goes whole. Never queue a listen
  with an empty artist or title (`Listen::of` falls back to ""); optionally re-send one by one
  (`scrobble.rs`, `scrobble/listenbrainz.rs`, `scrobble/lastfm.rs`).
- A skip within ~1 s of crossing the threshold loses the scrobble (checked on the 1 s tick only);
  `end_play` could check `scrobble_due` (`playback.rs`).
- Last.fm's `verdict` checks HTTP 5xx before the codes (a 5xx with 9/10/26 is Retry, not Reconnect):
  reorder, or fix the comment (`scrobble/lastfm.rs`).
- After a "reconnect" verdict the service leaves the connected set, so plays until the user connects
  again are not queued (offline, they are): decide whether to queue them (`scrobble.rs`).
- Discord: timestamps can carry over to another track that starts within 2 s (add
  `&& new.url == old.url`; only a < 2 s gapless track reaches it); the thread wakes every 30 s with
  nothing to do (block on `recv` while nothing is dirty); no reconnect right after a write error (it
  waits for the 30 s tick) (`discord.rs`).
- A later launch: fastframe-instance's own warnings go nowhere (no logger yet, `instance.rs`); on
  Windows it may only flash the taskbar instead of raising the window, where
  `AllowSetForegroundWindow(ASFW_ANY)` in the later launch is the remedy (check in the acceptance
  session).
- The current track's own in-flight load keeps the old quality when the settings change (documented,
  `playback.rs`).
- `ui/testing.rs`' `every_state` shadows `shown` (closure, module fn, local); the
  `too_many_arguments` reason in `ui/header.rs` ("each part named at the call") reads oddly.
- Untested in M4: Cancel/OK in the name and playlist dialogs, and an edit's Auth path (no error, no
  reload) (`ui/library.rs`, `library.rs`); the row-menu reload test changes track and ETag at once (a
  one-field guard would pass); the album header's "Add to playlist" sending every track id; a dialog
  closing on LoggedOut/SessionExpired (`library::close`); a saved-queue flush at start without a user
  (`a_token_without_a_user_sends_nothing` flushes an empty queue), the 500 cap, and repeat-one through
  `Playback`; the no-keys filter at the send level (`Scrobbler::new` with `lastfm_keys` None and
  Last.fm connected); Discord's `run` loop (generic over `DiscordIpc`, it could take a fake); the
  hide/quit state machine (quit beats hide, Toggle on a minimized window restores, Toggle headless
  shows) and the `quit_shortcuts` clear in `setup_look` (`main.rs`); the backend's
  `Action::OpenTrack` arm; `set_audio`'s abort of an unsent preload and a normalization-only change
  (`playback/tests.rs`).

## Design system

Parked in the design-system milestone (2026-10-11; its rulings are summarized in the plan's Outcome).

- Not built: play and favourite buttons on cards (playing an album or playlist by id needs a backend
  action that does not exist yet); the track row's "…" button (the context menu covers it); Shuffle on
  page headers, beside Play, as TIDAL 2.43 has it.
- Ship Inter's licence (`Inter-LICENSE.txt`, in fastframe-fonts' `fonts/` folder) and the icons'
  Lucide (ISC) and Spotifast (MIT) notices (`crates/app/assets/icons/LICENSE.txt`) in every package
  (M5).
- With Exclusive on, a window at the 960×540 minimum leaves the player's title column ~31 px: the
  right column is sized from its measured controls and the widest short pill ("24/176.4 FLAC ·
  Exclusive") so Play never moves (Rulings 25, 28; this replaced M4's estimated 420 pt); a longer
  "Exclusive" translation could make the left column's floor bind and cut the pill
  (`ui/player_bar.rs`). Look at it in the acceptance session.
- Between tracks the player bar dips toward `panel` until the new cover's 16 px image loads: keep the
  last tint (one `Option` in `UiState`). The bar's tint also fades on a theme change while everything
  else switches at once (`ui/mod.rs`).
- `ui/mod.rs` is ~980 lines (soft limit ~800): move the tint (`tint`, `bar_fill`, their test) to
  `ui/tint.rs` and the behind-the-bar helpers (`bar_id`, `behind_bar`) to `top_bar.rs`.
- Theme changes: a change and a change back within 0.25 s leave the `Transition` capturing, so the next
  change skips the reveal (`ui/mod.rs`); "Loading themes…" blinks and shifts Settings on each visit
  (reserve its line, `ui/settings.rs`); `widgets.noninteractive`'s `fg_stroke` and `bg_fill` keep
  egui's greys (`theme.rs`).
- Top bar: Ctrl+F still opens the Search page now that the field is on every page (product question);
  the focused search pill draws its hint like typed text, and a click on its icon does not focus it
  (`ui/widgets.rs`); a long error pushes its Dismiss button off-screen.
- Sidebar: collapsed, the icons pop in at the end of the slide, and the scroll bar leaves 36 px for the
  40 px nav rows (~4 px spill, icons off-centre); a long "All playlists" translation pushes the New
  buttons out (`ui/sidebar.rs`, `ui/library.rs`).
- Shelves: a quick second › click lands short of a page (it retargets from the animated offset); the
  arrows are never dimmed at the ends or when every card fits; "View all" lost its pointing-hand
  cursor (`ui/shelf.rs`).
- Headers: the exact-size cover request is built twice (`ui/header.rs`, `ui/mod.rs`); a debug build
  logs egui's PERF WARNING when a header's height changes three frames running; the cover to action
  row gap is 40 against TIDAL's ~48 (check in the acceptance session).
- `dominant()`'s heaviest bucket and `TextureCache::get`'s nearest size break ties in `HashMap` order,
  so a tie can differ between runs: break them by key, the larger size for `get` (`theme.rs`,
  `images.rs`).
- A hovered square card's shade has sharp corners over the cover's 2 px radius (`ui/card.rs`).
- In rows under ~121 px wide the duration column can overlap the cover and index (unreachable at the
  960 px minimum): keep the time column from starting left of the title (`ui/tracks.rs`).
- `icon_button` and `circle_button` repeat their scaffolding (a private disc helper); `badge` takes a
  palette it does not use (`ui/widgets.rs`).
- Test nits: the widgets tests' `run` repeats `testing::pass_with`'s input setup (`testing::pass_raw`
  takes a whole `RawInput` now); the logged-in error strip (Dismiss below `TOP_BAR`) is untested; the
  rows' long-title test compares against the next shape, not the next column's start; nothing tests
  that a stand-in drawn this pass survives an eviction; the no-slab test's 700 px literal assumes the
  800 px test window; the Settings fit test omits the token field and allows flush edges.
- Settings' help texts under the rows are not AccessKit descriptions; the token field says "User token"
  twice (name and placeholder) (`ui/settings.rs`).
- "Album", "Artist" and "Playlist" share one msgid across senses (type line, column header, setting)
  without `msgctxt`, and their location comments miss `src/ui/tracks.rs` (`tidash.pot`).
- The player bar's slider knob copies egui's private slider geometry (height / 2.5, `shrink(radius)`),
  so an egui upgrade can move egui's handle out from under it; add a same-centre check
  (`ui/player_bar.rs`).
- Screenshot switch: a minimized or occluded window can hang it (the 20 s cap lives in `draw`; put the
  deadline in `logic`), it does not wait for a running theme `Transition`, and a run while Tidash
  runs exits silently (`main.rs`). The pageless-route list is in three places (`main.rs`, `model.rs`,
  `pages.rs`; a `Route::has_page`).
- Untested: the top bar painting above the backdrop and hero, and the AccessKit order; the off-screen
  16 px tint request, the coverless gradient and the 16 px texture surviving an over-budget insert;
  folder open state across the sidebar switch; ‹ on shelves and the grid geometry; the widget kit at
  idle (`repaint_delay == MAX`); `dominant()` on partly transparent images. `testing::bounds` matches
  only `label()` (`Role::Label` nodes keep their text in `value`); the settings test pairs "Nord.json"
  with `presets::themes().nth(2)` instead of finding it by filename.

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
  drops it; a device unplugged while paused is noticed only on resume, after one period.
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
- `Error::Status { status: 0 }` sentinel for client-side errors (a playlist GET without an `ETag`
  header is one, "no playlist ETag", `edit.rs`) → own variant; `read_timeout` is an idle timeout, not
  a total deadline; truncate the raw body in `Status.message`.
- A revoked refresh token makes each waiter retry the token call in turn.
- A corrupt `session` chunk count orphans chunks on clear/save.
- Persistent play-report retry queue, if reports go missing in practice.
- `reqwest`'s `rustls` feature pulls aws-lc-sys (cmake, NASM); `ring` is the lighter provider.
- `paged_all` waits for every concurrent page before returning the first error, and trusts
  `totalNumberOfItems` without a bound; a cursor cycle longer than one page runs to the 200-page cap.
- Writes go through `send`, so 429/5xx are retried (the official client never retries writes): a
  `create-folder` that TIDAL applied but answered 5xx can make two folders (v2 has no idempotency
  key), and an add applied but answered 5xx reports 0 added after its SKIP retry (`edit.rs`).
- R10: `playlist_items` positions count the items TIDAL lists; if TIDAL ever left an unavailable item
  out of a page, later positions would shift and a remove or move would hit a neighbouring row
  (`browse.rs`, marked `ponytail:`; official client 2026.09.30 makes the same assumption).
- `folder()` would send both `cursor` and `offset` if a cursor ever appeared (R6's guess, `edit.rs`).
- `TokenStore`'s secret methods default to no-ops (`delete_secret` answers Ok), so a half-written
  store would hide a failed Disconnect: make them required (`store.rs`).
- Untested: that only a 404 maps to `Ok(None)` in `lyrics` (500/403 are errors); `parse_lrc`'s
  rejected stamps (`[ab:cd.ef]`, `[01:02.5]`, `[ar:x]`, a missing `]`, no space after the cue), checked
  by inspection only (`lyrics.rs`); a non-matching `client_refused()` (`invalid_grant`, another
  `Auth`, a `Status`) (`error.rs`).
- `catalog.rs:21`'s doc comment ("…is neither") reads oddly.
- S5 fixtures: `folder_items.json`'s top-level `lastModifiedAt` is earlier than the subfolder's
  `addedAt` (cosmetic); `openapi_playlist_created.json` keeps 4 `externalLinks` (the rule is ≤ 3); the
  probe titles stay instead of Playlist A/B; the probe uuid `…0001` is S4's Playlist A in older
  fixtures (R8; renumber to `…0009`?).
- Spike docs: S5 row 11 says "add" with the SKIP ETag where the Task 1 report says a move;
  `s4-library-endpoints.md` line 3 shows the recorder's old positional header syntax (now `--header`).

## CI and dependencies

- fastframe-audio v0.6.0 allocates in the render callback (`typed()`'s `scratch.resize`); the upstream
  issue is drafted (M3 Task 11) and waits for the human's go-ahead to file.
- Duplicates `cargo update` cannot settle: windows-sys 0.52/0.59 beside 0.60/0.61 (the winit fork,
  glutin, glutin_egl_sys and ring ask for 0.52, rustix 0.38 for 0.59), and `dpi` twice (the winit
  fork's 0.1.1, muda's 0.1.2) (`Cargo.lock`).
- egui, eframe and egui_extras 0.37 are out; Tidash stays on 0.36 until fastframe's egui fork moves.

## M5 and later

- Links: register the `tidal://` scheme and handle macOS Apple Events at packaging; an autostart
  argument then pings instead of showing the window (R21, `instance.rs`).
- Linux Flatpak: `discord-rich-presence`'s Unix transport commits to the first socket path that
  exists, so a stale Flatpak socket refuses every connect (SONE replaced the transport; `discord.rs`).
- Re-test "first WM_CLOSE ignored after minimize → restore" on a desktop without GlazeWM (M5).
- The Windows media overlay shows "Unknown app" (exe version resource / AUMID at packaging, M5).
- macOS RAM with the fonts pin (M5 performance-budget pass).
- glow presents through `Composed: Copy with GPU GDI` (~20 ms to display, 0–4 dropped frames per
  20 s scroll); revisit if users report judder (flip model needs wgpu, whose idle spin would need a
  fix first).
- Before the repo goes public: the test fixtures keep the account's personal mix ids and artist-based
  mix subtitles (listening taste); replace them if that matters.
