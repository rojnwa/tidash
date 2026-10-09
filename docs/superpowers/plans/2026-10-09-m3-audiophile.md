# M3 Audiophile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bit-perfect Hi-Res playback: exclusive output per OS with a device picker, gapless, ReplayGain, Hi-Res streams and a format badge, plus the M3 follow-ups.

**Architecture:** One ring and one `Renderer` serve every output. The ring keeps carrying `f32`, which holds any ≤ 24-bit sample exactly, so exclusive backends turn it back into the device's integers without loss (a test checks all 2^24 values). The engine owns at most one sink: fastframe's shared `Output`, or an exclusive stream opened at the track's own format (WASAPI on Windows, cpal plus hog mode on macOS, cpal `hw:` on Linux). When the device refuses, the engine falls back to shared and sends a notice. Gapless works like this: the app preloads the next track's stream 20 s before the end, the engine appends it to the same ring, and the renderer moves the clock to the new track at the exact frame where it begins.

**Tech Stack:** Rust 2024; `player`: symphonia 0.6.1, rubato 5, rtrb 0.4, fastframe-audio v0.4.1, plus `wasapi` 0.25 (Windows), `cpal` 0.18 (already in the graph), `coreaudio-rs` 0.14 (macOS, hog mode only); `app`: egui 0.36, tokio, serde/serde_json (workspace).

**Spec:** `docs/superpowers/specs/2026-10-07-tidash-design.md` (§5, §7 Settings, §9, §12, §13 M3, §16). Facts behind it: `docs/spikes/s2-audio.md` (formats the test DAC accepts, WASAPI call sequence, cpal ALSA/CoreAudio pitfalls) and `docs/spikes/s1-auth-manifests.md` (Hi-Res requests downgrade on their own on non-Hi-Res tracks). The working S2 WASAPI exclusive code: `git show a738ddb:spikes/s2-audio/src/main.rs`.

## Global Constraints

- License `GPL-3.0-or-later`; edition 2024; `rust-version = "1.98"`; workspace lints unchanged (`unsafe_code = "deny"`); CI runs `cargo clippy --all-targets -- -D warnings` and `cargo test` on Windows, macOS and Linux.
- `player` depends on no tokio, HTTP client, UI or `tidal` type. New dependencies: only `wasapi = "0.25"` (Windows target), `cpal = "0.18"`, `coreaudio-rs = "0.14"` (macOS target) in `player`, plus `serde`/`serde_json` (workspace) in `app`.
- Real-time paths (`Renderer::render`, the exclusive render loops and callbacks) never allocate, block, log, or take a lock other than fastframe's own `try_lock`.
- Exclusive = the device opened at the track's rate, bit depth and channels; no resampling, no software volume, no ReplayGain. A device that cannot do this falls back to shared with a notice (the spec §5 policy, now decided).
- Look up every library API before using it: cpal with ctx7 (`/rustaudio/cpal`); `wasapi` and `coreaudio-rs` are not on ctx7, so read their sources under `~/.cargo/registry/src/*/wasapi-0.25.0` and `coreaudio-rs-0.14.2`.
- All user-visible strings go through `tr`/`tr_n` and are added to `crates/app/po/tidash.pot`.
- Soft limit ~800 lines per file. Tests never touch the network or play audible sound; hardware tests are `#[ignore]`, render silence and run for at most 1 s.
- In the worktree, run git through PowerShell (the Bash tool's git is blocked by a worktree guard). Never synthesize input on the human's desktop; GUI and listening checks go to Task 11's acceptance session.

## Review Focus

1. **The device refuses the track's format or exclusive mode altogether** (88.2 kHz on a 96 kHz DAC, a Bluetooth headset, a device another app holds): playback continues in shared mode, the badge says Shared, and one notice shows per run of such tracks. Tests: Task 5 `a_refused_exclusive_plays_shared_with_a_fallback`, Task 10 `fallback_notice_once_per_streak`.
2. **The queue is edited in a track's last 20 s** (play next, remove, move, shuffle, repeat): the track that plays next is the one the queue shows next. Test: Task 9 `a_queue_edit_cancels_a_stale_preload`.
3. **The USB DAC is unplugged during exclusive playback**: playback falls back to shared, continues from the same position and shows one notice, with no hang. Test: Task 5 `a_lost_exclusive_device_falls_back_and_continues`; on hardware in Task 11.
4. **A seek, skip or device change in the half second after the next track is decoded but before it is heard**: the shown track, the position and what is heard stay together. Test: Task 4 `seek_before_the_boundary_stays_in_the_ending_track`.
5. **An exclusive queue mixing formats** (16/44.1 then 24/96): the device reopens after the first track's tail has played, with only a short gap and no stale audio, and the badge follows. Test: Task 5 `format_change_reopens_after_the_tail`.

---

### Task 1: `player` — interrupts reach every open file; a writer panic or EINTR never strands a reader

**Files:**
- Modify: `crates/player/src/stream.rs` (tests in its `mod tests`)

**Interfaces:**
- Produces (public API unchanged): `Interrupt::interrupt` wakes readers blocked on **any** live `ProgressiveFile` spawned with it, not just the newest one. Task 4 keeps two files alive, the current track and the preload.

- [ ] **Step 1: Write failing tests.** Wait on the reading thread through a channel with `recv_timeout(2 s)`, so a failing test fails and does not hang.
  - `interrupt_wakes_a_reader_on_an_older_file`: interrupt `i`; spawn `a` (a `Gate` reader) and then `b` (another `Gate`) with `i`; block a read on `a` on a thread; `i.interrupt()`. The read returns an error with `is_interrupt` within 100 ms.
  - `a_panicking_source_fails_reads`: a reader that serves 10 bytes, then `panic!("boom")`. `read_to_end` returns `Err` (kind `Other`, message contains `panicked`) within 1 s.
  - `eintr_is_retried`: a reader that returns `Err(ErrorKind::Interrupted)` once mid-stream. `read_to_end` equals all the data.
- [ ] **Step 2: Run** `cargo test -p player --lib stream`. The three tests FAIL.
- [ ] **Step 3: Implement.**
  - `Raised.file` becomes `Mutex<Vec<Weak<(Mutex<State>, Condvar)>>>`: `spawn` prunes the dead entries and pushes the new one; `interrupt` notifies every live one.
  - The writer thread holds a guard whose `Drop` stores `error = Some((Other, "download thread panicked"))` when neither `done` nor `error` is set, then calls `notify_all`.
  - `copy_loop` retries a read that fails with `ErrorKind::Interrupted`.
- [ ] **Step 4: Run** `cargo test -p player` and `cargo clippy -p player --all-targets -- -D warnings`. Both PASS.
- [ ] **Step 5: Commit** `fix(player): interrupts reach every open file; a writer panic fails reads`.

### Task 2: `player` — the resampler counts its output

**Files:**
- Modify: `crates/player/src/resample.rs` (tests in its `mod tests`)

**Interfaces:**
- Produces: `Resampler::expected_out(&self) -> u64`: the output frames that the input fed so far will produce, `round(in_frames × to / from)`. Passthrough counts too (= input frames). Task 4 sets gapless boundaries with it.

- [ ] **Step 1: Write failing tests.**
  - `exact_length_48000_to_44100`: 48 000 stereo frames go in; `process` + `flush` puts out exactly 44 100 frames, and `expected_out() == 44_100`.
  - `exact_length_96000_to_48000`: 96 000 frames → exactly 48 000.
  - `exact_length_44100_to_48000`: 44 100 frames → exactly 48 000 (tightens `ratio_44100_to_48000_length`).
  - `expected_out_tracks_input`: 1 000 frames at 44.1 → 48 kHz, before any flush, give `expected_out() == 1088`. A passthrough after 1 000 frames gives 1 000.
  - `expected_out_matches_flushed_output`: the odd chunk sizes of `chunked_equals_whole`; after `flush`, the output frames equal `expected_out()`.
- [ ] **Step 2: Run** `cargo test -p player --lib resample`. The new tests FAIL.
- [ ] **Step 3: Implement.**
  - `in_frames` moves from `Inner` into `Resampler`, so passthrough counts it too.
  - `flush` ends with `debug_assert!(out_frames >= target, "flush shortfall: …")`.
  - `flush_stops_when_target_unreachable` gets `#[cfg_attr(debug_assertions, should_panic(expected = "shortfall"))]` and corrupts the moved counter.
- [ ] **Step 4: Run** `cargo test -p player` and clippy. Both PASS.
- [ ] **Step 5: Commit** `fix(player): the resampler counts its output; exact-length tests`.

### Task 3: `player` — seek fixes

**Files:**
- Modify: `crates/player/src/decode.rs`, `crates/player/src/engine.rs`
- Test: unit tests in `decode.rs`; engine unit tests in `engine.rs`

**Interfaces:**
- Produces: `Decoder::seek` converts with the track's `time_base` (symphonia `Track::time_base`, `1/rate` when absent), both the target and the landing timestamp. `Track::decoded` then stays in frames whatever the time base.

- [ ] **Step 1: Write failing tests.**
  - `decode.rs`: `time_base_other_than_the_rate_converts`. With `TimeBase` 1/1000, the private conversion helpers turn 1.5 s into ts 1500 and ts 1500 back into 1.5 s. With 1/96000, 250 ms is ts 24 000.
  - `engine.rs`: `a_failed_forward_seek_on_a_stale_decoder_is_an_error`. Like `a_failed_backward_seek_is_an_error`, but after five packets set `t.stale = true`, swap in the garbage file and seek **forward** to 400 ms. The result is `Error { id: 1, .. }`, and no `TrackEnded(1)`.
- [ ] **Step 2: Run** `cargo test -p player --lib`. Both FAIL.
- [ ] **Step 3: Implement.**
  - `Decoder` stores the time base. `Engine::seek` computes `let rebuilt = t.stale || t.behind(to)` before `t.seek`, and when that seek fails the failure is an error (only a refused seek on an intact decoder means "past the end").
  - `Track::seek` logs at debug when it rebuilt the decoder: `seek to {to:?} rebuilt the decoder in {elapsed:?}` (Task 11 measures it on a long track).
- [ ] **Step 4: Run** `cargo test -p player` and clippy. Both PASS.
- [ ] **Step 5: Commit** `fix(player): seeks use the stream's time base; a failed stale forward seek is an error`.

### Task 4: `player` — gapless playback and per-track gain

**Files:**
- Create: `crates/player/src/track.rs` (`Track` and its impl moved from `engine.rs`, plus `Next`), `crates/player/src/engine/tests.rs` (`engine.rs`'s unit tests moved there via `#[cfg(test)] #[path = "engine/tests.rs"] mod tests;`), `crates/player/tests/fixtures/s16_44100_mono.mp4`
- Modify: `crates/player/src/engine.rs`, `crates/player/src/render.rs`, `crates/player/src/lib.rs`, `crates/player/examples/play_file.rs`, `crates/app/src/playback.rs` (`Source` literal gets `gain: 1.0`)
- Test: `crates/player/tests/engine.rs`

**Interfaces:**
- Consumes: `Resampler::expected_out` (Task 2); interrupts that reach every file (Task 1).
- Produces:
  - `Source { id: u64, reader: Box<dyn Read + Send>, len: Option<u64>, gain: f32 }`. `gain` is linear and multiplies each decoded sample (`s as f32 / 2^31 * gain`); `1.0` leaves it unchanged.
  - `PlayerCmd::Preload(Option<Source>)`. `Some` means play this right after the current track, replacing any earlier preload; it is dropped when nothing is loaded. `None` cancels a preload the engine has not switched to yet. `Load` and `Stop` drop any preload.
  - The events' meaning, written in the doc comments: `TrackStarted(id)` means `id` is heard from now on, and if another track was playing, that track ended there (gapless, or after a short gap). `TrackEnded(id)` means playback stopped at `id`'s natural end with no preload after it.
  - Crate-internal: `Shared.boundary: AtomicU64` (`u64::MAX` = none) and `Shared.switches: AtomicU64`.

- [ ] **Step 1: Move code only.** Move `Track` to `track.rs` and the engine unit tests to `engine/tests.rs`, changing nothing else. `cargo test -p player` passes. Commit `refactor(player): track and engine tests in their own files`.
- [ ] **Step 2: Generate the mono fixture** with the M1 fixture command (plan `2026-10-07-m0-m1…` Task "Generate fixtures", same `$F` flags): `ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=0.5" -ac 1 -c:a flac -sample_fmt s16 $F s16_44100_mono.mp4`. Check that it contains several `moof` boxes.
- [ ] **Step 3: Write failing tests** in `tests/engine.rs`. `raw(name)` is the `.raw` reference as `f32` (`i32 as f32 / 2^31`). "Trimmed" means leading and trailing all-zero frames are dropped. `Source` literals gain `gain: 1.0`.
  - `gapless_is_sample_exact`: configure 44 100/2; `Load` A (`s16_44100`, id 1), then immediately `Preload(Some(B))` (`s16_44100`, id 2); render until `State(Stopped)`. Trimmed output == trimmed `raw(A) ++ raw(B)`, sample for sample. Events in order: `TrackStarted(1)`, `TrackStarted(2)`, `TrackEnded(2)`; no `TrackEnded(1)`.
  - `gapless_resampled_matches_one_resample`: configure 48 000/2, with the same A and B. Trimmed output == trimmed `Resampler::new(44_100, 48_000, 2)` run over `raw(A) ++ raw(B)` and flushed, exactly (one resampler sees the same input).
  - `position_restarts_at_the_boundary`: when `TrackStarted(2)` arrives, `position() <= 100 ms`; at `TrackEnded(2)`, `position()` is within 10 ms of 500 ms.
  - `a_late_preload_still_follows`: `Load` A, render 100 ms (A is fully decoded by then), then `Preload(Some(B))`. `TrackStarted(2)` follows, there is no `TrackEnded(1)`, and the non-silent frames are within 1 % of 48 000 at 48 kHz.
  - `seek_before_the_boundary_stays_in_the_ending_track`: `Load` A, `Preload` B, render 50 ms (the switch to B is decoded, not heard), `Seek(100 ms)`. The position reads 100 ms (A's), `TrackStarted(2)` still comes, and the non-silent frames after the seek are within 1 % of 900 ms.
  - `cancelled_preload_is_not_played`: `Load` A, `Preload(Some(B))`, `Preload(None)`. `TrackEnded(1)` arrives and no `TrackStarted(2)`.
  - `preload_after_the_end_is_dropped`: render A to `State(Stopped)`, then `Preload(Some(B))`. No `TrackStarted(2)` arrives within 200 ms.
  - `a_preload_that_fails_to_open_ends_the_track_normally`: B's reader is `io::empty()`. `TrackEnded(1)` arrives and no `Error` event.
  - `a_mono_track_follows_after_a_flush`: B = `s16_44100_mono`. `TrackStarted(2)` arrives, there is no `TrackEnded(1)`, and B's non-silent frames are within 1 % of 500 ms.
  - `gain_scales_output`: like `volume_scales_output`, with `gain: 0.5` on the source instead of the volume.
- [ ] **Step 4: Run** `cargo test -p player --test engine`. The new tests FAIL.
- [ ] **Step 5: Implement.** These decisions are fixed; the code is the implementer's:
  - `Preload(Some(src))` with a track loaded sets `next = Next { id, file: ProgressiveFile::spawn(..), gain }`. The download starts; there is no probe yet. `Preload` never raises the interrupt.
  - Once the current track A's decoder returns `None`, or while A's tail is still in the ring (`eof`), and `next` is set with no boundary pending, probe it with `Decoder::new(file.forward())`. If the probe fails or is superseded, drop `next`, and A ends as before.
  - `seamless = A.channels == B.channels` (Task 5 adds the exclusive condition).
    - **Seamless:** B takes A's `pending`. When the rates are equal and A's resampler is not flushed, B keeps A's resampler; otherwise A's resampler is flushed into the pending samples (if it is not already) and B gets a new one.
    - The boundary is computed before anything moves: `boundary = A.resampler.expected_out() - A.out_mark`. `B.out_mark` is the kept resampler's `expected_out()` at the switch, or 0 with a new one.
    - Store `boundary` in `Shared.boundary`; set `ending = Some(A)` and `track = Some(B)` with `started = true`.
    - **Not seamless:** A drains as at a track's end, then B starts through the Load path (factor `begin(id, file, gain)` out of `load`), which flushes; B's `TrackStarted` comes as for any Load.
  - **Renderer:** after counting `n` frames, if `frames >= boundary`: `base_ns = 0`, `frames -= boundary`, `boundary = MAX`, `switches += 1`. `Engine::flush` stores `boundary = MAX` before it bumps `flush_req`.
  - **Engine loop:** once `switches` differs from `switches_seen`, clear `ending` and emit `TrackStarted(track.id)`.
  - **Seek while `ending` is set** (also via `adopt_ring`): first undo the switch. `next` gets B's file back (B's decoder is dropped), `track = A`, and `switches_seen` is resynced. Then seek A as usual.
  - `play_file` takes an optional second path, which it preloads (a manual gapless check).
- [ ] **Step 6: Run** `cargo test -p player` and clippy. Both PASS. `engine.rs` stays under ~800 lines.
- [ ] **Step 7: Commit** `feat(player): gapless playback and per-track gain`.

### Task 5: `player` — shared or exclusive output, and format events

**Files:**
- Create: `crates/player/src/exclusive.rs`, `crates/player/tests/fixtures/s24_192000.{mp4,raw}`
- Modify: `crates/player/src/engine.rs`, `crates/player/src/render.rs`, `crates/player/src/decode.rs`, `crates/player/src/lib.rs`, `crates/player/examples/play_file.rs`, `crates/app/src/backend.rs` (`spawn` call)
- Test: `crates/player/src/engine/tests.rs`, unit tests in `exclusive.rs`

**Interfaces:**
- Consumes: Task 4's `begin`, gapless switch and `Source.gain`.
- Produces (pub, re-exported from `lib.rs`):
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Format { pub rate: u32, pub bits: u16, pub channels: u16 }`, in `decode.rs`, with `Decoder::format(&self) -> Format`.
  - `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct OutputConfig { pub device: fastframe_audio::Device, pub exclusive: bool }`.
  - `Player::spawn(config: OutputConfig) -> io::Result<(Player, Receiver<PlayerEvent>)>`, replacing the `OutputOptions` parameter. It still opens shared output first, so a missing device fails here as today.
  - `PlayerCmd::SetOutput(OutputConfig)`.
  - `PlayerEvent::Format { track: Format, exclusive: bool, device_rate: u32 }`, sent with every `TrackStarted` and whenever the output under a started track changes.
  - `PlayerEvent::Fallback(String)`: exclusive was refused or lost (the reason); shared output plays now.
- Produces (crate) for Tasks 6–7:
  - `exclusive::Exclusive`: an open exclusive stream at one `Format` that pulls from a `Renderer`; methods `pause(&mut self)`, `resume(&mut self)`, `is_paused(&self) -> bool`, `failed(&self) -> bool`; dropping it closes the device.
  - `exclusive::open(device: &fastframe_audio::Device, format: Format, renderer: Renderer) -> Result<Exclusive, String>`. It configures the renderer at `format.rate`/`channels` before it starts; on platforms without a backend yet it returns `Err("exclusive output is not supported on this platform")`.
  - `exclusive::to_i32(v: f32) -> i32`, left-justified with saturation: `(v * 2_147_483_648.0) as i32`.
  - `#[cfg(test)] Exclusive::detached(format: Format) -> (Exclusive, Arc<AtomicBool>)`: no device, and the flag sets `failed`.
  - Engine field `open_exclusive: Box<dyn FnMut(&Device, Format, Renderer) -> Result<Exclusive, String> + Send>` (`exclusive::open`, which tests replace).
  - `Renderer::new` gives a renderer that continues the shared clock, so any number of renderers can be made over a player's life, one in use at a time. It starts at `acked = flush_ack`, `src_channels = shared.channels`, and `configure` counts heard frames at `shared.device_rate`, not its own rate.

- [ ] **Step 1: Generate the fixture** the M1 way: `sine=frequency=1000:sample_rate=192000:duration=0.25`, `-ac 2 -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 $F s24_192000.mp4`, then `ffmpeg -i s24_192000.mp4 -f s32le s24_192000.raw`.
- [ ] **Step 2: Write failing tests.**
  - `exclusive.rs`: `to_i32_round_trips_every_24_bit_value`. For every `i` in `-(1 << 23)..(1 << 23)`, `to_i32(((i << 8) as f32) / 2^31) == i << 8`; also `to_i32(-1.0) == i32::MIN`.
  - `engine/tests.rs`: use `parts()`, driven synchronously as in `output_runs_only_with_an_unpaused_track`. The test opener records each `Format` it is asked for, accepts or refuses per a predicate, configures the renderer it gets, sends it to the test over a channel and returns `Exclusive::detached`.
    - `exclusive_output_is_bit_identical` (`s16_44100`, `s24_96000`, `s24_192000`): exclusive config; `Load`. The opener gets `Format { rate, bits, channels: 2 }`. Render from the handed renderer until `TrackEnded`. The trimmed samples, each through `to_i32`, equal the trimmed `.raw` reference.
    - `exclusive_ignores_volume_and_gain`: `SetVolume(0.5)` and `gain: 0.5`; still bit-identical.
    - `a_refused_exclusive_plays_shared_with_a_fallback`: the opener refuses with `"no 88.2 kHz"`. Events: `Fallback("no 88.2 kHz")`, then `Format { exclusive: false, device_rate: 48_000, .. }`, and audio comes from the test's shared renderer, which is configured at 48 kHz.
    - `same_format_next_track_keeps_the_device`: exclusive; A and the preloaded B are both `s16_44100`. The opener is called once, and `TrackStarted(2)` comes without `TrackEnded(1)`.
    - `format_change_reopens_after_the_tail`: exclusive; A `s16_44100`, preloaded B `s24_96000`. The opener is called twice (the second time at 96 000/24). The first renderer gives A's full 22 050 frames (bit-identical), then `TrackStarted(2)` and `Format { track.rate: 96_000, exclusive: true, .. }`.
    - `a_lost_exclusive_device_falls_back_and_continues`: render 100 ms, set the detached flag, run `maintain`. Events: `Fallback`, then `Format { exclusive: false, .. }`, and the position stays ≥ the position before.
    - `set_output_mid_track_continues_from_the_position`: shared, render 200 ms, then `SetOutput(exclusive)`. The opener is called with the track's format, and the position stays within 20 ms.
    - `a_new_renderer_continues_the_position`: render 100 ms; a second `Renderer::new`, configured at 44 100, takes over; `position()` moves by less than 1 ms.
- [ ] **Step 3: Run** `cargo test -p player`. The new tests FAIL.
- [ ] **Step 4: Implement.**
  - `enum Sink { Shared(Output<Renderer>), Exclusive(Exclusive) }`; the engine holds `sink: Option<Sink>`. `spawn_manual` sets `manual = true`, and there opening shared output is a no-op: the test's renderer is the shared output.
  - Each sink gets a fresh `Renderer::new`.
  - `begin` probes first, then `use_output(format)`:
    - With an exclusive config, an `Exclusive` already at that format is kept. Otherwise, close the current sink first (some devices take one stream), then `open_exclusive`. On `Err(reason)`, open shared and emit `Fallback(reason)`.
    - With a shared config, keep or open shared.
    - Then `adopt_ring`, and build the resampler for the device rate (the passthrough when exclusive).
  - `Shared.exclusive: AtomicBool` tells the renderer to use volume 1.0; the engine uses gain 1.0 while the sink is exclusive.
  - Task 4's `seamless` becomes `channels equal && (!exclusive || A.format == B.format)`.
  - `SetOutput`: store the config. With a track loaded, `use_output(track format)`; `adopt_ring` re-seeks to the position. Without a track, a shared config reopens shared now; an exclusive config waits for the next Load.
  - `maintain`: an `Exclusive` with `failed()` is dropped, shared opens, and `Fallback("audio device lost")` goes out; playback continues from the position. Shared works as today.
  - `sync_output` pauses and resumes whichever sink there is. The exclusive device stays held while paused: `// ponytail: exclusive device held while paused; release after a long pause like shared if users ask`.
  - `play_file` takes `--exclusive` and prints `Format`/`Fallback`.
  - The app's `open_player` passes `player::OutputConfig::default()` (Task 8 passes the settings).
- [ ] **Step 5: Run** `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`. Both PASS.
- [ ] **Step 6: Commit** `feat(player): exclusive or shared output with format events`.

### Task 6: `player` — Windows exclusive output (WASAPI)

**Files:**
- Create: `crates/player/src/exclusive/wasapi.rs`
- Modify: `crates/player/Cargo.toml` (`[target.'cfg(windows)'.dependencies] wasapi = "0.25"`), `crates/player/src/exclusive.rs` (Windows `open` dispatch)

**Interfaces:**
- Consumes: `exclusive::open`'s signature, `Exclusive`, `to_i32`, `Renderer` (Task 5).
- Produces: `exclusive::open` on Windows.

- [ ] **Step 1: Write failing unit tests.**
  - `container_bytes`: the left-justified `0x1234_5600` packs to `[0x56, 0x34, 0x12]` (packed 24) and `[0x00, 0x56, 0x34, 0x12]` (24-in-32 and 32); `0x1234_0000` as 16-bit is `[0x34, 0x12]`.
  - `candidates_by_bits`: 16-bit gives [16, packed 24, 24-in-32, 32]; 24-bit gives [packed 24, 24-in-32, 32]. Each is `(store bits, valid bits)`.
- [ ] **Step 2: Run** `cargo test -p player --lib wasapi`. FAIL.
- [ ] **Step 3: Implement.**
  - **Thread and device:** each open stream gets one thread named `exclusive-output`, in the COM MTA, which owns the `AudioClient` and the `Renderer`; `open` waits up to 2 s for its init result.
    - `Device::Default` is the default render device. `Named(n)` is the render device whose friendly name is `n`; if there is none, return `Err("device {n} not found")`.
  - **Format:** the first candidate that `is_supported_exclusive_with_quirks` accepts (`WaveFormat::new(store, valid, &SampleType::Int, rate, channels, None)`). If none is accepted, return `Err("{rate} Hz / {bits}-bit is not supported in exclusive mode")`.
  - **Init:** `calculate_aligned_period_near(default period, Some(128), ..)` and `StreamMode::EventsExclusive`, with the `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` retry, both lifted from S2's `init_exclusive`.
  - **Loop:**
    - `renderer.configure(rate, channels)`, then pre-fill one buffer before `start_stream`.
    - On each event, render `buffer_frames × channels` into a preallocated `Vec<f32>`, convert through `to_i32` into a preallocated byte buffer, then `write_to_device`.
    - Pause stops the stream and parks the thread (no wakeups); resume unparks it and starts the stream.
    - An error, or 2 s with no event while running, sets `failed` and ends the thread.
    - Drop stops the thread and joins it.
  - **Log:** once at open, at info: `exclusive output: {device}, {rate} Hz, {container}, period {ms} ms`.
- [ ] **Step 4: Add `#[ignore]` hardware tests:** `exclusive_opens_the_default_device`. Open the default device at 44 100/16/2 and then at 96 000/24/2, each with a paused player so it plays silence; run 200 ms, pause, resume, run 200 ms, drop. Not failed. Also check that `fastframe_audio::output_device_names()` resolves via friendly name to the same device. Run `cargo test -p player --lib -- --ignored exclusive` on this machine (silent, ≤ 1 s) and quote the log lines in the report.
- [ ] **Step 5: Run** `cargo test -p player` and clippy. PASS.
- [ ] **Step 6: Commit** `feat(player): WASAPI exclusive output`.

### Task 7: `player` — macOS and Linux exclusive output (cpal; hog mode on macOS)

**Files:**
- Create: `crates/player/src/exclusive/cpal.rs` (compiled on every OS so Windows type-checks it; `#[cfg_attr(windows, expect(dead_code))]` on the module)
- Modify: `crates/player/Cargo.toml` (`cpal = "0.18"`; `[target.'cfg(target_os = "macos")'.dependencies] coreaudio-rs = "0.14"` with the features that expose `audio_unit::macos_helpers`), `crates/player/src/exclusive.rs` (non-Windows `open` dispatch)

**Interfaces:**
- Consumes: as in Task 6.
- Produces: `exclusive::open` on macOS and Linux; `fn pick(ranges: &[cpal::SupportedStreamConfigRange], format: Format) -> Option<(cpal::StreamConfig, cpal::SampleFormat)>`.

- [ ] **Step 1: Write failing unit tests** (they run on every OS).
  - `pick_needs_the_exact_rate_and_channels`: one `I32` stereo range of 44 100–48 000. 48 000/24/2 gives `Some`; 96 000/24/2 gives `None`; 48 000/24/1 gives `None`.
  - `pick_prefers_i32_for_24_bit`: `I24` and `I32` ranges; 24-bit gets `I32`.
  - `pick_prefers_i16_for_16_bit`: `I16`, `I24` and `I32` ranges; 16-bit gets `I16`.
  - The `StreamConfig` uses `BufferSize::Fixed(rate / 100)`, clamped to the range's buffer limits when they are known.
- [ ] **Step 2: Run** `cargo test -p player --lib cpal`. FAIL.
- [ ] **Step 3: Implement.**
  - **Formats:** 16-bit sources prefer `I16`, then `I32`, `I24`; 24-bit sources prefer `I32`, then `I24`. Nothing else qualifies.
  - **Device:** `cpal::default_host()`. `Default` is the default output; `Named(n)` is an output device whose `to_string() == n`. On Linux, pick among those (or the default's name) the one whose ALSA PCM id (`DeviceTrait::id`) starts with `hw:`; with none, return `Err("exclusive output needs a hw: device")`. Linux needs the exact rate (cpal's ALSA rate is "nearest").
  - **macOS:** before building, look up the CoreAudio device id (`get_device_id_from_name(n, false)` / `get_default_device_id(false)`). If `get_hogging_pid` is neither -1 nor our pid, return `Err("the device is in exclusive use by another app")`. `toggle_hog_mode` must return `std::process::id()`; the guard toggles hog mode back on drop. cpal sets the physical format and nominal rate itself.
  - **Stream:** `renderer.configure(rate, channels)`, then `build_output_stream::<T>` per format. `I32` takes `to_i32(v)`; `I24` takes `cpal::I24::new(to_i32(v) >> 8)` (check the exact constructor); `I16` takes `(to_i32(v) >> 16) as i16`.
    - The callback renders into a scratch buffer preallocated for the fixed period (in pieces when asked for more) and never allocates.
    - The error callback sets `failed`; pause and resume map to `StreamTrait`.
  - **Log:** once at open, at info, as in Task 6. `// ponytail: DACs that take only S24_3LE fall back to shared on Linux; the alsa crate can drive them if users ask.`
- [ ] **Step 4: Run** `cargo test -p player` and clippy on Windows. PASS.
- [ ] **Step 5: macOS and Linux CI.** Push the branch and run CI (`gh workflow run ci.yml --ref <branch>`, then `gh run watch`). Fix what fails until all three OSes pass. Add the same `#[ignore]` hardware test as Task 6 (silent), for the human to run on a Mac.
- [ ] **Step 6: Commit** `feat(player): exclusive output on macOS (hog mode) and Linux (hw:)`.

### Task 8: `app` — audio settings

**Files:**
- Create: `crates/app/src/settings.rs`, `crates/app/src/ui/settings.rs`
- Modify: `crates/app/Cargo.toml` (`serde.workspace = true`, `serde_json.workspace = true`), `crates/app/src/main.rs` (`mod settings`), `messages.rs`, `model.rs`, `pages.rs`, `ui/mod.rs`, `ui/sidebar.rs`, `backend.rs`, `playback.rs`, `po/tidash.pot`
- Test: unit tests in `settings.rs`, `model.rs`, `ui/settings.rs`, `backend/tests.rs`

**Interfaces:**
- Consumes: `player::OutputConfig`, `PlayerCmd::SetOutput`, `Source.gain` (Tasks 4–5).
- Produces:
  - `settings.rs`:
    - `#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] #[serde(default)] pub struct Settings { pub audio: Audio }`.
    - `#[serde(default)] pub struct Audio { pub device: Option<String>, pub exclusive: bool, pub normalization: Normalization, pub hi_res: bool }`, defaulting to `None` (system default), `false`, `Off`, `true`.
    - `pub enum Normalization { #[default] Off, Track, Album }`.
    - `Settings::load(path: &Path) -> Settings`: missing or unreadable gives the default, logged.
    - `Settings::save(&self, path: &Path) -> io::Result<()>`: writes `.tmp`, then renames.
    - `pub fn path() -> Option<PathBuf>`: `ProjectDirs::config_dir()/settings.json`.
    - `Audio::output(&self) -> player::OutputConfig`.
    - `pub fn replay_gain(mode: Normalization, info: &tidal::StreamInfo) -> f32`.
  - `Action::SetAudio(settings::Audio)`, `Action::ListDevices`; `Event::Settings(settings::Settings)`, `Event::Devices(Vec<String>)`.
  - `Route::Settings`; `Model.settings: Settings` and `Model.devices: Vec<String>`, both kept across `LoggedOut`.
  - `Playback::new(player, emit, msgs, audio: settings::Audio)`; `Playback::set_audio(&mut self, audio: settings::Audio)` sends `SetOutput` only when the device or exclusive mode changed. Quality and normalization apply from the next stream request: `Quality::HiResLossless` if `hi_res`, else `Lossless`; `gain = replay_gain(..)`.
  - Backend field `settings_path: Option<PathBuf>`, which tests point at a temp dir.

- [ ] **Step 1: Write failing tests.**
  - `replay_gain_track_album_off`: `track_gain -6.0206, track_peak 0.5, album_gain -3.0, album_peak 0.9`. `Off` gives `1.0`, `Track` ≈ `0.5` and `Album` ≈ `0.7079` (each ±1e-4).
  - `replay_gain_never_clips`: `track_gain +6.0, track_peak 0.9`; `Track` gives `1/0.9`.
  - `replay_gain_falls_back_to_track_values`: no album values, so `Album` == `Track`; no gain at all gives `1.0`.
  - `settings_load_save_and_defaults`: a missing file gives the defaults above; save then load is equal; `{"audio":{"exclusive":true}}` gives the other fields' defaults; garbage gives the default.
  - `model.rs`: `settings_and_devices_survive_logout`.
  - `ui/settings.rs` (headless pass, as the queue panel tests do): `asks_for_devices_once_per_visit` (two passes send one `ListDevices`; leaving and coming back sends another) and `exclusive_checkbox_sends_set_audio` (the click sends `SetAudio(Audio { exclusive: true, .. })`).
  - `backend/tests.rs`: `set_audio_saves_and_announces`. After `SetAudio`, the temp-dir file holds `"exclusive": true` and `Event::Settings` was sent.
- [ ] **Step 2: Run** `cargo test -p tidash`. FAIL.
- [ ] **Step 3: Implement.**
  - **`replay_gain`:** `10^(dB / 20)`; with peak `p > 0`, at most `1 / p`. `Album` uses the album values, each falling back to the track value; a missing gain gives `1.0`.
  - **Backend:**
    - At start, read the settings before the runtime, as the token store is (blocking): `Event::Settings`, `open_player(.., settings.audio.output())`, `Playback::new(.., audio)`.
    - `ListDevices` runs `fastframe_audio::output_device_names()` in `spawn_blocking` and sends `Event::Devices` (empty on error, logged).
    - `SetAudio` stores and saves (`spawn_blocking`; a failure sends `Event::Error(tr("Settings could not be saved") + ": {e}")`), sends `Event::Settings`, then calls `set_audio`.
  - **View** (`ui/settings.rs`; the sidebar gets a "Settings" entry; `needs_load` and `pages::fetch` treat it like Search):
    - Heading "Settings", section "Audio".
    - "Output device": a ComboBox of "System default" plus the devices; a saved device that is missing still shows as the selected text.
    - "Exclusive mode" checkbox, with the hint: "Bit-perfect: each track plays at its own sample rate and bit depth, without other apps' sound and without volume control. Falls back to shared mode when the device refuses."
    - "Quality" radios: "Hi-Res (up to 24-bit, 192 kHz)" and "Lossless (16-bit, 44.1 kHz)".
    - "Normalization" radios: "Off", "Track", "Album".
    - Every change sends `SetAudio`; the UI never mutates `Model`.
    - `ListDevices` is sent when the view shows and `UiState.settings_listed` is false; `ui::draw` clears the flag on any other route.
- [ ] **Step 4: Run** `cargo test --workspace` and clippy. PASS.
- [ ] **Step 5: Commit** `feat(app): audio settings with device picker, exclusive mode, quality and normalization`.

### Task 9: `app` — gapless: preload the next track

**Files:**
- Modify: `crates/app/src/queue.rs`, `crates/app/src/playback.rs`, `crates/app/src/backend.rs` (`tick` gets the client)
- Test: `crates/app/src/queue.rs` tests, `crates/app/src/playback/tests.rs`

**Interfaces:**
- Consumes: `PlayerCmd::Preload(Option<Source>)` and the `TrackStarted`/`TrackEnded` meanings (Task 4); Task 8's stream request (quality, gain).
- Produces: `Queue::peek_advance(&self) -> Option<&Track>`, which returns what `advance` would make current and moves nothing; `Playback::tick(&mut self, c: Option<&Arc<Client>>)`. `start_current`'s request is factored out into `fn request(&self, c: &Arc<Client>, track_id: u64, id: u64)`, which sends `Msg::Loaded { seq: id, client, result: Result<Resolved, _> }` with `pub struct Resolved { source: Source, quality: String, codec: String }`. `Playback` keeps `codec` for the current track (for Task 10) as it keeps `quality`.

- [ ] **Step 1: Write failing tests.**
  - `queue.rs`: `peek_advance_matches_advance`. For repeat Off, All and One, mid-queue and at the end, with an unavailable track in between, `peek_advance()` equals what a copy's `advance()` returns, and the queue is unchanged.
  - `playback/tests.rs` (idle runtime, as in the existing tests):
    - `preloads_the_next_track_near_the_end`: playing track 0 of three, 200 s long. At 185 s a tick records a preload for track 1; at 170 s nothing.
    - `a_preloaded_track_starts_without_a_load`: a sent preload with id X; `TrackStarted(X)` makes track 1 current with `seq == X`, `loading == false`, and starts a new play.
    - `a_queue_edit_cancels_a_stale_preload`: a sent preload for track 1, then `PlayNext([track 9])` clears it. A `TrackStarted(X)` that comes after makes track 9 current and starts loading it.
    - `a_late_preload_result_is_dropped`: the preload is still resolving; a queue edit, then `Msg::Loaded { seq: X, .. }` with `Ok`, leaves no preload record and sends nothing.
    - `repeat_one_preloads_the_same_track`.
- [ ] **Step 2: Run** `cargo test -p tidash`. FAIL.
- [ ] **Step 3: Implement.**
  - **Lead time:** `const PRELOAD_LEAD: Duration = Duration::from_secs(20)`. A tick that is playing, not loading, with no preload yet, and with `duration − position ≤ PRELOAD_LEAD`, calls `request` for `peek_advance()` under a fresh id, and records `Preload { id, track_id, sent: false, quality, codec }`.
  - **Ids:** loads and preloads take ids from one counter (`fresh_id`), so ids never repeat. `supersede` sets `seq = fresh_id()` and clears the preload and `cancelled` (the player drops its preload on Load and Stop).
  - **Result:** `Msg::Loaded` with the preload's id. `Ok` sends `Preload(Some(source))` and sets `sent = true`. An `Err` is logged at debug and the record stays, so it is not retried and the track loads at the end as before. An expired session is handled as today.
  - **Queue edits:** after every change (`edit_queue`, `RemoveFromQueue`), if the preload's track is no longer `peek_advance()`, drop the record; if it was sent, send `Preload(None)` and remember its id in `cancelled`.
  - **Track starts:** `TrackStarted(id)` with the sent preload's id: the current play ends `End::Natural`, the queue advances, and `seq`, quality and codec come from the preload; then it is handled as any `TrackStarted`. `TrackStarted(id)` with the `cancelled` id: the player went on before the cancel landed, so it is handled as the current track's natural end (advance, `start_current`).
- [ ] **Step 4: Run** `cargo test --workspace` and clippy. PASS.
- [ ] **Step 5: Commit** `feat(app): preload the next track for gapless playback`.

### Task 10: `app` — format badge and the exclusive volume

**Files:**
- Modify: `crates/app/src/model.rs`, `crates/app/src/playback.rs`, `crates/app/src/ui/player_bar.rs`, `crates/app/po/tidash.pot`
- Test: unit tests in `player_bar.rs`; `crates/app/src/playback/tests.rs`

**Interfaces:**
- Consumes: `PlayerEvent::Format` and `Fallback` (Task 5); the codec saved with each load and preload (Task 9).
- Produces: `#[derive(Clone, Debug, PartialEq)] pub struct OutputInfo { pub codec: String, pub format: player::Format, pub exclusive: bool, pub device_rate: u32 }` in `model.rs`; `PlaybackSnapshot.output: Option<OutputInfo>`; `player_bar::badge(o: &OutputInfo) -> String`.

- [ ] **Step 1: Write failing tests.**
  - `badge_formats`:
    - 24/96 exclusive gives `24/96 FLAC · Exclusive`.
    - 16/44.1 shared at 44 100 gives `16/44.1 FLAC · Shared`.
    - 24/192 shared at 48 000 gives `24/192 FLAC · Shared 48 kHz`.
    - 24/176.4 exclusive gives `24/176.4 FLAC · Exclusive`.
  - `format_event_fills_the_snapshot`: with codec `flac`, `Format { .. }` publishes `output: Some(OutputInfo { codec: "FLAC", .. })`; `stop` clears it.
  - `fallback_notice_once_per_streak`: `Fallback`, `Fallback` gives one error; then `Format { exclusive: true }` and `Fallback` give a second.
- [ ] **Step 2: Run** `cargo test -p tidash`. FAIL.
- [ ] **Step 3: Implement.**
  - **Badge:** the kHz values are the rate / 1000 with no trailing `.0`. It sits right of the volume slider. Hover text: "Exclusive: bit-perfect, volume fixed", or "Shared: mixed with other apps" plus " · resampled to {kHz} kHz" when the device rate differs.
  - **Volume:** the slider is disabled while `exclusive`, with the hover text "Volume is fixed in exclusive mode".
  - **Fallback:** sends `Event::Error(tr("Exclusive output unavailable ({reason}); playing in shared mode"))`, but only while no fallback has been shown since the last exclusive `Format`.
- [ ] **Step 4: Run** `cargo test --workspace` and clippy. PASS.
- [ ] **Step 5: Commit** `feat(app): format and output badge; fixed volume in exclusive mode`.

### Task 11: Docs, measurements and acceptance

**Files:**
- Modify: `docs/superpowers/specs/2026-10-07-tidash-design.md`, `docs/follow-ups.md`

- [ ] **Step 1: Measure the backward-seek cost** (controller, release build, the human's stored session). Use Task 3's debug line on a Hi-Res track of at least 5 minutes, seeking back from its last minute. Record the time in follow-ups: close the item if it is under ~300 ms, else keep it with the number.
- [ ] **Step 2: Update the spec.**
  - §5 Decode: the `f32` ring is exact for ≤ 24-bit (the round-trip test), replacing "integers end to end".
  - §5 Output table: macOS is cpal plus coreaudio-rs hog mode; Linux is cpal on `hw:` (I32/I24/I16; S24_3LE-only DACs fall back).
  - §5: the unsupported-rate policy (shared with a notice) is decided.
  - §5 Gapless: the app preloads 20 s before the end; `NearEnd` is dropped.
  - §5 Events: as built.
  - §13: an M3 outcome line.
  - §15: `alsa` unused; `coreaudio-rs` only for hog mode.
  - §16: the rate row is decided.
- [ ] **Step 3: Update `docs/follow-ups.md`.** Remove the M3 items now done. Add what came up: Linux S24_3LE, hardware volume in exclusive mode, releasing the exclusive device after a long pause, plus anything the task reviews parked.
- [ ] **Step 4: Draft the fastframe issue.** fastframe-audio v0.4.1 `typed()` calls `scratch.resize(data.len())` inside the render callback, which allocates on the first callback and whenever the device asks for a bigger buffer; it could size the buffer from the stream's buffer at build time. Show the draft to the human and file it (`gh issue create -R crmne/fastframe`) only with their go-ahead.
- [ ] **Step 5: Human acceptance** (batched, on the human's machines, release build; record the results in spec §10/§13):
  - **Windows** (the FiiO E10 is 24/96 at most):
    - In Settings, pick the DAC and turn on Exclusive.
    - A 16/44.1 album: the badge reads `16/44.1 FLAC · Exclusive`, the volume is locked, and there is no gap between the tracks of a continuous (live) album.
    - A 24/96 track: the log has `exclusive output: … 96000 Hz, packed 24-bit`.
    - S1's 24/88.2 track 328638758: one notice, and the badge reads `24/88.2 FLAC · Shared 96 kHz`.
    - Unplug the DAC mid-track: one notice, and playback goes on through the default device. Time
      the latency from the unplug to the `Fallback` notice (at most ~1.5 s expected: the 0.5 s event
      timeout, then the engine's once-a-second check) and record it in spec §10.
    - The first exclusive open after the app starts succeeds (the shared stream is closed first).
    - Exclusive off: the volume works again.
    - Normalization Track vs Off: the level changes audibly.
    - Hi-Res off: the badge shows 16/44.1 on a Hi-Res album.
    - 24/192 bit-perfect needs a DAC that takes 192 kHz; without one, record that limitation.
  - **macOS** (the human's Mac): the same checks with a 24/192 track. Audio MIDI Setup shows 192.0 kHz while it plays, another app cannot play (hog mode), and `cargo test -p player --lib -- --ignored exclusive` passes. After the app quits, check the device's physical format in Audio MIDI Setup (it stays changed after hog mode ends; see follow-ups).
  - **Linux:** on a Linux machine with a `hw:` device, `/proc/asound/card*/pcm0p/sub0/hw_params` shows `rate: 192000`; without one, it is verified in CI only and recorded as such.
- [ ] **Step 6: Commit** `docs: M3 audiophile outcome and follow-ups`.

## Execution notes

- Worktree under `.claude/worktrees/` from local main, as for M1/M2. Subagents run on Opus 5.5; the final whole-branch review runs on Fable 5.1. Then finishing-a-development-branch.
- The order is Tasks 1–3 (independent), then 4, then 5, then 6 and 7 (independent of each other), then 8, 9, 10, then 11.
- Task 7's macOS/Linux code can only be checked in CI, which runs on pushes to `main`, on PRs and on `workflow_dispatch`. The human approved pushing the M3 branch to run CI on it (2026-10-09), and chose Off as the normalization default.
