# Follow-ups carried out of M0 + M1

Parked by task reviews and the final review. Pick each up in the milestone that touches its code.

## M2 (browse, library, UI)

- Memory while browsing, as one change: cap concurrent image fetches (semaphore), request covers at
  pixel size (HiDPI), cap `TextureCache` by bytes not entries, give `Model.albums` an eviction rule.
- Idle RAM is over the §10 budget on both OSes: fastframe-fonts reads the system fallback faces
  whole; mapping them (upstream) saves ~78 MB.
- Abort superseded playback load tasks (like `search_task`); Previous during a load re-fetches.
- Resolve `TogglePlay` in `Playback`: two media-key toggles within one frame send Play, Play on
  macOS/Linux. Windows shows Buffering as Paused; the overlay shows the old position briefly on a
  track change.
- One error dismissal per frame (a second click is lost); `ClientConfig::default()` duplicated in
  `now_playing.rs`; unused direct `egui` dependency in `crates/app`.
- `open::that` blocks inside an async handler (move to `spawn_blocking`).
- Cover-dir test uses `starts_with` (use `assert_eq` with `join("covers")`) and needs a home dir.
- Move `playback.rs` tests to their own file (904 lines, over the ~800 soft limit).

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

## CI

- Add an MSRV 1.98 job (CI runs stable only).

## M4 / M5

- Client ID override setting (README promises it as planned).
- A second instance wipes the first instance's covers (single instance, M4).
- Re-test "first WM_CLOSE ignored after minimize → restore" on a desktop without GlazeWM (M5).
- The Windows media overlay shows "Unknown app" (exe version resource / AUMID at packaging, M5).
