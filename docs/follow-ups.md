# Follow-ups carried out of M0 + M1

Parked by task reviews and the final review. Pick each up in the milestone that touches its code.

## M2 (browse, library, UI)

- Idle RAM is over the §10 budget on both OSes: fastframe-fonts reads the system fallback faces
  whole; mapping them (upstream) saves ~78 MB.
- Previous on the first track while its stream loads fetches the stream again.
- Windows shows Buffering as Paused; the overlay shows the old position briefly on a track change.
- `ClientConfig::default()` duplicated in `now_playing.rs`; unused direct `egui` dependency in
  `crates/app`.
- `open::that_detached` runs inside an async handler (move to `spawn_blocking`).

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
- Re-test "first WM_CLOSE ignored after minimize → restore" on a desktop without GlazeWM (M5).
- The Windows media overlay shows "Unknown app" (exe version resource / AUMID at packaging, M5).
