# S1: TIDAL auth and manifests

Findings from live probing on 2026-10-07 with the human's subscribed account (account country not material). Official client IDs are embedded in this project with the human partner's approval. Streaming only; no downloads or DRM circumvention. Probe: `spikes/s1-auth/probe.py`. Fixtures: `crates/tidal/tests/fixtures/` (sanitized).

## Client

**Chosen: `android`**, client_id `6BDSRdpK9hqEBTgU`, redirect `https://tidal.com/android/login/auth`. It needs NO client secret for the login code exchange and NO client secret for refresh (verified live). It returns full-length FLAC for LOSSLESS (16/44.1) and HI_RES_LOSSLESS (24/88.2 on the test track), unencrypted (DRM-free DASH/fMP4), see Manifests. The token response includes `user` with `countryCode`.

Candidates, in rank order, and their status:

| name | client_id | redirect_uri | appMode | secret | status |
|------|-----------|--------------|---------|--------|--------|
| `android` | `6BDSRdpK9hqEBTgU` | `https://tidal.com/android/login/auth` | `android` | not needed for exchange or refresh (python-tidal sends one on refresh; server accepted refresh without it) | CHOSEN, fully tested |
| `web-listen` | `49YxDN9a2aFV6RTG` (official web client) | `https://listen.tidal.com/login/auth` | `WEB` | no | untested |
| `desktop` | `mhPVJJEBNRzVjr2p` (official desktop web client, `enableDesktopFeatures`) | `tidal://login/auth` | `DESKTOP` | no | untested; the installed TIDAL app owns `tidal://` and would swallow the redirect |
| `web` | `49YxDN9a2aFV6RTG` | `https://desktop.tidal.com/login/auth` | `WEB` | no | REJECTED by login.tidal.com: "Something went wrong ... 11102" (no documentation of 11102 found in python-tidal, SONE or web searches; most likely an unregistered redirect_uri for that client) |

Excluded: device-code client (`fX2JxdmntZWK0ixT`, has a secret; capped at 320 kbps AAC since Sept 2026). Other web-bundle IDs (not tried): `5DXXTensXNaZNdzL` Epic store, `3a5dzbD1VqdaUmqG` Windows store, `DfPdNV5M8ZYTLwqW2sZfPFm1ce6erS` Safari/FairPlay, `bj79VpA2sCAwTufe` DEVICE_LOGIN, `YuBokFUKeupw1zTdlyw4` STAGE.

Sources: official web bundle (`index-*.js` client-id selector, `vendor-*.js` auth module), SONE `src-tauri/src/commands/auth.rs`, python-tidal `tidalapi/session.py` at commit `e49ea353d4127fffaa46a204f0468aafc6ed909d` (2025-11-12, main). The `android` client's client_secret exists in python-tidal and SONE but is neither required nor recorded here.

## Authorize request

`GET https://login.tidal.com/authorize` with query (mirrors python-tidal `pkce_login_url`, verified working):

```
response_type=code
redirect_uri=https://tidal.com/android/login/auth
client_id=6BDSRdpK9hqEBTgU
lang=EN
appMode=android
client_unique_key=<64-bit random as 16 hex chars>
code_challenge=<base64url(sha256(verifier)), no padding>
code_challenge_method=S256
restrict_signup=true
```

No `scope` and no `state` are sent for `android` (the app still gets scopes `r_usr w_usr w_sub`). `code_verifier` = base64url(32 random bytes), no padding. After login the browser lands on the `tidal.com/android/login/auth?code=...` page; the user copies the full URL from the address bar. Opening this in a normal browser is fine; scripted GETs get a Cloudflare 403.

Code exchange: `POST https://auth.tidal.com/v1/oauth2/token`, `application/x-www-form-urlencoded`, fields `code, client_id, grant_type=authorization_code, redirect_uri, scope=r_usr+w_usr+w_sub (sent literally with plus signs), code_verifier, client_unique_key`. No client_secret, no Authorization header.

## Token response

HTTP 200 JSON, keys: `access_token` (JWT), `refresh_token`, `token_type` ("Bearer"), `expires_in` (14400, i.e. 4 h), `scope` ("w_usr w_sub r_usr"), `clientName` ("TIDAL_Android_2.87.0"), `user_id` (number), `user` (object). `user` keys: `userId, countryCode, email, username, firstName, lastName, fullName, nickname, birthday, address, city, postalcode, phoneNumber, usState, appleUid, googleUid, channelId, parentId, created, updated, acceptedEULA, accountLinkCreated, emailVerified, newUser`. The `user.countryCode` is what to pass as `countryCode=` on API calls. See `fixtures/token.json` (sanitized).

## Refresh

`POST https://auth.tidal.com/v1/oauth2/token` form: `grant_type=refresh_token, refresh_token, client_id` (no client_secret, no scope). Verified HTTP 200 without a secret.

Response keys: `access_token, clientName, expires_in (14400), scope, token_type, user, user_id`. It does NOT return a new `refresh_token`: keep the existing one. It does return the full `user` object. See `fixtures/refresh.json`.

## Manifests

Request (Bearer access token, host `https://api.tidal.com/v1`): `GET /tracks/{id}/playbackinfo` and `GET /tracks/{id}/playbackinfopostpaywall` with `audioquality={LOW|HIGH|LOSSLESS|HI_RES_LOSSLESS}&playbackmode=STREAM&assetpresentation=FULL&countryCode=<cc>`. Both endpoints returned identical results for all four qualities on both test tracks.

Response JSON keys: `trackId, assetPresentation, audioMode, audioQuality, manifestMimeType, manifestHash, manifest (base64), albumReplayGain, albumPeakAmplitude, trackReplayGain, trackPeakAmplitude`, plus `bitDepth, sampleRate` only for LOSSLESS and HI_RES_LOSSLESS.

Every quality uses `manifestMimeType: application/dash+xml`: a base64 MPD (not the old `vnd.tidal.bts` JSON). No `ContentProtection` element appears in any manifest (the `cenc` namespace is declared but unused), so no encryption: segments are plain fMP4. One Period, one AdaptationSet (`mimeType audio/mp4`), one Representation, `SegmentTemplate` with `initialization` URL, `media` URL containing `$Number$`, `startNumber=1`, a `SegmentTimeline` of `<S d=".." r="..">` entries (~4 s segments; timescale = sample rate), `mediaPresentationDuration`, and the URLs on `sp-ad-fa.audio.tidal.com/mediatracks/...` (CDN host; signed tokens in the path).

| requested | returned audioQuality | MPD Representation id | codecs | encryption | bitDepth / sampleRate (JSON) | bandwidth (track 328638758) |
|-----------|----------------------|-----------------------|--------|-----------|------------------------------|-----------|
| LOW | LOW | `HEAACV1` | `mp4a.40.5` | none | absent | 96968 |
| HIGH | HIGH | `AACLC` | `mp4a.40.2` | none | absent | 321678 |
| LOSSLESS | LOSSLESS | `FLAC,44100,16` | `flac` | none | 16 / 44100 | 688497 (track 20115557 measured 906236; FLAC bandwidth varies per track) |
| HI_RES_LOSSLESS | HI_RES_LOSSLESS | `FLAC_HIRES,88200,24` | `flac` | none | 24 / 88200 | 2242796 |

(Identical for `playbackinfo` and `playbackinfopostpaywall`.) A track that is not available in hi-res silently downgrades: requesting HI_RES_LOSSLESS on a LOSSLESS-only track (id 20115557, album tagged only LOSSLESS) returned `audioQuality: LOSSLESS`, 16/44100 flac. The hi-res test track was id 328638758 (album tagged LOSSLESS + HIRES_LOSSLESS).

Stream check (init segment + first segments, ~1.2-1.4 MB, fetched without auth headers, deleted after probing): `ffprobe` reported `codec_name=flac, sample_fmt=s16, sample_rate=44100, bits_per_raw_sample=16` for LOSSLESS and `flac, s32, 88200, bits_per_raw_sample=24` for HI_RES_LOSSLESS. Segment URLs need no Authorization header.

Error shape: unavailable asset, e.g. track id 100 returns **HTTP 401** with `{"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}` (fixture `playbackinfo_error_4005.json`). Note a 401 here is NOT an expired token: distinguish by `subStatus`. Tiny ids like 1, 2, 5 return HTTP 500 subStatus 999.

## Images

Album object `cover` is a UUID (`d7b7a55e-8c5a-4957-b7ba-786184496354`). URL: `https://resources.tidal.com/images/<uuid with - replaced by />/<w>x<h>.jpg`, e.g. `.../d7b7a55e/8c5a/4957/b7ba/786184496354/640x640.jpg`. Verified HTTP 200 `image/jpeg` for 80x80, 320x320, 640x640, 1280x1280 and `origin.jpg`. `images.tidal.com/<same path>` returned 400 for the same input, so use `resources.tidal.com`.

## Play events

Reporting a play (so it shows in TIDAL Recently Played). Sources: web bundle (`vendor-*.js` event sender: `group: play_log`, `name: playback_session`, `version: 2`; `tlConsumerUri = <origin>/api/event-batch`, i.e. the web client posts to its own origin) and SONE `src-tauri/src/tidal_report/{event,mod}.rs`, which posts the mobile shape that matches our `android` token. Not sent live in this spike (no write probing).

- Endpoint: `POST https://ec.tidal.com/api/event-batch`, header `Authorization: Bearer <access_token>`, `Content-Type: application/x-www-form-urlencoded`. Body is an SQS `SendMessageBatch` form (max 10 entries): per entry n (from 1): `SendMessageBatchRequestEntry.n.Id=<uuid>`, `.MessageBody=<JSON string below>`, `.MessageAttribute.1.Name=Name` / `.Value.StringValue=playback_session` / `.Value.DataType=String`, `.MessageAttribute.2.Name=Headers` / `.Value.StringValue=<headers JSON>` / `.Value.DataType=String`. Response is SQS XML; `<BatchResultErrorEntry` means sender fault (drop); HTTP 401/403 means refresh and retry once; 5xx/network means queue and retry.
- Headers JSON (exactly 9 keys): `client-id` (the OAuth client id), `app-version`, `os-name`, `os-version`, `device-model`, `device-vendor`, `consent-category: "NECESSARY"`, `requested-sent-timestamp` (ms, string), `authorization` (bare access token, no Bearer).
- MessageBody JSON: `{"group":"play_log","version":2,"ts":<end ms>,"uuid":"<uuid>","user":{"id":<uid>,"clientId":<cid>,"sessionId":"<sid>"},"client":{"token":"<cid as string>","deviceType":"mobile","version":"<app version>","platform":"android"},"payload":{"playbackSessionId":"<uuid>","isPostPaywall":true,"productType":"TRACK","requestedProductId":"<track id>","actualProductId":"<track id>","actualAssetPresentation":"FULL","actualAudioMode":"STEREO","actualQuality":"LOSSLESS","startTimestamp":<ms>,"endTimestamp":<ms>,"startAssetPosition":0.0,"endAssetPosition":<secs>,"actions":[],"sourceType":"ALBUM","sourceId":"<id>"}}`. `uid`, `cid`, `sid` come from the access-token JWT claims (`uid`, `cid`, `sid`). `sourceType` is one of `ALBUM, PLAYLIST, ARTIST, MIX, ITEM (sourceId = the track id), MY_ITEMS (sourceId MY_TRACKS)`; a play with no source is accepted but never reaches Recently Played.
- When sent: one `playback_session` event per play, when the play ends (natural end, skip, stop, or app shutdown), only if at least 30 s of audible play accrued (pause time excluded). `endAssetPosition` is the track duration on natural end, otherwise the elapsed play seconds clamped to duration. `actions` stays `[]` for a straight play (only interruptions are recorded). Failed sends are queued and retried.
- Unknown/needs care: values for `app-version`, `os-*`, `device-*` mimic a real Android client and drift weekly; this is an undocumented endpoint, best effort.
