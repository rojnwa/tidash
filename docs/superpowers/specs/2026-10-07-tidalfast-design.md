# tidalfast — design spec

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
at any time; users can enter a replacement client ID in settings. The name "tidalfast" uses TIDAL's trademark, as
Spotifast uses Spotify's; rename if challenged.

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
| Lossy tiers (LOW/HIGH AAC) | On desktop they are Widevine-protected. Revisit only if spike S1 shows them DRM-free. |
| Music videos | Would require bundling a video stack (libmpv); deferred. |
| Mini player, crossfade, EQ | Deferred past v1 (crossfade breaks bit-perfect anyway). |
| Translations | Strings are wrapped from day one; only English ships in v1. |

## 4. Architecture

```
tidalfast/
  Cargo.toml          # workspace; [patch.crates-io] egui/winit forks required by fastframe
  crates/tidal/       # TIDAL client: PKCE auth, token refresh, typed endpoints, models,
                      # playback info + manifests, play-event reporting. tokio + reqwest. No UI, no audio.
  crates/player/      # audio engine: HTTP stream -> symphonia decode -> ring buffer -> output
                      # (shared | exclusive). Gapless preload. std threads only. No UI, no TIDAL types.
  crates/app/         # binary `tidalfast`: egui on fastframe, Model, queue, settings, integrations
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

**Decode.** symphonia 0.6 FLAC. If S1 shows Hi-Res arrives as DASH, the fragmented-MP4 demuxer reads init + segments
as one continuous byte stream (one adapter, same `Read + Seek` contract). In bit-perfect mode samples stay integers
end to end; 24-bit is placed in a 32-bit container by a lossless shift.

**Output.** `trait Output` with one implementation per backend (multiple real implementations):

| Mode | Backend | Processing |
|---|---|---|
| Shared (default) | cpal 0.18 via fastframe-audio | rubato resample when rates differ, software volume, ReplayGain with peak-based clip prevention |
| Exclusive (Windows) | `wasapi` 0.25, exclusive, event-driven | none |
| Exclusive (macOS) | `coreaudio-rs` 0.14: hog mode + set device nominal rate and physical format | none |
| Exclusive (Linux) | cpal ALSA host on a `hw:` device; the `alsa` crate only if cpal cannot set the exact format (S2 decides) | none |

Exclusive = device opened at the track's rate and bit depth; no resampling, no software volume, no normalization;
volume via hardware endpoint control if available, else fixed 100%. If the device refuses (busy, format unsupported),
fall back to shared and emit a notice.

**Gapless.** ~20 s before the end the next track is preloaded; the decoder appends it to the same ring buffer. Same
format: no gap. Shared mode with a format change: the resampler absorbs it, no gap. Exclusive with a format change:
drain, reopen device at the new format (short gap, inherent to bit-perfect).

**Devices.** Enumerate per backend. Device lost → fall back to system default in shared mode and emit
`PlayerEvent::DeviceLost`.

**Events.** `State(Playing|Paused|Buffering|Stopped)`, `TrackStarted(id)`, `TrackEnded(id)`, `NearEnd(id)` (triggers
preload request), `Format { rate, bits, channels, mode }`, `Error(kind)`, `DeviceLost`, `Notice(text)`.

## 6. `tidal` — service layer

**Auth.** OAuth 2.1 Authorization Code + PKCE (S256) at `login.tidal.com/authorize`, token at
`auth.tidal.com/v1/oauth2/token`, with an official client ID (default baked in, overridable in settings). Redirect
strategy is decided by spike S1, in preference order: loopback listener → custom scheme handed to the running
instance via fastframe-instance → user pastes the redirect URL.

**Tokens.** Stored only in the OS credential store (keyring-core with the native store per OS, as Spotifast does). If
none is available, tokens stay in memory with a visible warning — no plaintext fallback. Refresh is single-flight on
401; all other requests run concurrently (no global client mutex). Concurrency cap ~6 in-flight requests; honor
`Retry-After` on 429.

**Endpoints.** Exactly the set the official desktop client uses (v1 `api.tidal.com/v1`, plus `api.tidal.com/v2` /
`openapi.tidal.com/v2` where the official client uses them): pages (home, explore, artist, album, mix), search,
favorites + ids, playlists CRUD + items + folders, mixes, track/artist radio, lyrics, credits, playbackinfo, user,
event batch.

**Models.** Hand-written serde structs with only the fields we use; unknown fields ignored; enums carry
`#[serde(other)] Unknown` so new values never break parsing.

**Playback info.** Quality preference HI_RES_LOSSLESS → LOSSLESS. Decode BTS (`application/vnd.tidal.bts`) and, only if
S1 requires it, DASH MPD. Classify playback errors (sub-status) as terminal (skip track) or retryable (back off and
retry), following SONE's mapping.

**Caching.** GET JSON responses go to a disk cache served stale-while-revalidate (key = hash of path + query, size-capped
LRU directory). Artwork fetched from `images.tidal.com` at the exact size needed (160/320/640/1280), disk-cached, decoded
off the UI thread, held as textures in an in-memory LRU.

**Play reporting.** Playback-session events sent to TIDAL's event endpoint with a streaming session id, matching the
official desktop client's payloads (derived from the official web bundle during M1).

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
| M2 Library | Home/Explore, artist/playlist/mix pages, collection, favorites, queue panel, caching | Daily-driver browsing |
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

Findings are appended here at the end of M0; any spec change they force is made in the affected section.

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
| Hi-Res arrives Widevine-encrypted for our client ID | S1 answers first; fall back to LOSSLESS 16/44.1 |
| Bit-perfect quirks per DAC/OS | Fallback to shared mode; format badge makes the real path visible |
| Single maintainer burnout (Psst, SONE pattern) | Small files, clear crate boundaries, CI, contributor docs |

## 17. References

- Spotifast / fastframe: architecture model (process/thread split, auth via official client ID, packaging, updater).
- SONE (github.com/lullabyX/sone, GPL-3.0): reference for TIDAL endpoints, playbackinfo handling, error sub-status
  mapping, ALSA hw-params negotiation, ReplayGain selection, play-event format. Reference only, not a fork base
  (Linux-only, GStreamer, WebKitGTK, state in React, global API mutex, plaintext key file).
- Official desktop app 2.43.2 analysis: Electron shell + hosted web client; native player IPC over stdin/stdout JSON;
  lossless via BTS FLAC to a native WASAPI player; lossy and video via Widevine in Chromium.
- TIDAL developer platform: openapi.tidal.com/v2 (JSON:API); third-party tier = 30 s previews only.
