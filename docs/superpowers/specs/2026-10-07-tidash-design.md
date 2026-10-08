# Tidash — design spec

Date: 2026-10-07 · Status: approved in brainstorming, pending written review · License: GPL-3.0-or-later

## 1. Intent

A fast, native, open-source desktop client for TIDAL on Windows, macOS and Linux that replaces the official
Electron app. Modeled on [Spotifast](https://github.com/crmne/spotifast) (native Rust + egui on
[fastframe](https://github.com/crmne/fastframe)).

**Why:** the official app (v2.43.2) is a ~190 MB Electron 43 shell that loads the hosted web client from
`desktop.tidal.com`, plus a separate 32-bit native `TIDALPlayer.exe` (WASAPI + libFLAC) for lossless audio. It is slow,
laggy and buggy. There is no official Linux client.

**Success criteria**

- Feature parity with the official desktop app for everything a listener uses (scope in §3).
- Bit-perfect Hi-Res playback (up to 24/192 FLAC) with exclusive output on all three OSes.
- Meets the performance budget (§10) on every release.
- Shippable: signed-when-possible installers, self-update, CI on all three OSes.

## 2. Decisions and their reasons

| Decision | Reason |
|---|---|
| Rust + egui 0.36 on fastframe (Spotifast model) | No browser engine; fastframe already provides tray, single instance, OS now-playing, signed self-update, desktop-tuned text/scroll, paused-silent audio stream, i18n. Spotifast proves the domain: 11.8 MB installer, 100–250 MB RAM, 0% CPU paused. |
| Rejected: Tauri 2 | Webview floor (~100 MB RAM, startup ≈ Electron); WebKitGTK on Linux is the main failure source in SONE (10–15 fps, 20 GB leak, 100% CPU scrolling). |
| Rejected: Slint | Stable, but every desktop integration fastframe gives us would be ours to write; styling a media UI is laborious; no music-client precedent. |
| Native audio engine, bit-perfect required | Core audience is audiophiles; it's where the official app is weakest (32-bit, WASAPI-only, Windows/macOS-only). |
| Auth with an official TIDAL client ID via PKCE, overridable in settings | The official developer API limits third-party playback to 30 s previews (`FULL_REQUIRES_HIGHER_ACCESS_TIER`); device-code sessions are capped at 320 kbps AAC since Sept 2026. Every working third-party player (SONE, High Tide, hiresTI, python-tidal) does this. |
| Strictly a streaming player | No downloads, no persistent audio cache, no DRM circumvention, no ripping. Takedowns historically targeted downloaders only. |
| Mirror the official desktop client's endpoints | TIDAL can't break them without breaking its own app. |
| Report playback events like the official client | So artists get credited for streams. |
| GPL-3.0-or-later | Norm for full clients in this space; blocks closed forks; allows porting GPL code (e.g. SONE's ALSA format negotiation). All deps (MIT/Apache-2.0/MPL-2.0) are compatible. |
| Three crates (`tidal`, `player`, `app`) | Each has one job and can be tested and replaced independently (Psst's lesson: keep the core separate from the toolkit). |

**Legal stance (README states this plainly):** using an official client ID breaches TIDAL's ToS and can be revoked
at any time; users can enter a replacement client ID in settings. The name "Tidash" (TIDAL plus dash, for fast) plays
on TIDAL's trademark, as Spotifast does on Spotify's; rename if challenged.

## 3. Scope

**v1 includes**

- Browse: login, Home, Explore, Search (with tabs), artist / album / playlist / mix pages, track & artist radio, credits.
- Library: collection (tracks, albums, artists, playlists, mixes), favorites, playlist create/edit/delete, add/remove/reorder tracks, playlist folders.
- Playback: queue (play next, add, reorder, shuffle, repeat), synced lyrics, gapless, ReplayGain normalization
  (track/album/off), Hi-Res FLAC, exclusive/bit-perfect output with device picker, format & output-path badge.
- Desktop: media keys and OS now-playing (SMTC / macOS Now Playing / MPRIS), tray with close-to-tray, single instance,
  `tidal://` links, self-update.
- Extras: Last.fm + ListenBrainz scrobbling, Discord Rich Presence.

**Out of scope** (with reason)

| Feature | Reason |
|---|---|
| TIDAL Connect, Sonos, Chromecast | Proprietary, certificate-protected protocols; SDK only for approved hardware vendors. |
| Dolby Atmos | Not offered on any desktop client by TIDAL. |
| Offline downloads | Mobile-only officially; legal risk. |
| Uploads, marketplace, payouts | Not listening features. |
| Lossy tiers (LOW/HIGH AAC) | DRM-free per S1, but not needed in v1: every TIDAL plan includes lossless. Add a data-saver tier only if users ask. |
| Music videos | Would require bundling a video stack (libmpv); deferred. |
| Mini player, crossfade, EQ | Deferred past v1 (crossfade breaks bit-perfect anyway). |
| Translations | Strings are wrapped from day one; only English ships in v1. |

## 4. Architecture

```
tidash/
  Cargo.toml          # workspace; [patch.crates-io] egui/winit forks required by fastframe
  crates/tidal/       # TIDAL client: PKCE auth, token refresh, typed endpoints, models,
                      # playback info + manifests, play-event reporting. tokio + reqwest. No UI, no audio.
  crates/player/      # audio engine: HTTP stream -> symphonia decode -> ring buffer -> output
                      # (shared | exclusive). Gapless preload. std threads only. No UI, no TIDAL types.
  crates/app/         # binary `tidash`: egui on fastframe, Model, queue, settings, integrations
```

- `player` accepts `Source { url, codec, replaygain }` and never sees TIDAL types; `tidal` never touches audio.
- **Threads**
  - UI (main): egui, event-driven repaint only. While playing, repaint ~4 Hz only if the window is visible. Paused and
    hidden = 0% CPU.
  - Backend: one tokio multi-thread runtime for API, images, cache, scrobbling, Discord.
  - Player: control thread (commands, preload), decoder thread, OS audio callback (real-time: reads a lock-free ring
    buffer only; no allocation, no locks).
- **Data flow**
  - UI → backend: `Action` enum over a channel. Backend → UI: `Event`; the UI thread applies it with
    `Model::apply(event)` and requests a repaint.
  - Player: `PlayerCmd` in, `PlayerEvent` out. Playback position = shared atomic frame counter read by the UI (no
    position events).
  - `Model` is plain data (pages, collection, queue, playback snapshot), mutated only in `Model::apply`. Views read it.
  - Queue and playback state live in Rust (`app::queue`, pure and unit-tested), never in view code.
- **Code hygiene:** one file per view and per integration; soft limit ~800 lines per file. Workspace lints as in
  fastframe (`unsafe_code = "deny"`, allowed per item only with a `SAFETY:` comment; `unwrap_used`, `dbg_macro`, `todo`).

## 5. `player` — audio engine

**Fetch.** A fetcher thread downloads the stream sequentially into a temp file (deleted on drop). symphonia reads it
through a `Read + Seek` adapter that blocks until the requested bytes are present.
`ponytail: sequential fetch, seek-ahead waits for the download; add HTTP Range requests if slow links complain.`

**Decode.** Streams are FLAC in fragmented MP4 delivered as DASH segments (S1), decoded by symphonia 0.6 `flac` +
`isomp4` from one continuous byte stream (init + segments, one adapter, same `Read + Seek` contract). In bit-perfect
mode samples stay integers end to end; 24-bit is placed in a 32-bit container by a lossless shift.

**Output.** `trait Output` with one implementation per backend (multiple real implementations):

| Mode | Backend | Processing |
|---|---|---|
| Shared (default) | cpal 0.18 via fastframe-audio | rubato resample when rates differ, software volume, ReplayGain with peak-based clip prevention |
| Exclusive (Windows) | `wasapi` 0.25, exclusive, event-driven | none |
| Exclusive (macOS) | `coreaudio-rs` 0.14: hog mode + set device nominal rate and physical format | none |
| Exclusive (Linux) | cpal ALSA host on a `hw:` device; the `alsa` crate only if cpal cannot set the exact format (S2 decides) | none |

Exclusive = device opened at the track's rate and bit depth; no resampling, no software volume, no normalization;
volume via hardware endpoint control if available, else fixed 100%. If the device refuses (busy, format unsupported),
fall back to shared and emit a notice. Devices may not support the source rate (e.g. 88.2 kHz on a 24/96 DAC); policy
decided in M3 (current rule: fall back to shared with a notice).

**Gapless.** The next track is pre-opened well before EOF (playbackinfo + init + first segment + decoder; queue sized
for network latency, not ~170 ms); the decoder appends it to the same ring buffer. Same
format: no gap. Shared mode with a format change: the resampler absorbs it, no gap. Exclusive with a format change:
drain, reopen device at the new format (short gap, inherent to bit-perfect).

**Devices.** Enumerate per backend. Device lost → fall back to system default in shared mode and emit
`PlayerEvent::DeviceLost`.

**Events.** `State(Playing|Paused|Buffering|Stopped)`, `TrackStarted(id)`, `TrackEnded(id)`, `NearEnd(id)` (triggers
preload request), `Format { rate, bits, channels, mode }`, `Error(kind)`, `DeviceLost`, `Notice(text)`.

## 6. `tidal` — service layer

**Auth.** OAuth 2.1 Authorization Code + PKCE (S256) at `login.tidal.com/authorize`, token at
`auth.tidal.com/v1/oauth2/token`, with an official client ID (default baked in, overridable in settings): `android`
`6BDSRdpK9hqEBTgU`, redirect `https://tidal.com/android/login/auth` (S1). No client secret for exchange or refresh.
The user pastes the redirect URL back (M1 flow).

**Tokens.** Stored only in the OS credential store (keyring-core with the native store per OS, as Spotifast does). If
none is available, tokens stay in memory with a visible warning — no plaintext fallback. Refresh is single-flight on
401; all other requests run concurrently (no global client mutex). Concurrency cap ~6 in-flight requests; honor
`Retry-After` on 429.

**Endpoints.** Exactly the set the official desktop client uses (S4): Home is the v2 feed
`api.tidal.com/v2/home/feed/static` (cursor pages, a `view-all` per shelf); the artist page is v2 `artist/{id}`;
Explore, genre/mood pages and the mix page are v1 `pages/*` documents; playlists, favorite tracks/albums/artists and
favorite ids are v1; favorite mixes and the playlist list are v2 (`favorites/mixes`, `my-collection/playlists/folders`).
Also search, album, playlist edits, radio, lyrics, credits, playbackinfo, user, event batch. v1 requests add
`countryCode`, `locale=en_US` and `deviceType=DESKTOP`; v2 requests also add `platform=DESKTOP` and the header
`x-tidal-client-version` (required). The public `openapi.tidal.com/v2` API accepts our token and is the fallback if the
internal endpoints break; M2 does not use it.

**Models.** Hand-written serde structs with only the fields we use; unknown fields ignored; enums carry
`#[serde(other)] Unknown` so new values never break parsing.

**Playback info.** Quality preference HI_RES_LOSSLESS → LOSSLESS. Manifest is a DASH MPD (`application/dash+xml`):
`SegmentTemplate` + `SegmentTimeline` → init + media URLs; segment URLs are pre-signed, no auth header. Classify
playback errors (sub-status) as terminal (skip track) or retryable (back off and retry), following SONE's mapping.

**Caching.** GET JSON responses go to a disk cache served stale-while-revalidate (key = hash of path + query, size-capped
LRU directory). Artwork fetched from `resources.tidal.com/images/<uuid with slashes>/<w>x<h>.jpg` (`images.tidal.com`
returned 400) at the exact size needed (160/320/640/1280), disk-cached, decoded off the UI thread, held as textures in
an in-memory LRU.

**Play reporting.** Playback-session events sent to TIDAL's event endpoint with a streaming session id, matching the
official desktop client's payloads (see `docs/spikes/s1-auth-manifests.md` § Play events).

## 7. `app` — UI

- **Layout:** left sidebar (Home, Explore, Search, Collection sections, playlist/folder tree); centre view with
  back/forward history (`Route` stack); bottom player bar (art, title/artist, transport, seek, volume, format badge
  e.g. `24/96 FLAC · Exclusive`); right panel toggling Queue (drag reorder) / Lyrics (synced, auto-scroll,
  click-to-seek) / Credits.
- **Views:** Login, Home/Explore (shelves and grids rendered from TIDAL page modules), Search, Collection, Album,
  Artist, Playlist (edit/reorder), Mix, Settings.
- **Lists and grids** are virtualized (`ScrollArea::show_rows`; grids as rows). Track context menu: play next, add to
  queue, add to playlist, favorite, go to album/artist, track radio, copy link.
- **Keyboard** (Cmd on macOS): space play/pause, Ctrl+→/← next/prev, Ctrl+F search, Alt+←/→ back/forward; shortcut list in Settings.
- **Theme:** dark default plus light (fastframe-theme). Accessibility via AccessKit. All strings through
  fastframe-i18n.
- **Settings:** account (login/logout, client ID override), audio (quality, output device, exclusive, normalization
  mode), behavior (close to tray, start minimized), integrations (Last.fm, ListenBrainz, Discord toggle), updates
  (check, pre-release channel). Stored as JSON in the config dir; secrets in the credential store.

## 8. Integrations

- **Now playing + media keys:** fastframe-now-playing.
- **Tray:** fastframe-tray (play/pause, next, previous, show, quit).
- **Single instance + `tidal://` links:** fastframe-instance; second launches hand their URL to the running instance.
- **Scrobbling:** Last.fm (desktop auth flow with the project's API key) and ListenBrainz (user token). Rule: track
  > 30 s, scrobble after half its length or 4 min, whichever comes first; "now playing" on start. Failed scrobbles
  persisted to disk and retried.
- **Discord Rich Presence:** `discord-rich-presence`; title, artist, album art URL, elapsed time; toggle in settings.
- **Self-update:** fastframe-update with Ed25519-signed `checksums.txt` (publisher key set), using its `Transport`
  trait implemented over our reqwest 0.13 client (no second HTTP stack).

## 9. Error handling

| Failure | Behavior |
|---|---|
| 401 | Single-flight refresh, retry once; refresh fails → login screen, queue preserved |
| 429 / 5xx | Back off (honor `Retry-After`), retry; cached page stays visible |
| Offline | Cached pages shown; banner; playback of the current download continues |
| Playback terminal sub-status | Skip to next with a notice; 3 consecutive failures stop playback |
| Stream interrupted | Re-request from current offset, then retry with backoff |
| Exclusive refused / format unsupported | Fall back to shared, notice |
| Device lost | Fall back to system default, shared mode, notice |
| Credential store unavailable | Memory-only session, warning |
| Client ID revoked | Clear error pointing to the client ID setting |

## 10. Performance budget (measured with a scripted run before each release)

| Metric | Target |
|---|---|
| Cold start to first frame (warm cache) | < 500 ms |
| Idle RAM after Home loaded | < 150 MB |
| CPU paused, window hidden | 0% |
| CPU playing, window minimized | < 1% |
| Installer size | < 20 MB |
| Scrolling a 10 000-track playlist | no dropped frames at display refresh rate |

Release profile: thin LTO, `codegen-units = 1`, `strip`.

M1 on macOS (M2 Pro, release build, idle at the start screen; the app's `first frame` log line and
`footprint`): first frame after 179–213 ms (368 ms on the first launch of a new build), 0% CPU, a
13.1 MB binary, and a physical footprint of 158 MB, already over the RAM budget. About 78 MB of that
is the 24 system fallback faces (CJK, Arabic, Indic, ...) that fastframe-fonts reads whole into
memory; mapping them instead (upstream) would bring it near 80 MB.

M1 on Windows 11 (Ryzen 7 7800X3D, release build, three launches): first frame after 136–145 ms
(window handle after ~80 ms, 435 ms on the first launch of a new build), 0–0.3% CPU idle at the
start screen, a 16.8 MB binary, a 146 MB working set (262 MB private) at the start screen and
162 MB (257 MB private) on an album page after playing a dozen tracks, 0% CPU there while paused.
RAM is over budget on both OSes for the same font reason. Linux: build and tests in CI only.

M2 on Windows (same machine, release build, Home loaded 15 s after launch, three launches each):
before mapping the fallback fonts, a 150 MB working set (256–262 MB private); with `fastframe-fonts`
pinned to crmne/fastframe#17 (fallback faces memory-mapped), 93 MB (202–204 MB private). First frame
121–136 ms and 0–0.3% idle CPU in both. Measured as the working set (Task Manager's Memory column;
private bytes for reference), Windows now meets the 150 MB budget; macOS is to be re-measured with
the pin.

M2 renderer check on Windows (same machine, release build `0b7975e` with both renderers, Home
loaded, 10 s idle, `TIDASH_REPAINT_LOG=1`): glow 0 repaint passes, 0.0% CPU, 94 MB working set;
wgpu on Vulkan 0 egui passes but 100.2% CPU (a full core spinning) and 232 MB; wgpu on DX12 0
passes, 99.8% CPU, 205 MB. Glow is kept and wgpu dropped: wgpu breaks the 0% idle budget on both
backends, costs 111–138 MB more RAM, and adds 6.8 MB to the binary (25.8 MB with both renderers,
19.0 MB with glow alone). PresentMon frame-pacing numbers for glow: pending (M2 acceptance).

## 11. Shipping

- **CI (GitHub Actions):** matrix Windows / macOS / Linux: `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test`, release build.
- **Release on tag:** Windows x64 + ARM64 installer and zip, macOS universal DMG, Linux AppImage / deb / rpm / tar.gz,
  `checksums.txt` + Ed25519 signature. Packaging follows Spotifast's setup so fastframe-update works unchanged.
  Flatpak and AUR after v1.
- **Signing:** macOS notarization (Apple Developer account) and Windows signing (SignPath Foundation for OSS, or Azure
  Trusted Signing) wired as optional CI secrets; unsigned builds until accounts exist.

## 12. Testing

- `tidal`: sanitized recorded JSON fixtures for models, manifest decoding, PKCE, error classification; no network in
  CI. One `#[ignore]` live smoke test run manually with a real account.
- `player`: a capturing `Output` implementation. Tests: bit-perfect output is bit-identical to decoded source;
  gapless output equals the concatenation of two tracks' decoded samples; resample and ReplayGain correctness. Small
  FLAC fixtures (16/44.1, 24/96) committed under `crates/player/tests/fixtures`.
- `app`: queue logic (shuffle, repeat, insert, reorder), scrobble rule, `Model::apply`.
- No UI tests in v1; add when UI regressions appear.

## 13. Milestones

| Milestone | Content | Exit criterion |
|---|---|---|
| M0 Spikes (throwaway code) | S1 auth + manifests, S2 exclusive + gapless, S3 egui grid performance | Findings written into §14; spec updated |
| M1 Walking skeleton | Login, search, album page, lossless shared-mode playback, player bar, media keys | Log in and play an album end to end on all three OSes |
| M2 Library | Home/Explore, artist/playlist/mix pages, collection, favorites, queue panel, caching; frame-pacing check (PresentMon; glow vs wgpu renderer), no idle repaint, display-size textures | Daily-driver browsing |
| M3 Audiophile | Exclusive per OS, device picker, gapless, ReplayGain, Hi-Res, format badge | Bit-perfect 24/192 verified on each OS |
| M4 Extras | Playlist editing/folders/reorder, radio, lyrics, credits, scrobbling, Discord, tray, settings | v1 scope complete |
| M5 Ship | Packaging, updater, signing, performance budget, docs | v1.0.0 released |

Implementation plans are written one milestone at a time; the first plan covers M0 + M1.

## 14. Spikes

| Spike | Question | Done when |
|---|---|---|
| S1 Auth + manifests | Which official client ID + redirect works with PKCE without a client secret? What manifest types (BTS single FLAC vs DASH) and encryption come back for LOSSLESS and HI_RES_LOSSLESS? Are lossy tiers DRM-free? | Written answer + a working token and a decodable Hi-Res stream URL |
| S2 Audio | symphonia decode from a progressive temp file; WASAPI exclusive at 24/96 and 24/192 via `wasapi`; gapless append between two FLACs | Bit-identical capture and audible gapless on Windows; notes for macOS/Linux |
| S3 UI | egui virtualized grid of 10 000 covers at 320 px with async decode | RAM and frame-time numbers recorded |
| S4 Library (M2) | Which endpoints and response shapes do Home, Explore, artist, playlist, mix and the collection use, with which page sizes and cache headers? | Live recordings, sanitized fixtures, written answers |

**Findings (M0, 2026-10-07; S4, 2026-10-08)**

| Spike | Result |
|---|---|
| [S1](../../spikes/s1-auth-manifests.md) | Client `android` `6BDSRdpK9hqEBTgU` via PKCE, redirect `https://tidal.com/android/login/auth`; no client secret for exchange or refresh; refresh returns no new `refresh_token` (keep the old one). Every quality (LOW to HI_RES_LOSSLESS) is unencrypted DASH, FLAC in fMP4 for lossless; lossy tiers are DRM-free too. Images: `resources.tidal.com`. Play events: `POST ec.tidal.com/api/event-batch`. |
| [S2](../../spikes/s2-audio.md) | WASAPI exclusive works on the test DAC (24/96 USB, accepts 24-bit packed at 44.1/48/96 kHz only). Gapless a→b: 0 underruns, output sample-equal to the source. fMP4 FLAC decodes bit-exact via symphonia, but `num_frames = 0` (duration from the MPD) and `tfdt` is ignored (add segment start to pts). Pre-open the next track before EOF; the probe's 118–154 µs open was a warm local file. Unsupported source rates exist (88.2 kHz on a 24/96 DAC) → M3 decision. |
| [S3](../../spikes/s3-grid.md) | 10 000-cell grid: working set 90–114 MB, app CPU ~0.3 ms/frame; 144 Hz pacing not demonstrated (stalls up to 59 ms, mostly in frames with 0 uploads; cause not isolated, driver/DWM suspected); idle repaints from the egui scrollbar fade while the pointer hovers; 200 px display-size textures save ~60 MB. Verdict: keep egui. |
| [S4](../../spikes/s4-library-endpoints.md) | All probed endpoints answer the android token. Home = v2 feed (cursor; 10 + 8 modules), artist = v2 `artist/{id}` (discography as separate shelves, no tabs seen), Explore/genre/mix = v1 `pages/*`. v2 rejects requests without `x-tidal-client-version`; `DESKTOP` and `BROWSER`/`WEB` return the same. Page caps: playlist items 100, v1 `pages/data` 50, v2 folders 50; favorites pages run short mid-list (unavailable items dropped), so offsets step by `limit` up to the total. v1 pages, playlists and favorites honour `If-None-Match` (304); v2 home/artist are `no-store` with no ETag. Favorited playlists are `itemType: PLAYLIST` in the flattened list. `openapi.tidal.com/v2` accepts the token. |

**Gate decisions (human, 2026-10-07)**

- LOSSLESS/HI_RES_LOSSLESS arrive as unencrypted MPEG-DASH (FLAC in fMP4), not BTS: approved; the plan is amended (DASH segment URLs, fMP4 decoding); continue to M1.
- 144 Hz frame pacing not demonstrated in S3: keep egui; M2 gets a task to measure the real grid with PresentMon and compare eframe's glow vs wgpu renderer, picking the one that paces; also no repaint while idle and textures uploaded at display size.
  M2 outcome (2026-10-09): glow kept, wgpu dropped. Idle on Home, wgpu spun a full core (99.8–100.2%
  CPU on DX12 and Vulkan, glow 0.0%) and used 111–138 MB more RAM (§10). PresentMon frame-pacing
  numbers for glow: pending (M2 acceptance).

## 15. Dependencies (latest stable, verified on crates.io 2026-10-07)

| Crate | Version | Use |
|---|---|---|
| eframe / egui / egui_extras | 0.36.2 | UI (glow renderer, accesskit) |
| fastframe-* | git tag v0.4.1 | text, fonts, icons, theme, i18n, log, tray, shell, instance, now-playing, update, audio, scroll |
| symphonia | 0.6.1 | FLAC decode, MP4 demux |
| cpal | 0.18.2 | shared output (via fastframe-audio), Linux ALSA hw |
| wasapi | 0.25.0 | Windows exclusive |
| coreaudio-rs | 0.14.2 | macOS hog mode / device format |
| alsa | 0.12.1 | Linux hw params if cpal falls short |
| rubato | 5.0.1 | resampling (shared mode) |
| rtrb | 0.4.0 | lock-free ring buffer |
| reqwest | 0.13.5 | HTTP (rustls) |
| tokio | 1.53 | async runtime |
| keyring-core + native stores | 1.x | credential storage |
| serde / serde_json | 1.x | models |
| roxmltree | 0.21 | DASH MPD parsing |
| sha2 / base64 / rand | 0.11 / 0.23 / 0.10 | PKCE |
| discord-rich-presence | 1.1.0 | Discord |
| image | 0.25.10 | artwork decode (jpeg/png) |
| thiserror / log | 2.x / 0.4 | errors / logging (fastframe-log) |

Rust edition 2024, rust-version 1.98 (fastframe's minimum).

## 16. Risks

| Risk | Mitigation |
|---|---|
| Client ID revoked / ToS enforcement | Overridable client ID; mirror official endpoints; streaming-only scope |
| TIDAL API changes | `tidal` crate isolated; tolerant models; fixtures make breakage visible |
| fastframe young, egui/winit forks | Pin tag; crates are MIT and small — vendor the ones we use if abandoned |
| TIDAL starts encrypting streams or revokes the android client | Overridable client ID; the `tidal` crate isolates playback-info parsing; fixtures make a format change visible |
| Device cannot play the source rate (e.g. 88.2 kHz on a 24/96 DAC) | Shared-mode fallback with a notice; policy refined in M3 |
| Bit-perfect quirks per DAC/OS | Fallback to shared mode; format badge makes the real path visible |
| Single maintainer burnout (Psst, SONE pattern) | Small files, clear crate boundaries, CI, contributor docs |

## 17. References

- Spotifast / fastframe: architecture model (process/thread split, auth via official client ID, packaging, updater).
- SONE (github.com/lullabyX/sone, GPL-3.0): reference for TIDAL endpoints, playbackinfo handling, error sub-status
  mapping, ALSA hw-params negotiation, ReplayGain selection, play-event format. Reference only, not a fork base
  (Linux-only, GStreamer, WebKitGTK, state in React, global API mutex, plaintext key file).
- Official desktop app 2.43.2 analysis: Electron shell + hosted web client; native player IPC over stdin/stdout JSON;
  lossless via BTS FLAC to a native WASAPI player (official app; our client receives DASH, see §6); lossy and video via Widevine in Chromium.
- TIDAL developer platform: openapi.tidal.com/v2 (JSON:API); third-party tier = 30 s previews only.
