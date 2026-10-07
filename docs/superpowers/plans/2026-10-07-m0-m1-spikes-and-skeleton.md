# M0 + M1: Spikes and Walking Skeleton — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Answer the three spike questions, then ship a native app that logs in to TIDAL, searches, opens an album and
plays it losslessly in shared mode with a player bar and media keys.

**Architecture:** Cargo workspace with three crates: `tidal` (async service client), `player` (std-thread audio
engine fed by any `Read` stream), `app` (egui on fastframe). The app's tokio backend owns the TIDAL client, the queue
and the player handle; the UI exchanges `Action`/`Event` messages with it and renders a plain-data `Model`.

**Tech Stack:** Rust 1.98 (edition 2024), egui/eframe 0.36 + fastframe v0.4.1, symphonia 0.6, rubato 5, rtrb 0.4,
reqwest 0.13 (rustls), tokio 1.53, keyring-core 1, wiremock 0.6 (tests).

**Spec:** `docs/superpowers/specs/2026-10-07-tidalfast-design.md`

## Global Constraints

- License `GPL-3.0-or-later` in every `Cargo.toml`; edition 2024; `rust-version = "1.98"`.
- Dependency versions as in spec §15. fastframe crates all from `git = "https://github.com/crmne/fastframe", tag = "v0.4.1"`.
- Root `Cargo.toml` carries fastframe's `[patch.crates-io]` block verbatim (egui rev `ba6790fe7cf46e58e8d27ce1524cbfdee745e938`, winit rev `ed7caa9023f10b397f5b6ec8284a840cbd8a6f65`); list only the egui crates we use.
- TLS is rustls only; no OpenSSL anywhere.
- Tokens live only in the OS credential store or in memory. Never written to a plain file, never logged.
- `player` depends on no tokio, no HTTP client, no `tidal` types. `tidal` depends on no UI or audio crate.
- Workspace lints: `unsafe_code = "deny"`, clippy `all`, `unwrap_used`, `dbg_macro`, `todo`, `print_stdout`, `print_stderr` = warn; CI runs clippy with `-D warnings`.
- Tests never touch the network, except `#[ignore]` live tests.
- Soft limit ~800 lines per file; one file per view.
- Look up every library API with ctx7 before using it (`npx ctx7@latest library <name> "<topic>"`): symphonia 0.6, rubato 5, cpal 0.18 and egui 0.36 differ from older examples.
- All user-visible UI strings go through fastframe-i18n; English only.
- `tidal` and `player` re-export every public type named in an Interfaces block from the crate root (e.g. `tidal::Track`, `player::PlayState`).

## Review Focus

1. **Network drops mid-track:** the stream reader errors after some bytes. Expected: the player emits `PlayerEvent::Error` for that track and stops; it never hangs. (Task 13, Task 16)
2. **Output device reopens at another rate mid-track** (default device changed, unplugged). Expected: playback continues at the right pitch; position stays correct. (Task 16)
3. **Seek edge cases:** seek while paused, seek past the end. Expected: no stale audio after resume; seeking past the end ends the track promptly. (Task 16)
4. **Stored session whose refresh token was revoked:** expected login screen, exactly one refresh attempt, no loop. (Task 9)
5. **Albums with more than one page of items or unavailable tracks:** expected all tracks listed; unavailable (`stream_ready == false`) tracks skipped by the queue. (Task 10, Task 19)

---

# M0 — Spikes (throwaway code)

Spike code lives in `spikes/<name>/`, each a standalone project with its own manifest (Rust spikes declare an empty
`[workspace]` table). Spikes have no TDD; each ends with a findings document. Task 4 deletes `spikes/`.

### Task 1: S1 — Auth and manifests

**Requires the human partner:** a TIDAL account with an active subscription, and a browser login.

**Files:**
- Create: `spikes/s1-auth/probe.py` (run with `uv run --with httpx spikes/s1-auth/probe.py ...`)
- Create: `spikes/s1-auth/.gitignore` containing `.secrets/`
- Create: `docs/spikes/s1-auth-manifests.md`
- Create: `crates/tidal/tests/fixtures/{token.json, refresh.json, search.json, album.json, album_items_page1.json, album_items_page2.json, playbackinfo_lossless.json, playbackinfo_hires.json, playbackinfo_error_4005.json}`

**Interfaces:**
- Produces: the findings document, which later tasks read for exact values. It must contain these headings: `## Client`, `## Authorize request`, `## Token response`, `## Refresh`, `## Manifests`, `## Images`, `## Play events`.

- [ ] **Step 1: Collect candidate client IDs.** Sources: the official web client bundle served by `https://desktop.tidal.com/` (look for the PKCE `client_id`, `appMode`, redirect route `login/auth`); SONE's `src-tauri/src/embedded_config.rs` + `scripts/gen_credentials.py`; python-tidal's `tidalapi/session.py`. Record each candidate's client ID, redirect URI and whether a secret exists.

- [ ] **Step 2: Write `probe.py` with subcommands** `login <candidate>` (PKCE S256, print and open the authorize URL, read the pasted redirect URL, exchange the code, save the token JSON to `.secrets/<candidate>.json`), `refresh <candidate>` (refresh without a client secret), `info <candidate> <track_id>` (call both `/v1/tracks/{id}/playbackinfo` and `/v1/tracks/{id}/playbackinfopostpaywall` with `playbackmode=STREAM&assetpresentation=FULL` for LOW, HIGH, LOSSLESS and HI_RES_LOSSLESS; print `manifestMimeType`, the decoded manifest's `mimeType`, `codecs`, `encryptionType`, plus `bitDepth`, `sampleRate`), `fixtures <candidate>` (save the fixture files listed above for a known multi-disc album with more than 50 tracks, so its items span two pages at `limit=50`).

- [ ] **Step 3: Run** `login`, `refresh` and `info` for each candidate. For one LOSSLESS and one HI_RES_LOSSLESS track, download the first 1 MB of the stream URL and confirm `ffprobe` identifies FLAC with the expected rate and bit depth.

- [ ] **Step 4: Derive the play-event payload** the official client sends (event-batch endpoint, from the web bundle; cross-check SONE's `src-tauri/src/tidal_report/`). Document endpoint, method, headers, JSON shape and when each event is sent.

- [ ] **Step 5: Run `fixtures`, then sanitize every fixture:** replace tokens with `"ACCESS"`/`"REFRESH"`, user ids with `12345`, emails and names with placeholders; stream URLs with `https://example.invalid/stream.flac`. Grep the fixtures for the real token prefix and your email; expect no match.

- [ ] **Step 6: Write `docs/spikes/s1-auth-manifests.md`** with the chosen client (one that returns full-length LOSSLESS without a client secret; prefer the one that also returns HI_RES_LOSSLESS DRM-free), its authorize parameters, the token/refresh response shapes (does refresh return a new refresh token? the `user` object?), the manifest type and encryption per quality, the image URL format (`resources.tidal.com/images/<id-with-slashes>/<w>x<h>.jpg` vs `images.tidal.com`), and the play-event format.

- [ ] **Step 7: Commit**

```bash
git add spikes/s1-auth docs/spikes/s1-auth-manifests.md crates/tidal/tests/fixtures
git commit -m "spike(s1): tidal auth and manifest findings"
```

### Task 2: S2 — Exclusive output and gapless

**Files:**
- Create: `spikes/s2-audio/Cargo.toml` (`symphonia` 0.6.1 with `flac`, `wasapi` 0.25.0, empty `[workspace]`), `spikes/s2-audio/src/main.rs`
- Create: `docs/spikes/s2-audio.md`

- [ ] **Step 1: Generate test audio** (one continuous 1 kHz sine split mid-cycle, so any gap clicks):

```bash
mkdir -p spikes/s2-audio/media && cd spikes/s2-audio/media
ffmpeg -f lavfi -i "sine=frequency=1000:sample_rate=96000:duration=6" -ac 2 -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 full.flac
ffmpeg -i full.flac -af "atrim=end_sample=288037" -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 a.flac
ffmpeg -i full.flac -af "atrim=start_sample=288037,asetpts=N/SR/TB" -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 b.flac
```

- [ ] **Step 2: Probe:** decode `a.flac` then `b.flac` with symphonia into one WASAPI exclusive, event-driven stream opened at exactly the files' format (96 kHz, 24-in-32 or 24-bit packed, whichever the device accepts without conversion). Print the negotiated format. Repeat with a 192 kHz pair if the device supports it.

- [ ] **Step 3: Verify:** listen through the a→b boundary (no click), and assert in code that the concatenated samples written to WASAPI equal `full.flac` decoded (count and values). Note which formats the device refused.

- [ ] **Step 4: Desk-research notes** for macOS (coreaudio-rs 0.14 hog mode + nominal rate + physical format) and Linux (cpal 0.18 ALSA host on `hw:` with an exact format) — what call sequence each needs. Write all results to `docs/spikes/s2-audio.md`.

- [ ] **Step 5: Commit** — `git add spikes/s2-audio docs/spikes/s2-audio.md && git commit -m "spike(s2): exclusive output and gapless findings"` (do not commit `media/`; add it to `spikes/s2-audio/.gitignore`).

### Task 3: S3 — egui grid performance

**Files:**
- Create: `spikes/s3-grid/Cargo.toml` (eframe 0.36 glow + fastframe's `[patch.crates-io]` block, `image` 0.25 jpeg, empty `[workspace]`), `spikes/s3-grid/src/main.rs`
- Create: `docs/spikes/s3-grid.md`

- [ ] **Step 1: Probe:** a virtualized grid of 10 000 cells (cell 200 px wide, image 320×320 JPEG generated in memory per index), decoded on a worker pool only for visible rows (+1 row ahead), textures held in an LRU capped at 300 entries. Overlay p50/p99 frame time from `ctx.input(|i| i.unstable_dt)` over the last 600 frames and process RSS.

- [ ] **Step 2: Measure** in a release build: idle RSS with the first screen loaded, RSS after scrolling end-to-end, p99 frame time while fling-scrolling, CPU when idle (expect 0% with no repaint requests). Record the numbers and the display refresh rate in `docs/spikes/s3-grid.md`.

- [ ] **Step 3: Commit** — `git add spikes/s3-grid docs/spikes/s3-grid.md && git commit -m "spike(s3): egui grid performance findings"`

### Task 4: M0 wrap-up and gate

**Files:**
- Modify: `docs/superpowers/specs/2026-10-07-tidalfast-design.md` (§14 findings; any section a finding changes)
- Delete: `spikes/`

- [ ] **Step 1: Append findings** to spec §14: chosen client and redirect, manifest per quality, image host, play-event summary, exclusive/gapless results, grid numbers.
- [ ] **Step 2: Gate.** Stop and consult the human partner if any is true: no client returns full-length LOSSLESS; LOSSLESS is not a BTS manifest with `encryptionType` `NONE` (Tasks 11 and 16 assume it); S3 p99 frame time exceeds the refresh interval or idle RSS exceeds 150 MB.
- [ ] **Step 3: Delete** `spikes/` from disk entirely, including untracked `.secrets/` token files, `media/` and `target/` directories, then commit — `git rm -r spikes && git add docs && git commit -m "docs: record M0 spike findings"`

---

# M1 — Walking skeleton

### Task 5: Workspace, license, CI

**Files:**
- Create: `Cargo.toml` (workspace: `members = ["crates/*"]`, `resolver = "3"`, `[workspace.package]`, `[workspace.lints]`, `[workspace.dependencies]` for shared crates, `[patch.crates-io]`, `[profile.release]` with `lto = "thin"`, `codegen-units = 1`, `strip = true`)
- Create: `crates/tidal/Cargo.toml`, `crates/tidal/src/lib.rs`; `crates/player/Cargo.toml`, `crates/player/src/lib.rs`; `crates/app/Cargo.toml` (package and bin name `tidalfast`), `crates/app/src/main.rs` (opens an empty eframe window titled "tidalfast")
- Create: `LICENSE` (verbatim from https://www.gnu.org/licenses/gpl-3.0.txt), `README.md`, `.gitignore` (`/target`), `.github/workflows/ci.yml`

- [ ] **Step 1: Write the files.** `README.md`: one-paragraph purpose, the legal stance from spec §2 verbatim, build instructions (`cargo run -p tidalfast`), Linux build deps. CI: jobs `quality` (ubuntu: `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`) and `test` (matrix `ubuntu-latest`, `windows-latest`, `macos-latest`: `cargo test --locked --all-targets`), using `actions/checkout@v7`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`; Linux step `sudo apt-get install -y libasound2-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev`.
- [ ] **Step 2: Verify:** `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test` → all succeed; `cargo run -p tidalfast` opens an empty window.
- [ ] **Step 3: Commit** — `git add -A && git commit -m "chore: workspace, license, CI"`

### Task 6: `tidal` — config, errors, PKCE

**Files:**
- Create: `crates/tidal/src/config.rs`, `crates/tidal/src/error.rs`, `crates/tidal/src/auth.rs`
- Modify: `crates/tidal/src/lib.rs` (modules, re-exports)
- Test: `crates/tidal/src/auth.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub struct ClientConfig { pub client_id: String, pub client_secret: Option<String>, pub redirect_uri: String, pub scopes: String, pub api_base: url::Url, pub auth_base: url::Url, pub login_base: url::Url, pub image_base: url::Url }` with `Default` filled from `docs/spikes/s1-auth-manifests.md`.
  - `pub enum Error { Http(reqwest::Error), Status { status: u16, sub_status: Option<u32>, message: String }, Auth(String), Json(serde_json::Error), Manifest(String), Store(String) }` (thiserror) and `pub type Result<T> = std::result::Result<T, Error>`.
  - `pub struct Pkce { pub verifier: String, pub challenge: String, pub state: String }` with `Pkce::new() -> Pkce` (random 32-byte verifier, base64url no padding) and `Pkce::from_parts(verifier: &str, state: &str) -> Pkce`.
  - `pub fn authorize_url(cfg: &ClientConfig, pkce: &Pkce) -> url::Url`
  - `pub fn code_from_redirect(redirect: &str, expected_state: &str) -> Result<String>`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn challenge_matches_rfc7636_vector() {
    let p = Pkce::from_parts("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", "s");
    assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}
#[test]
fn authorize_url_carries_pkce_params() {
    let cfg = ClientConfig::default();
    let p = Pkce::from_parts("v", "st");
    let q: HashMap<_, _> = authorize_url(&cfg, &p).query_pairs().into_owned().collect();
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["code_challenge"], p.challenge);
    assert_eq!(q["state"], "st");
    assert_eq!(q["client_id"], cfg.client_id);
    assert_eq!(q["redirect_uri"], cfg.redirect_uri);
}
#[test]
fn redirect_code_extracted_and_state_checked() {
    assert_eq!(code_from_redirect("https://x.test/cb?code=abc&state=st", "st").unwrap(), "abc");
    assert!(matches!(code_from_redirect("https://x.test/cb?code=abc&state=evil", "st"), Err(Error::Auth(_))));
    assert!(matches!(code_from_redirect("https://x.test/cb?error=access_denied&state=st", "st"), Err(Error::Auth(_))));
    assert!(matches!(code_from_redirect("not a url", "st"), Err(Error::Auth(_))));
}
#[test]
fn verifiers_are_unique_and_long_enough() {
    let (a, b) = (Pkce::new(), Pkce::new());
    assert_ne!(a.verifier, b.verifier);
    assert!((43..=128).contains(&a.verifier.len()));
}
```

- [ ] **Step 2: Run** `cargo test -p tidal auth` → FAIL (items not defined).
- [ ] **Step 3: Implement** in the files above. Extra authorize params required by the chosen client (e.g. `appMode`) come from the findings doc. `code_from_redirect` trims whitespace (users paste with newlines).
- [ ] **Step 4: Run** `cargo test -p tidal auth` → PASS.
- [ ] **Step 5: Commit** — `git add crates/tidal && git commit -m "feat(tidal): config, errors, PKCE"`

### Task 7: `tidal` — token exchange and refresh

**Files:**
- Modify: `crates/tidal/src/auth.rs`
- Test: `crates/tidal/tests/auth_http.rs` (wiremock; fixtures `token.json`, `refresh.json`)

**Interfaces:**
- Consumes: `ClientConfig`, `Pkce`, `Error` (Task 6).
- Produces:
  - `#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)] pub struct Session { pub access_token: String, pub refresh_token: String, pub expires_at: u64 /* unix seconds */, pub user_id: u64, pub country_code: String }`
  - `pub async fn exchange_code(http: &reqwest::Client, cfg: &ClientConfig, code: &str, pkce: &Pkce) -> Result<Session>`
  - `pub async fn refresh(http: &reqwest::Client, cfg: &ClientConfig, old: &Session) -> Result<Session>`

- [ ] **Step 1: Write failing tests** (each mounts a wiremock server and points `cfg.auth_base` at it):
  - `exchange_posts_form_and_parses_session`: matcher on `POST /v1/oauth2/token` with body containing `grant_type=authorization_code`, `code=abc`, `code_verifier=<pkce.verifier>`, `client_id=<cfg.client_id>`; responds `token.json`; asserts `session.access_token == "ACCESS"`, `refresh_token == "REFRESH"`, `user_id == 12345`, `country_code` equals the fixture's value, `expires_at` ≈ now + `expires_in` (±5 s).
  - `refresh_keeps_old_refresh_token_and_user_when_absent`: responds `refresh.json`; asserts new `access_token`, and `refresh_token`, `user_id`, `country_code` equal the old session's when the response lacks them.
  - `invalid_grant_is_auth_error`: responds 400 `{"error":"invalid_grant"}`; asserts `Err(Error::Auth(_))`.
- [ ] **Step 2: Run** `cargo test -p tidal --test auth_http` → FAIL.
- [ ] **Step 3: Implement.** Form-encoded POSTs; include `client_secret` only when `Some`; scope from config.
- [ ] **Step 4: Run** `cargo test -p tidal --test auth_http` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(tidal): token exchange and refresh"` (add the new test file first).

### Task 8: `tidal` — token store

**Files:**
- Create: `crates/tidal/src/store.rs`
- Modify: `crates/tidal/Cargo.toml` (`keyring-core = "1"`; target deps `windows-native-keyring-store = "1.1"`, `apple-native-keyring-store = { version = "1.0.2", features = ["keychain"] }`, `zbus-secret-service-keyring-store = { version = "1.0.1", features = ["rt-tokio-crypto-rust"] }`)

**Interfaces:**
- Consumes: `Session`, `Error::Store`.
- Produces:
  - `pub trait TokenStore: Send + Sync { fn load(&self) -> Result<Option<Session>>; fn save(&self, s: &Session) -> Result<()>; fn clear(&self) -> Result<()>; }`
  - `pub struct KeyringStore` (service `"tidalfast"`, user `"session"`, value = Session JSON) and `pub struct MemoryStore` (`Mutex<Option<Session>>`).
  - `pub fn default_store() -> (std::sync::Arc<dyn TokenStore>, bool)` — the keyring store if a probe `load()` succeeds, else `MemoryStore`; the bool says "persistent".

- [ ] **Step 1: Write failing tests:** `memory_store_round_trip` (save → load equals, clear → `None`); `#[ignore] keyring_store_round_trip` (same against the OS store with service `"tidalfast-test"`).
- [ ] **Step 2: Run** `cargo test -p tidal store` → FAIL.
- [ ] **Step 3: Implement.** Register the platform's native store at first use as keyring-core requires (see its docs via ctx7). A missing entry is `Ok(None)`, not an error.
- [ ] **Step 4: Run** `cargo test -p tidal store` → PASS; run `cargo test -p tidal store -- --ignored` locally on Windows → PASS.
- [ ] **Step 5: Commit** — `git add crates/tidal && git commit -m "feat(tidal): credential store"`

### Task 9: `tidal` — authenticated client

**Files:**
- Create: `crates/tidal/src/client.rs`
- Test: `crates/tidal/tests/client_http.rs`

**Interfaces:**
- Consumes: `ClientConfig`, `Session`, `refresh`, `TokenStore`, `Error`.
- Produces:
  - `pub struct Client` (`Send + Sync`, used behind `Arc`)
  - `pub fn Client::new(cfg: ClientConfig, session: Session, store: Arc<dyn TokenStore>) -> Result<Client>`
  - `pub fn Client::session(&self) -> Session`
  - `pub fn Client::config(&self) -> &ClientConfig`
  - `pub async fn Client::get<T: serde::de::DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T>` — `path` is relative to `cfg.api_base`; adds `countryCode=<session.country_code>` and `Authorization: Bearer`.
  - `pub(crate) async fn Client::send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response>` for non-GET calls in later tasks, with the same auth/retry rules.

Rules (pinned; not determined by the signature):
- At most 6 requests in flight (`tokio::sync::Semaphore`).
- 401: if the body's `subStatus` (integer or whole float) is within 4000–4999, return `Error::Status` without refreshing. Otherwise refresh once, single-flight: take the refresh mutex; if the current access token differs from the one the failed request used, another task already refreshed — just retry. After refreshing, `store.save` the new session. A second 401 → `Error::Auth`. A failed refresh → `Error::Auth`.
- 429: wait `Retry-After` seconds (default 2, cap 30), at most 2 retries. 5xx: one retry after 1 s.
- Other non-2xx → `Error::Status { status, sub_status, message }` from the body (`userMessage` field if present).

- [ ] **Step 1: Write failing tests** (wiremock; `api_base` and `auth_base` both point to the mock):
  - `get_adds_country_and_bearer`: asserts the query has `countryCode` and header `authorization: Bearer ACCESS`.
  - `expired_token_refreshes_once_and_retries`: first GET → 401 `{"status":401,"subStatus":11003}`, refresh → 200, second GET → 200; asserts token endpoint hit exactly once and the store holds the new token.
  - `concurrent_401s_share_one_refresh`: 5 parallel `get`s all first see 401; asserts exactly 1 refresh call and 5 successes.
  - `playback_substatus_401_does_not_refresh`: 401 `{"status":401,"subStatus":4005}` → `Err(Error::Status { sub_status: Some(4005), .. })`, 0 refresh calls.
  - `revoked_refresh_token_is_auth_error_once`: GET 401, refresh 400 `invalid_grant` → `Err(Error::Auth(_))`, exactly 1 refresh call. (Review Focus 4)
  - `retry_after_is_honored`: 429 with `Retry-After: 1` then 200 → success after ≥ 1 s.
- [ ] **Step 2: Run** `cargo test -p tidal --test client_http` → FAIL.
- [ ] **Step 3: Implement** `client.rs` per the rules.
- [ ] **Step 4: Run** `cargo test -p tidal --test client_http` → PASS.
- [ ] **Step 5: Commit** — `git add crates/tidal && git commit -m "feat(tidal): authenticated client with single-flight refresh"`

### Task 10: `tidal` — catalog models and endpoints

**Files:**
- Create: `crates/tidal/src/catalog.rs`
- Test: `crates/tidal/tests/catalog_http.rs` (fixtures `search.json`, `album.json`, `album_items_page1.json`, `album_items_page2.json`)

**Interfaces:**
- Consumes: `Client::get`.
- Produces (all `#[derive(Clone, Debug, Deserialize)]`, camelCase, unknown fields ignored):
  - `pub struct Artist { pub id: u64, pub name: String }`
  - `pub struct AlbumRef { pub id: u64, pub title: String, pub cover: Option<String> }`
  - `pub struct Album { pub id: u64, pub title: String, pub cover: Option<String>, pub artists: Vec<Artist>, pub number_of_tracks: u32, pub duration: u32, pub release_date: Option<String> }`
  - `pub struct Track { pub id: u64, pub title: String, pub duration: u32, pub track_number: u32, pub volume_number: u32, pub artists: Vec<Artist>, pub album: AlbumRef, pub stream_ready: bool }`
  - `pub struct SearchResults { pub artists: Vec<Artist>, pub albums: Vec<Album>, pub tracks: Vec<Track> }`
  - `pub async fn Client::search(&self, query: &str, limit: u32) -> Result<SearchResults>` — `GET search` with `types=ARTISTS,ALBUMS,TRACKS`.
  - `pub async fn Client::album(&self, id: u64) -> Result<Album>` — `GET albums/{id}`.
  - `pub async fn Client::album_tracks(&self, id: u64) -> Result<Vec<Track>>` — `GET albums/{id}/items` with `limit=50&offset=n` until `offset >= totalNumberOfItems`; keeps only `type == "track"` items.
  - `pub fn image_url(cfg: &ClientConfig, cover: &str, size: u32) -> String` — format per the findings doc (`## Images`).

- [ ] **Step 1: Write failing tests:**
  - `search_parses_fixture`: non-empty `albums` and `tracks`; first track has `id > 0` and a non-empty `title`.
  - `search_query_is_url_encoded`: query `"AC/DC & Björk"` arrives at the mock decoded intact.
  - `album_tracks_paginates_until_total`: page 1 (`offset=0`) and page 2 (`offset=50`) mocked; asserts result length equals the fixture's `totalNumberOfItems` and ordering by (`volume_number`, `track_number`) is preserved. (Review Focus 5)
  - `image_url_format`: for cover `"ab12-cd34"` and size 320 the exact string from the findings doc.
- [ ] **Step 2: Run** `cargo test -p tidal --test catalog_http` → FAIL.
- [ ] **Step 3: Implement** `catalog.rs`.
- [ ] **Step 4: Run** `cargo test -p tidal --test catalog_http` → PASS.
- [ ] **Step 5: Commit** — `git add crates/tidal && git commit -m "feat(tidal): search, album, album tracks"`

### Task 11: `tidal` — playback info and failure classification

**Files:**
- Create: `crates/tidal/src/playback.rs`
- Modify: `crates/tidal/Cargo.toml` (`roxmltree = "0.21"`; `base64` already present)
- Test: `crates/tidal/tests/playback_http.rs` (fixtures `playbackinfo_lossless.json`, `playbackinfo_hires.json`, `playbackinfo_error_4005.json`)

_Amended after M0 (human-approved): S1 found every quality is an unencrypted MPEG-DASH manifest (`application/dash+xml`, FLAC in fragmented MP4), not BTS. BTS support is dropped._

**Interfaces:**
- Consumes: `Client::get`, `Error`.
- Produces:
  - `pub enum Quality { Lossless, HiResLossless }` → `"LOSSLESS"`, `"HI_RES_LOSSLESS"`.
  - `#[derive(Clone, Debug)] pub struct StreamInfo { pub track_id: u64, pub urls: Vec<String> /* init segment first, then media segments in order */, pub codec: String, pub quality: String, pub bit_depth: Option<u16>, pub sample_rate: Option<u32>, pub track_gain: Option<f32>, pub track_peak: Option<f32>, pub album_gain: Option<f32>, pub album_peak: Option<f32> }`
  - `pub fn dash_segments(manifest_b64: &str) -> Result<(String /*codec*/, Vec<String> /*urls*/)>` — base64-decode, parse with roxmltree; any `ContentProtection` element → `Error::Manifest`; from the first `Representation` take `codecs`; from its `SegmentTemplate` take `initialization`, `media`, `startNumber` (default 1); segment count = Σ over `SegmentTimeline/S` of (1 + `r`, default 0); urls = [initialization] + `media` with `$Number$` replaced by `startNumber ..= startNumber + count - 1`.
  - `pub async fn Client::stream_info(&self, track_id: u64, quality: Quality) -> Result<StreamInfo>` — `GET tracks/{id}/playbackinfo` with `audioquality`, `playbackmode=STREAM&assetpresentation=FULL`; `manifestMimeType` other than `application/dash+xml` → `Error::Manifest`; gains/peaks from `trackReplayGain`, `trackPeakAmplitude`, `albumReplayGain`, `albumPeakAmplitude`.
  - `pub enum Failure { Terminal, Retryable }` and `pub fn classify(err: &Error) -> Failure` — `Terminal` for sub-status 4005, 4010, 4030, 4031, 4032, 4034, 4035 and for `Error::Manifest`; `Retryable` otherwise (including 4006 and 4033).

- [ ] **Step 1: Write failing tests:** `lossless_fixture_yields_segment_urls` (`codec == "flac"`, `bit_depth == Some(16)`, `sample_rate == Some(44100)`, `urls[0]` is the fixture's initialization URL, `urls[1]` and the last url are the media template with the first and last number substituted, `urls.len() == 1 + timeline count`); `hires_fixture_is_24_88200` (`bit_depth == Some(24)`, `sample_rate == Some(88200)`); `timeline_repeat_counts` (inline MPD with `<S d=".." r="67"/><S d=".."/>` → 69 media urls, numbering starts at `startNumber`); `content_protection_is_rejected` (inline MPD with a `ContentProtection` element → `Error::Manifest`); `non_dash_manifest_is_rejected` (`manifestMimeType: "application/vnd.tidal.bts"` → `Error::Manifest`); `classify_terminal_and_retryable` (table over the codes above plus 4006, 4033, and a 500); `error_fixture_is_terminal` (`playbackinfo_error_4005.json` served with 401 → `classify(&err) == Terminal`).
- [ ] **Step 2: Run** `cargo test -p tidal --test playback_http` → FAIL.
- [ ] **Step 3: Implement** `playback.rs`.
- [ ] **Step 4: Run** `cargo test -p tidal --test playback_http` → PASS.
- [ ] **Step 5: Commit** — `git add crates/tidal && git commit -m "feat(tidal): playback info and failure classification"`

### Task 12: `tidal` — play-event reporting

**Files:**
- Create: `crates/tidal/src/events.rs`
- Test: `crates/tidal/tests/events_http.rs`

**Interfaces:**
- Consumes: `Client::send`.
- Produces:
  - `pub fn new_session_id() -> String` — random UUID v4 string (16 random bytes, version/variant bits set; no new crate).
  - `#[derive(Clone, Debug)] pub struct PlaybackReport { pub track_id: u64, pub quality: String, pub streaming_session_id: String, pub started_at_ms: u64, pub ended_at_ms: u64, pub start_position_s: f64, pub end_position_s: f64 }`
  - `pub async fn Client::report_playback(&self, r: &PlaybackReport) -> Result<()>` — exactly the endpoint, headers and JSON documented under `## Play events` in the findings doc.

- [ ] **Step 1: Write failing tests:** `session_id_is_uuid_v4` (36 chars, char 14 is `'4'`, char 19 in `8 9 a b`); `report_matches_documented_payload` (wiremock asserts method, path and that the body JSON equals the documented shape for a fixed report, using `body_partial_json`).
- [ ] **Step 2: Run** `cargo test -p tidal --test events_http` → FAIL.
- [ ] **Step 3: Implement** `events.rs`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `git add crates/tidal && git commit -m "feat(tidal): play-event reporting"`

### Task 13: `player` — progressive stream file

**Files:**
- Create: `crates/player/src/stream.rs`
- Modify: `crates/player/Cargo.toml` (`symphonia = { version = "0.6.1", default-features = false, features = ["flac", "isomp4"] }`, `tempfile = "3"`)

**Interfaces:**
- Produces:
  - `pub struct ProgressiveFile` implementing `std::io::Read`, `std::io::Seek` and symphonia's media-source trait (seekable; byte length known when `len` is `Some` or the download finished).
  - `pub fn ProgressiveFile::spawn(reader: Box<dyn std::io::Read + Send>, len: Option<u64>) -> std::io::Result<ProgressiveFile>` — a writer thread copies `reader` into an anonymous temp file in 64 KiB chunks, publishing `downloaded` (AtomicU64), `done` and the first error through shared state with a `Condvar`.
  - Reads block until the requested bytes are downloaded, the download ends (EOF → short read / 0), or it failed (→ the stored `io::Error` kind and message). Seek never blocks; the following read does.
  - Dropping it stops the writer (flag checked per chunk) and deletes the temp file.

- [ ] **Step 1: Write failing tests** with a `SlowReader` test helper (yields fixed chunks with a short sleep, optionally an error after N bytes):
  - `reads_everything_in_order`: 1 MiB of pseudo-random bytes through `SlowReader` → `read_to_end` equals source.
  - `seek_ahead_waits_then_reads`: seek to byte 900 000 before it is downloaded; the read returns the right bytes.
  - `error_mid_stream_surfaces`: reader fails after 100 000 bytes → reads up to 100 000 succeed, the next read returns `Err`. (Review Focus 1)
  - `drop_stops_writer`: dropping while an endless reader is running returns within 1 s and the thread exits.
- [ ] **Step 2: Run** `cargo test -p player stream` → FAIL.
- [ ] **Step 3: Implement** `stream.rs`. `ponytail:` comment on the struct: sequential fetch, seek-ahead waits; add HTTP Range requests if slow links complain.
- [ ] **Step 4: Run** `cargo test -p player stream` → PASS.
- [ ] **Step 5: Commit** — `git add crates/player && git commit -m "feat(player): progressive stream file"`

### Task 14: `player` — FLAC-in-fMP4 decoder

**Files:**
- Create: `crates/player/src/decode.rs`
- Create: `crates/player/tests/fixtures/{s16_44100.mp4, s16_44100.raw, s24_96000.mp4, s24_96000.raw}`

_Amended after M0: TIDAL streams are FLAC in fragmented MP4 (init segment + media segments as one byte stream). S2 verified symphonia 0.6.1 (`flac` + `isomp4`) decodes them bit-exactly; it reports `num_frames = 0` and ignores `tfdt`, so duration comes from TIDAL metadata, not the decoder._
- Test: `crates/player/tests/decode.rs`

**Interfaces:**
- Produces:
  - `pub struct Pcm { pub rate: u32, pub channels: u16, pub bits: u16, pub samples: Vec<i32> }` — interleaved, left-justified in i32 (sample `<< (32 - bits)`), so 24-bit and 16-bit share one path and `as f32 / 2^31` is full scale.
  - `pub struct Decoder` with `pub fn new(src: Box<dyn MediaSource>) -> Result<Decoder, DecodeError>` (symphonia's media-source trait object; `ProgressiveFile` and `std::fs::File` both qualify), `pub fn rate(&self) -> u32`, `pub fn channels(&self) -> u16`, `pub fn bits(&self) -> u16`, `pub fn next(&mut self) -> Result<Option<Pcm>, DecodeError>` (`None` at end of stream), `pub fn seek(&mut self, to: std::time::Duration) -> Result<std::time::Duration, DecodeError>` (returns the actual position).
  - `#[derive(Debug, thiserror::Error)] pub enum DecodeError { Io(std::io::Error), Unsupported(String), Corrupt(String) }`

- [ ] **Step 1: Generate fixtures** (0.5 s each; `.raw` is ffmpeg's reference decode, left-justified 32-bit):

```bash
cd crates/player/tests/fixtures
F="-f mp4 -movflags frag_keyframe+empty_moov+default_base_moof -frag_duration 100000"
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=0.5" -ac 2 -c:a flac -sample_fmt s16 $F s16_44100.mp4
ffmpeg -i s16_44100.mp4 -f s32le s16_44100.raw
ffmpeg -f lavfi -i "sine=frequency=1000:sample_rate=96000:duration=0.5" -ac 2 -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 $F s24_96000.mp4
ffmpeg -i s24_96000.mp4 -f s32le s24_96000.raw
```

Verify each `.mp4` contains several `moof` boxes (e.g. a byte search for `moof`) before using it.

- [ ] **Step 2: Write failing tests:** `decodes_16bit_bit_exact` and `decodes_24bit_bit_exact` (concatenate all `Pcm.samples`; equal to the `.raw` file read as little-endian i32; `rate`, `channels`, `bits` match); `seek_lands_near_target` (seek to 250 ms on the 96 kHz file → returned position within 10 ms; next samples equal the `.raw` slice at that position).
- [ ] **Step 3: Run** `cargo test -p player --test decode` → FAIL.
- [ ] **Step 4: Implement** `decode.rs` with symphonia 0.6 (probe the stream — fragmented MP4 container, FLAC codec — default track, decoder; convert each decoded buffer to left-justified interleaved i32).
- [ ] **Step 5: Run** `cargo test -p player --test decode` → PASS.
- [ ] **Step 6: Commit** — `git add crates/player && git commit -m "feat(player): bit-exact FLAC-in-fMP4 decoder"`

### Task 15: `player` — resampler

**Files:**
- Create: `crates/player/src/resample.rs`
- Modify: `crates/player/Cargo.toml` (`rubato = "5.0.1"`)

**Interfaces:**
- Produces:
  - `pub struct Resampler` with `pub fn new(from: u32, to: u32, channels: u16) -> Resampler`, `pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>)` (interleaved in, appends interleaved out; buffers partial chunks internally), `pub fn flush(&mut self, out: &mut Vec<f32>)` (emits the tail), `pub fn is_passthrough(&self) -> bool` (`from == to`: copies input unchanged).

- [ ] **Step 1: Write failing tests:** `passthrough_is_identity`; `ratio_44100_to_48000_length` (1 s of stereo sine in → after `flush`, output frames within ±1% of 48 000); `sine_amplitude_preserved` (RMS of the middle 50% of output within 1% of input RMS); `chunked_equals_whole` (feeding 1 s in 37 odd-sized chunks gives the same frame count ±1 chunk as feeding at once).
- [ ] **Step 2: Run** `cargo test -p player resample` → FAIL.
- [ ] **Step 3: Implement** with rubato 5's fixed-ratio synchronous resampler (pick via ctx7; high quality, fixed input chunk).
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `git add crates/player && git commit -m "feat(player): resampler"`

### Task 16: `player` — engine, renderer, device output

**Files:**
- Create: `crates/player/src/render.rs`, `crates/player/src/engine.rs`
- Modify: `crates/player/src/lib.rs` (public API), `crates/player/Cargo.toml` (`rtrb = "0.4"`, `fastframe-audio` git tag v0.4.1, `log`)
- Test: `crates/player/tests/engine.rs`

**Interfaces:**
- Consumes: `ProgressiveFile`, `Decoder`, `Pcm`, `Resampler`.
- Produces:
  - `pub struct Source { pub id: u64, pub reader: Box<dyn std::io::Read + Send>, pub len: Option<u64> }`
  - `pub enum PlayerCmd { Load(Source), Play, Pause, Stop, Seek(std::time::Duration), SetVolume(f32) }`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum PlayState { Stopped, Buffering, Playing, Paused }`
  - `#[derive(Clone, Debug, PartialEq)] pub enum PlayerEvent { State(PlayState), TrackStarted(u64), TrackEnded(u64), Error { id: u64, message: String } }`
  - `pub struct Player` (cheap `Clone`) with `pub fn spawn(options: fastframe_audio::OutputOptions) -> std::io::Result<(Player, std::sync::mpsc::Receiver<PlayerEvent>)>`, `pub fn spawn_manual() -> (Player, std::sync::mpsc::Receiver<PlayerEvent>, Renderer)` (no device; the caller drives the renderer — used by tests), `pub fn send(&self, cmd: PlayerCmd)`, `pub fn position(&self) -> std::time::Duration`.
  - `pub struct Renderer` implementing `fastframe_audio::Render` (`configure(rate, channels)`, `render(&mut [f32])`).

Engine algorithm (pinned):
- One player thread owns the decoder, the resampler, the ring producer and (in device mode) the `fastframe_audio::Output`. Ring: `rtrb` of `f32`, capacity 0.5 s at the device rate × channels.
- Shared atomics: `volume` (f32 bits, default 1.0), `paused` (bool), `flush` (bool), `pending_base_ns`, `base_ns`, `frames` (u64, frames of the current track rendered since `base_ns`), `device_rate`, `device_channels`.
- Loop: drain commands with `try_recv`; when there is nothing to decode (no track, paused with a full ring, or waiting on `flush`), block on `recv_timeout(50 ms)`. In device mode call `output.maintain()` about once per second and drain `output.take_errors()` (fatal → `PlayerEvent::Error` for the current track).
- Decode step: `Pcm` → f32 (`s as f32 / 2_147_483_648.0`) → `Resampler` (from track rate to `device_rate`; rebuilt when `device_rate` changes) → push into the ring; keep whatever doesn't fit as pending for the next iteration. Never push while `flush` is set.
- Flush (Load, Seek, Stop): store the new position in `pending_base_ns`, then set `flush`; the renderer, on its next callback, discards everything readable, sets `base_ns = pending_base_ns`, `frames = 0`, clears `flush` and outputs silence for that callback. In device mode no callbacks run while paused, so the flag waits for resume; no stale audio can play.
- Position = `base_ns + frames / device_rate` (read without locking; momentary inconsistency is fine for UI).
- Device rate change (`configure` reported a new rate, or `maintain()` returned `Reopened`): handled as `Seek(position())` with the resampler rebuilt for the new rate, so no ring audio plays at the wrong pitch.
- End of track: decoder returned `None`, resampler flushed, pending empty, ring empty → emit `TrackEnded(id)` and `State(Stopped)`; drop the decoder.
- Errors from `Decoder::new`/`next` → `PlayerEvent::Error { id, message }`, `State(Stopped)`.
- Renderer: while `paused`, output silence and consume nothing; otherwise output `ring sample × volume`; silence on underrun (does not advance `frames`); maps source channel `i` to device channel `i`, duplicates a mono source to both, leaves extra device channels silent; `configure` stores `device_rate`/`device_channels`.
- `Play`/`Pause` set `paused`, call `output.resume()`/`pause()` in device mode and emit `State`. `Load` emits `State(Buffering)`, then `TrackStarted(id)` and `State(Playing)` once the first samples are in the ring.

- [ ] **Step 1: Write failing tests** (all with `spawn_manual`, `renderer.configure(48_000, 2)`, a helper `render_ms(&mut renderer, ms)` that calls `render` in 10 ms blocks and returns the samples, and `Source` built from the fixture files):
  - `plays_track_to_end`: Load s16_44100.mp4 + Play → events `Buffering, TrackStarted(1), Playing`; render until `TrackEnded(1)` (deadline 5 s); total non-silent frames within ±1% of 24 000.
  - `volume_scales_output`: SetVolume(0.5) → peak of rendered audio ≈ half of the full-volume run (±2%).
  - `pause_stops_and_play_resumes`: Pause → `State(Paused)`; rendering 100 ms yields only silence and `position()` is unchanged; Play → `State(Playing)` and the next 50 ms contain audio.
  - `seek_while_paused_has_no_stale_audio`: Load, Play, render 50 ms, Pause, Seek(400 ms), Play, render until `TrackEnded` → the non-silent frames rendered after the seek total 4 800 ± 10% (the remaining 100 ms at 48 kHz), proving pre-seek ring audio was discarded; `position()` ≥ 400 ms right after the seek. (Review Focus 3)
  - `seek_past_end_ends_track`: Seek(10 s) on a 0.5 s file → `TrackEnded` within 500 ms of rendering. (Review Focus 3)
  - `device_rate_change_mid_track`: after 100 ms, `renderer.configure(44_100, 2)`; track still ends with total duration 0.5 s ± 2% as measured by `position()` just before `TrackEnded`. (Review Focus 2)
  - `stream_error_emits_error`: Source reader that fails after 4 KiB → `PlayerEvent::Error { id: 1, .. }` then `State(Stopped)`, within 2 s. (Review Focus 1)
- [ ] **Step 2: Run** `cargo test -p player --test engine` → FAIL.
- [ ] **Step 3: Implement** `render.rs` and `engine.rs`; `Player::spawn` opens the output on the player thread with the given options (channels 2).
- [ ] **Step 4: Run** `cargo test -p player` → PASS.
- [ ] **Step 5: Manual check:** add `crates/player/examples/play_file.rs` (`cargo run -p player --example play_file -- <path.mp4>`) and play `s24_96000.mp4` on the real device → audible, ends cleanly.
- [ ] **Step 6: Commit** — `git add crates/player && git commit -m "feat(player): engine, renderer, device output"`

### Task 17: `app` — shell, model, backend, login

**Files:**
- Create: `crates/app/src/main.rs` (replace), `crates/app/src/model.rs`, `crates/app/src/backend.rs`, `crates/app/src/ui/mod.rs`, `crates/app/src/ui/login.rs`, `crates/app/build.rs`, `crates/app/po/tidalfast.pot`
- Modify: `crates/app/Cargo.toml` (eframe/egui 0.36 with `glow`, `accesskit`, `wayland`, `x11`; fastframe `text`, `fonts`, `theme`, `i18n` (+ build), `log`; `tidal`, `player`; `tokio` with `rt-multi-thread`, `sync`, `time`, `macros`; `reqwest` with `blocking`; `open = "5"`)
- Test: `crates/app/src/model.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `tidal::{ClientConfig, Pkce, authorize_url, code_from_redirect, exchange_code, Session, Client, default_store}`.
- Produces:
  - `pub enum Route { Login, Search, Album(u64) }`
  - `pub enum Action { BeginLogin, FinishLogin(String), Logout, Search(String), OpenAlbum(u64), PlayTracks { tracks: Vec<tidal::Track>, start: usize }, Play, Pause, Next, Previous, Seek(std::time::Duration), SetVolume(f32), LoadImage { cover: String, size: u32 } }`
  - `pub enum Event { LoginUrl(String), LoggedIn, LoggedOut, CredentialsNotPersistent, SearchResults { query: String, results: tidal::SearchResults }, Album { album: tidal::Album, tracks: Vec<tidal::Track> }, Queue(QueueSnapshot), Playback(PlaybackSnapshot), Image { cover: String, size: u32, image: egui::ColorImage }, Error(String) }`
  - `#[derive(Clone, Debug, Default)] pub struct QueueSnapshot { pub tracks: Vec<tidal::Track>, pub current: Option<usize> }` and `#[derive(Clone, Debug, Default)] pub struct PlaybackSnapshot { pub state: Option<player::PlayState>, pub track: Option<tidal::Track>, pub volume: f32 }`
  - `pub struct Model { pub route: Route, pub history: Vec<Route>, pub logged_in: bool, pub login_url: Option<String>, pub search: Option<(String, tidal::SearchResults)>, pub albums: HashMap<u64, (tidal::Album, Vec<tidal::Track>)>, pub queue: QueueSnapshot, pub playback: PlaybackSnapshot, pub errors: Vec<String> }` with `pub fn apply(&mut self, ev: Event)` (images are handled by the UI's texture cache, not `Model`; `CredentialsNotPersistent` pushes the warning "Login will not be remembered: no system credential store available" to `errors`), `pub fn navigate(&mut self, r: Route)`, `pub fn back(&mut self) -> bool`.
  - `pub fn backend::start(waker: impl Fn() + Send + Sync + 'static) -> (tokio::sync::mpsc::UnboundedSender<Action>, std::sync::mpsc::Receiver<Event>)` — builds a 2-worker tokio runtime on its own thread; every sent `Event` is followed by `waker()` (the UI passes `ctx.request_repaint`).

- [ ] **Step 1: Write failing tests** in `model.rs`: `logged_in_moves_to_search` (`apply(LoggedIn)` from `Route::Login` → `Route::Search`, `logged_in == true`); `logged_out_returns_to_login`; `navigate_and_back` (Search → Album(1) → Album(2); `back()` twice → Search; third `back()` → false); `errors_are_collected` (`apply(Error("x"))` pushes to `errors`).
- [ ] **Step 2: Run** `cargo test -p tidalfast model` → FAIL.
- [ ] **Step 3: Implement** `model.rs`, then `backend.rs` login flow: at start `default_store()`; a stored session → `Client::new` → `LoggedIn` (plus `CredentialsNotPersistent` when the store is memory-only); `BeginLogin` → new `Pkce` kept in the backend, `open::that(authorize_url)`, emit `LoginUrl`; `FinishLogin(url)` → `code_from_redirect` → `exchange_code` → `store.save` → `Client::new` → `LoggedIn`; `Logout` → `store.clear` → `LoggedOut`. Every `Err` → `Event::Error(user-facing message)`.
- [ ] **Step 4: Implement UI:** `main.rs` sets up fastframe fonts + text rendering + dark theme, starts the backend, runs `eframe::run_native`; each frame drains events into `Model::apply` and draws by route. `ui/login.rs`: "Log in with TIDAL" button → `BeginLogin`; then shows the URL (copyable), a text field for the pasted redirect URL and a "Continue" button → `FinishLogin`. Errors render as dismissible text at the top.
- [ ] **Step 5: Run** `cargo test -p tidalfast` → PASS; `cargo run -p tidalfast` → full login works against TIDAL; restart → logged in without the browser.
- [ ] **Step 6: Commit** — `git add crates/app && git commit -m "feat(app): shell, model, backend, login"`

### Task 18: `app` — search, album page, artwork

**Files:**
- Create: `crates/app/src/ui/search.rs`, `crates/app/src/ui/album.rs`, `crates/app/src/ui/sidebar.rs`, `crates/app/src/images.rs`
- Modify: `crates/app/src/backend.rs`, `crates/app/src/ui/mod.rs`, `crates/app/Cargo.toml` (`image = { version = "0.25.10", default-features = false, features = ["jpeg"] }`)
- Test: `crates/app/src/images.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Client::{search, album, album_tracks}`, `tidal::image_url`, `Action::{Search, OpenAlbum, LoadImage}`, `Event::{SearchResults, Album, Image}`.
- Produces:
  - `pub struct TextureCache` in `images.rs`: `pub fn new(capacity: usize) -> TextureCache`, `pub fn get(&mut self, ctx: &egui::Context, cover: &str, size: u32, request: impl FnOnce()) -> Option<egui::TextureHandle>` (returns the texture or calls `request` once per missing key), `pub fn insert(&mut self, ctx: &egui::Context, cover: String, size: u32, image: egui::ColorImage)`. Least-recently-used eviction; capacity 512.
  - Backend: `LoadImage` → fetch `image_url` → decode JPEG in `spawn_blocking` → `Event::Image`. Search and album actions call the client concurrently.

- [ ] **Step 1: Write failing tests:** `cache_requests_once_per_key` (two `get`s for a missing key call `request` once); `cache_evicts_least_recently_used` (capacity 2: insert a, b, get a, insert c → b gone, a present).
- [ ] **Step 2: Run** `cargo test -p tidalfast images` → FAIL.
- [ ] **Step 3: Implement** `images.rs`, the backend handlers, and the views: sidebar (Search entry, back button); search (text field submitting on Enter → `Search`; albums as a virtualized grid with 160 px covers, tracks as a virtualized list; click album → `OpenAlbum` + `navigate(Album(id))`; double-click track → `PlayTracks` with the result tracks); album (320 px cover, title, artists, year, track list with disc headers; double-click row → `PlayTracks { tracks, start: row }`; "Play" button → start 0). Rows of tracks with `stream_ready == false` render dimmed.
- [ ] **Step 4: Run** `cargo test -p tidalfast` → PASS; `cargo run -p tidalfast` → search shows results with covers; an album opens with its full track list.
- [ ] **Step 5: Commit** — `git add crates/app && git commit -m "feat(app): search, album page, artwork"`

### Task 19: `app` — queue, playback bridge, player bar

**Files:**
- Create: `crates/app/src/queue.rs`, `crates/app/src/playback.rs`, `crates/app/src/ui/player_bar.rs`
- Modify: `crates/app/src/backend.rs`, `crates/app/src/ui/mod.rs`, `crates/app/src/main.rs`

**Interfaces:**
- Consumes: `player::{Player, PlayerCmd, PlayerEvent, Source}`, `Client::{stream_info, report_playback}`, `tidal::{classify, Failure, Quality, new_session_id, PlaybackReport}`.
- Produces:
  - `pub struct Queue` with `pub fn replace(&mut self, tracks: Vec<tidal::Track>, start: usize) -> Option<&tidal::Track>` (first `stream_ready` track at or after `start`), `pub fn current(&self) -> Option<&tidal::Track>`, `pub fn next(&mut self) -> Option<&tidal::Track>` (skips `!stream_ready`; `None` at the end), `pub fn previous(&mut self) -> Option<&tidal::Track>` (skips `!stream_ready`; `None` at the start), `pub fn snapshot(&self) -> QueueSnapshot`.
  - `pub struct SegmentReader` in `playback.rs`: `pub fn new(urls: Vec<String>, open: impl FnMut(&str) -> std::io::Result<Box<dyn std::io::Read + Send>> + Send + 'static) -> SegmentReader`, implementing `Read` by reading each URL's body in order, opening the next lazily when the current one ends; an `open` or read error is returned from `read`. In production `open` is a `reqwest::blocking` GET with no Authorization header (S1: segment URLs are pre-signed); `Source { reader: Box::new(SegmentReader), len: None }`.
  - `backend::start` now creates the player first and returns `(tokio::sync::mpsc::UnboundedSender<Action>, std::sync::mpsc::Receiver<Event>, player::Player)`; the UI keeps the `Player` clone only to read `position()`.
  - `playback.rs`: backend-side glue. Owns `Queue`, the `Player`, and the current play report. `start_current()`: `stream_info(id, Quality::Lossless)` → build a `SegmentReader` over `StreamInfo.urls` (created inside `spawn_blocking`, since the blocking client must not run on a runtime thread) → `PlayerCmd::Load` + `Play`. Results for a track that is no longer current are dropped.

Rules (pinned):
- `PlayerEvent::TrackEnded` → send the play report, `queue.next()` → `start_current()`, or stop at the end.
- `PlayerEvent::Error` (e.g. the stream dropped): once per track, `start_current()` again and `Seek` to the last known position; a second error for the same track is handled like `Terminal`.
- `Previous`: if `player.position() > 3 s` → `Seek(0)`; else `queue.previous()` → `start_current()`.
- `stream_info` error: `Terminal` → `Event::Error("<title> is unavailable")` and advance; `Retryable` → one retry after 2 s, then same as `Terminal`. Three consecutive failures → stop playback with an error.
- Player events reach the backend through a std thread that forwards the player's `Receiver` into the backend's tokio channel.
- Player bar: cover 56 px, title, artists; previous / play-pause / next; seek slider (drag → `Seek` on release) showing `player.position()` against the track duration; volume slider → `SetVolume`. While playing and the window is visible, `ctx.request_repaint_after(250 ms)`; otherwise no periodic repaint.

- [ ] **Step 1: Write failing tests** in `playback.rs`: `segment_reader_concatenates_in_order` (3 in-memory segments via a fake `open` → `read_to_end` equals their concatenation; `open` called in URL order, lazily); `segment_reader_surfaces_open_error` (second `open` fails → `read` returns `Err` after the first segment's bytes). And in `queue.rs`: `replace_skips_unavailable_start` (start index on an unavailable track → next available); `next_skips_unavailable_and_ends` (Review Focus 5); `previous_stops_at_start`; `snapshot_reflects_current`.
- [ ] **Step 2: Run** `cargo test -p tidalfast queue playback` → FAIL.
- [ ] **Step 3: Implement** `queue.rs`, then `playback.rs`, backend wiring (`Player::spawn(OutputOptions::default())` at startup) and `ui/player_bar.rs`.
- [ ] **Step 4: Run** `cargo test --workspace` → PASS; `cargo run -p tidalfast --release` → play an album: tracks advance, pause/seek/volume/previous/next work, Task Manager shows ~0% CPU while paused with the window minimized.
- [ ] **Step 5: Commit** — `git add crates/app && git commit -m "feat(app): queue, playback, player bar"`

### Task 20: `app` — OS media controls

**Files:**
- Create: `crates/app/src/now_playing.rs`
- Modify: `crates/app/src/backend.rs`, `crates/app/Cargo.toml` (`fastframe-now-playing` git tag v0.4.1)

**Interfaces:**
- Consumes: `PlaybackSnapshot` changes in the backend; `Action::{Play, Pause, Next, Previous, Seek}`.
- Produces: `now_playing.rs` wraps fastframe-now-playing (per its README): publishes title, artists, album, cover URL (640 px via `image_url`), duration, playing/paused; maps incoming media commands to the matching `Action` on the backend channel.

- [ ] **Step 1: Implement** `now_playing.rs` and call it from the backend on every playback change.
- [ ] **Step 2: Verify:** `cargo clippy --all-targets -- -D warnings` clean; on Windows, the media overlay shows the track and cover; the keyboard play/pause, next and previous keys work with the window unfocused.
- [ ] **Step 3: Commit** — `git add crates/app && git commit -m "feat(app): OS media controls"`

### Task 21: M1 acceptance

**Files:**
- Modify: `docs/superpowers/specs/2026-10-07-tidalfast-design.md` (only if a deviation was found)

- [ ] **Step 1: CI:** push to GitHub; `quality` and `test` green on Ubuntu, Windows and macOS.
- [ ] **Step 2: Windows acceptance (human partner):** fresh start → log in → search → open album → play through two track changes → pause/seek/volume → media keys → restart stays logged in. Record cold-start time and idle RSS after an album page is shown.
- [ ] **Step 3: macOS / Linux acceptance:** same checklist if a machine is available; otherwise record "build + tests only (CI)".
- [ ] **Step 4: Commit** any spec corrections — `git commit -am "docs: M1 acceptance notes"`
