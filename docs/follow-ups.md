# Follow-ups carried out of M0–M2

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

## M3 (player)

- A forward seek on a stale decoder whose rebuild fails reports `TrackEnded` (`engine.rs` seek path).
- A writer-thread panic in `ProgressiveFile` leaves readers blocked (store an error on unwind); EINTR
  not retried.
- `decoded` mixes seek-landing timestamps with decoded frame counts; timebase assumed `1/rate`.
- Backward-seek cost grows with position (rebuild from byte 0), unmeasured on long tracks.
- Resampler: `debug_assert` on flush shortfall; exact-length tests for 48→44.1 and 96→48 kHz.
- fastframe-audio v0.4.1 allocates in the render callback (raise upstream).

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
