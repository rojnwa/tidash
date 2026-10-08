# S4: TIDAL library endpoints

Live probing on 2026-10-08 with the human's subscribed account and the `android` client token (scopes `r_usr w_usr w_sub`), read-only GETs only, with `cargo run -p tidal --example record -- <v1|v2|openapi> <path?query> <name> ["Header: value"…]` (`crates/tidal/examples/record.rs`). Fixtures: `crates/tidal/tests/fixtures/` (sanitized, see the end). What the official client does comes from reading the official desktop web client, build 2026.09.30, without network calls. Rows marked *official client, not probed* rest on that reading alone.

## Request conventions

| | v1 `https://api.tidal.com/v1/` | v2 `https://api.tidal.com/v2/` | openapi `https://openapi.tidal.com/v2/` |
|---|---|---|---|
| query | `countryCode`, `locale=en_US`, `deviceType=DESKTOP` | the same plus `platform=DESKTOP` | `countryCode` (+ `include`, `filter[…]`, `page[cursor]`) |
| headers | `Authorization: Bearer`, `Accept: application/json` | the same plus **`x-tidal-client-version: 2026.9.30` (required: without it → 400, subStatus 1002)** | `Accept: application/vnd.api+json` |
| home feed | — | plus `timeOffset=±HH:MM`, the local UTC offset (`+` sent as `%2B`) | — |

- `DESKTOP` vs `deviceType=BROWSER&platform=WEB`: the home feed and the artist page return the same modules and items (only the page `uuid` and mix `updated` timestamps differ). **Keep `DESKTOP`.**
- An old client version (`2025.7.16`) is accepted with the same response. The header must be present, but the server does not check its value. Keep the value in config anyway.
- v2 has the same origin as v1, so `Client`'s origin guard already lets v2 URLs through.
- Too-large pages: v1 → 400 subStatus 1001 `"Too big page, max page size is [N]"`; v2 → 400 subStatus 1002.
- The official client also retries GETs on 429/5xx and refreshes the token once on 401. Writes are never retried. *Official client, not probed.*

## Findings per question

| # | request | status | answer |
|---|---|---|---|
| 1 | v2 `home/feed/static?timeOffset=…` | 200 | Top level: `{uuid, page:{cursor}, header:{vibes:{items:[{name,type}]}}, items:[module]}`. Vibes: `STATIC`, `EDITORIAL`, `UPLOADS`. BROWSER/WEB gives the same modules. `cache-control: no-store, proxy-revalidate`, **no ETag**. |
| 2 | the same + `cursor=<page.cursor>`; + `limit` | 200 | Page 1 has 10 modules; page 2 has the remaining 8 modules, `header: null` and `cursor: null` (the end). `limit` counts **modules**: `limit=50` returns all 18 modules on one page with `cursor: null`, and `limit=3` returns 4. |
| 3 | v2 `home/pages/NEW_ALBUM_SUGGESTIONS/view-all?limit=50&offset=…` | 200 | `viewAll` strings have **no leading slash**: `home/pages/<MODULE_ID>/view-all`, sometimes with `?itemId=<opaque>` (and `&rankingToken=<n>` on interest shelves). Response: `{title, subtitle, description, itemLayout: GRID\|COMPACT, filters: [], items:[{type,data}]}`. **No total**: the end is a short page (105 items came as 50, 50, 5). `limit=200` is accepted; `limit=1000` → 400. |
| 4 | DEEP_LINK / MAGAZINE / TASK items | — | Not present in this account's feed. Only ALBUM, ARTIST, PLAYLIST, TRACK and MIX items were seen. |
| 5 | v1 `pages/explore`, then `pages/genre_pop` | 200 | Explore is only link modules: `PAGE_LINKS_CLOUD` "Genres" (18), "Moods & Activities" (15 of 19; `showMore.apiPath: pages/moods_page`), "Decades" (7), and an untitled `PAGE_LINKS` (New, Top, Videos, HiRes, Clean Content). Link item: `{title, apiPath: "pages/genre_pop", icon, imageId}` (one title is empty). A genre page has `PLAYLIST_LIST`, `TRACK_LIST`, `ALBUM_LIST`, `VIDEO_LIST` and `ARTIST_LIST` modules with `pagedList.items` = bare v1 entities. Its `showMore.apiPath` = `pages/single-module-page/<page>/<n>/<module>/<n>`, a v1 page with one module holding 50 items plus `dataApiPath`/`totalNumberOfItems`. |
| 6 | v1 `pages/data/<uuid>?offset=0&limit=100` | 400 | **Max 50** (subStatus 1001). At `limit=50` it returns `{limit, offset, totalNumberOfItems, items}`; the last page was 21 items at offset 250 of 271. |
| 7 | v2 `artist/8847`, then `artist/ARTIST_ALBUMS/view-all?artistId=8847&limit=50&offset=0` | 200 | Top level `{type:"ARTIST", item:{type,data}, header, items}`. `header`: `biography{text,source,moderationStatus}`, `followersAmount`, `playableContent{play,shuffle,items}`, `profilePrivacyType`, … The bio contains **`<br/>` and `[wimpLink albumId="…"]…[/wimpLink]`** markup. **No `DISCOGRAPHY_TABS`** on either artist probed: the discography is separate `HORIZONTAL_LIST` modules (see the module table). `viewAll` = `artist/<MODULE_ID>/view-all?artistId=<id>`. A view-all has the same shape as the home view-all; `filters` is `[]`. |
| 8 | v2 `artist/8847/playable?limit=50&offset=…` | 200 | `{play, shuffle, url:null, cursor:null, items:[bare tracks]}`. Pages by offset (50 more at offset 50). |
| 9 | v1 `playlists/<uuid>/items?limit=100&order=INDEX&orderDirection=ASC`, then `If-None-Match` | 200 / 304 | **Max 100** (`limit=101` → 400 subStatus 1001). `If-None-Match` → **304** on both `playlists/<uuid>` and `/items`, which share one ETag. Items: `{type:"track", cut, item:{…, dateAdded, itemUuid}}`. |
| 10 | v1 `users/{uid}/favorites/tracks?limit=…&order=DATE&orderDirection=DESC`, then `If-None-Match` | 200 / 304 | 304 honoured. `limit=1000` is accepted (no cap found). **Pages hold fewer items than `limit` mid-list**: 406 favorites (`totalNumberOfItems` and the `favorites/ids` count agree), but `limit=100` gives 90 items and `limit=1000` gives 362. The server drops 44 unavailable tracks. **A short page is not the end: advance `offset` by `limit` and stop at `totalNumberOfItems`.** |
| 11 | v1 `users/{uid}/favorites/ids`; v2 `…/folders/flattened?includeOnly=FAVORITE_PLAYLIST` | 200 | Values are strings. `PLAYLIST` is present and holds **only favorited playlists (not own ones)**, exactly the set the `includeOnly=FAVORITE_PLAYLIST` list returns. In the flattened list **favorited playlists have `itemType: "PLAYLIST"`**, like own ones; only the `includeOnly` filter (or `creator.id`) tells them apart. |
| 12 | v2 `favorites/mixes?offset=0&limit=50&order=DATE&orderDirection=DESC&cursor=` | 200 | `{items, cursor, lastModifiedAt}`, no total. Items are **bare mixes**: `id, title, subTitle, mixType, dateAdded, updated, master, images{SMALL,MEDIUM,LARGE:{url,width,height}}, detailImages{…}, titleTextInfo{text,color}, subTitleTextInfo{…}`. Paging by `cursor` works (`limit=2` → cursor → next 2). |
| 13 | v2 `favorites/mixes/ids?limit=500&cursor=` | 200 | `{content:[ids], cursor}`; `cursor: null` at the end; cursor paging works. |
| 14 | v2 `my-collection/playlists/folders?folderId=root&includeOnly=&…` | 200 | `{items, lastModifiedAt, totalNumberOfItems}` (flattened: `{items, lastModifiedAt, cursor}`, no total). **`limit=100` → 400** (max 50, subStatus 1002), on folders and on flattened. `order=DATE_UPDATED` is accepted (a different order from `DATE`). This account has no folders, so the FOLDER item shape is unrecorded. |
| 18 | v1 `pages/mix?mixId=<id>` | 200 | `MIX_HEADER.mix.images` and `detailImages` are 320/640/1500 square. The `TRACK_LIST` `pagedList` holds **all** tracks (`limit 100`, total 100, 100 items, bare tracks). Mix fields: `id, title, subTitle, shortSubtitle, mixType, titleColor, subTitleColor, images, detailImages, contentBehavior, master, …`. |
| 19 | v1 `mixes/<id>/items?offset=…&limit=…` | 200 | `offset`/`limit` are **ignored**: always all 100 items with `totalNumberOfItems: 100`. Items are `{type:"track", item}`. |
| 20 | v2 home and artist without `x-tidal-client-version`; with `2025.7.16` | 400 / 200 | The header is required, its value is not checked (see conventions). |
| 21 | v2 home and artist response headers | 200 | Both `no-store, proxy-revalidate`, no ETag, `vary: Origin,…`, so no conditional GET. View-alls and `playable` are the same. |
| 22 | openapi, see below | 200 | The android token is accepted. |

Q15–17 are writes and were not probed (see Favorites writes).

## Endpoints for M2

| page | request | paging | max page | cache-control | ETag / 304 |
|---|---|---|---|---|---|
| Home | v2 `home/feed/static` | `cursor` from `page.cursor` until null (or `limit=50` for everything at once) | — | `no-store` | no |
| Home view-all | v2 `<viewAll>` + `limit`, `offset` | offset; a short page ends it | ≥ 200 (1000 → 400) | `no-store` | no |
| Explore, genre/mood pages | v1 `pages/explore`, v1 `<apiPath>` | none (`showMore.apiPath` is another page) | — | `private, max-age=1` | yes / **304** |
| Page module lists | v1 `<pagedList.dataApiPath>` + `offset`, `limit` | offset to `totalNumberOfItems` | **50** | `private, max-age=1` | yes |
| Artist | v2 `artist/{id}`; view-all v2 `<viewAll>` + `limit`, `offset` | as home view-all | — | `no-store` | no |
| Playlist | v1 `playlists/{uuid}`, v1 `playlists/{uuid}/items` (`order=INDEX&orderDirection=ASC`) | offset to `totalNumberOfItems` | **100** | `no-cache` | yes / **304** |
| Mix | v1 `pages/mix?mixId=` (header and all tracks); v1 `mixes/{id}/items` (all tracks) | none | — | page `private, max-age=1`; items `public, max-age=300` | yes / **304** (both) |
| Favorite tracks/albums/artists | v1 `users/{uid}/favorites/{tracks\|albums\|artists}` (`order=DATE&orderDirection=DESC`), items `{created, item}` | offset **by `limit`** to `totalNumberOfItems` (pages run short) | ≥ 1000 | `no-cache` | yes / **304** |
| Favorite mixes | v2 `favorites/mixes` | `cursor` until null | 50 used | `max-age=0, proxy-revalidate` | ETag changes on every call → no 304 in practice |
| Playlists (own + favorited) | v2 `my-collection/playlists/folders/flattened?includeOnly=` (`order=DATE&orderDirection=DESC`) | `cursor` until null (offset stays 0) | **50** | `max-age=0, proxy-revalidate` | yes / **304** |
| Favorite ids | v1 `users/{uid}/favorites/ids`; v2 `favorites/mixes/ids?limit=500` (cursor loop) | — / cursor | — | `no-cache` / `max-age=0` | v1 **304**; v2 ETag changes every call |
| Queue | none: the official queue is local state; a bare track id hydrates with v1 `tracks/{id}` | — | — | — | — |

`users/{uid}`: `no-cache` with an ETag. Mix image URLs (`images.tidal.com/0/…?token=<signature>`) are full URLs: key them by the full URL.

## Module and item types

v2 home feed modules: `{type, moduleId, title, subtitle, icons, showQuickPlay, viewAll, header, items:[{type, data, following, latestRelease?, numberOfFollowers?}]}`. Item `data` for ALBUM, ARTIST, PLAYLIST and TRACK is the v1 entity shape; MIX is the v2 mix shape.

| module type | seen in | items | M2 |
|---|---|---|---|
| `SHORTCUT_LIST` | home (no `viewAll`) | ALBUM, MIX, PLAYLIST (official: also TASK) | cards (drop TASK) |
| `HORIZONTAL_LIST` | home, artist | any | cards |
| `HORIZONTAL_LIST_WITH_CONTEXT` | home (`header` = the context item `{type, data}`) | ALBUM | cards |
| `COMPACT_GRID_CARD` | home | ALBUM, MIX, TRACK | cards |
| `TRACK_LIST` | artist (`ARTIST_TOP_TRACKS`) | TRACK | tracks |
| `ARTIST_TRACK_CREDITS_CARD` | artist (`contributorId`) | TRACK_CREDITS | skip |
| `GRID_CARD`, `PUBLIC_PLAYLIST_LIST`, `DISCOGRAPHY_TABS` (`tabs[{label, sourceType, selected, items, viewAll}]`), `LINKS_LIST`, editorial cards | official client, not seen | — | per the plan; tolerate absence |

Home `moduleId`s seen: `SHORTCUT_LIST, DAILY_MIXES, CONTINUE_LISTEN_TO, NEW_ALBUM_SUGGESTIONS, YOUR_FAVORITE_ARTISTS, NEW_TRACK_SUGGESTIONS, BECAUSE_YOU_LISTENED_TO_ALBUM, RECOMMENDED_USERS_PLAYLISTS, SUGGESTED_RADIOS_MIXES, UPLOADS_FOR_YOU` (page 1); `BASED_ON_YOUR_INTERESTS_1/2, ALBUM_RECOMMENDATIONS, SUGGESTED_ESSENTIAL_PLAYLISTS, FORGOTTEN_FAVORITES, POPULAR_PLAYLISTS, LATEST_SPOTLIGHTED_TRACKS, HISTORY_MIXES` (page 2). Artist `moduleId`s: `ARTIST_TOP_TRACKS` (TRACK_LIST), `ARTIST_ALBUMS`, `ARTIST_TOP_SINGLES`, `ARTIST_COMPILATIONS`, `ARTIST_LIVE_ALBUMS`, `ARTIST_PLAYLIST`, `ARTIST_VIDEOS` (VIDEO items), `ARTIST_SIMILAR_ARTISTS`, `ARTIST_APPEARS_ON` (all HORIZONTAL_LIST), `ARTIST_CREDITS`.

Official home item whitelist: `ARTIST, TRACK, MIX, ALBUM, PLAYLIST, DEEP_LINK, MAGAZINE` (+ `TASK` in shortcuts). View-all items: `ALBUM, ARTIST, MIX, PLAYLIST, TRACK, TRACK_CREDITS, VIDEO`. Render per item, never per module (rows mix types).

v1 page document: `{id, title, selfLink, rows:[{modules:[{id, type, title, description, preTitle, width, showMore:{apiPath,title}, pagedList:{dataApiPath, limit, offset, totalNumberOfItems, items}, …}]}]}`.

| v1 module type | payload | M2 |
|---|---|---|
| `ALBUM_LIST`, `ARTIST_LIST`, `PLAYLIST_LIST`, `MIX_LIST`, `TRACK_LIST`, `VIDEO_LIST` | `pagedList.items` = bare v1 entities | yes (video: skip) |
| `MIXED_TYPES_LIST` | `pagedList.items` = `{type, item}` | yes (official client, not seen) |
| `PAGE_LINKS`, `PAGE_LINKS_CLOUD` | `pagedList.items` = `{title, apiPath, icon, imageId}` | links |
| `HIGHLIGHT_MODULE` | `highlights[{title, item:{type, item}}]` | yes (official client, not seen) |
| `MIX_HEADER` | `mix`, `playbackControls` | mix page header |
| promotions, `ARTICLE_LIST`, `TEXT_BLOCK`, `SOCIAL`, header/credit modules | — | skip |

v1 playlists inside page modules carry `creators: []` instead of `creator`. v1 playlist `type` is one of `USER, EDITORIAL, ARTIST, PUBLIC, PODCAST`; a `creator.id` of 0 (or none) means TIDAL.

## Mix shapes

| source | title / subtitle | images | mix type |
|---|---|---|---|
| v1 `pages/mix` `MIX_HEADER.mix` | `title`, `subTitle` | `images{SMALL,MEDIUM,LARGE:{url,width,height}}` | `mixType` |
| v2 home / view-all item `data` | `titleTextInfo.text`, `subtitleTextInfo.text` (lowercase t); no flat `title` | `mixImages[{size,url,width,height}]`, `detailMixImages[…]` | `type` |
| v2 `favorites/mixes` item | `title`, `subTitle`, `titleTextInfo`, `subTitleTextInfo` (capital T) | `images{SMALL,MEDIUM,LARGE}` | `mixType` |

Sizes are always 320 / 640 / 1500 square. Mix types (official client): `ARTIST_MIX, DAILY_MIX, DISCOVERY_MIX, HISTORY_ALLTIME_MIX, HISTORY_MONTHLY_MIX, HISTORY_YEARLY_MIX, MASTER_ARTIST_MIX, MASTER_NEW_RELEASE_MIX, MASTER_TRACK_MIX, NEW_RELEASE_MIX, PRODUCER_MIX, SONGWRITER_MIX, TRACK_MIX, VIDEO_DAILY_MIX` (+ `WELCOME_MIX`); keep an `Unknown` fallback.

## Image sizes (official client, not probed)

`https://resources.tidal.com/images/<uuid with - → />/<W>x<H>.jpg`. The official builder rejects any other size.

| entity | uuid field | sizes |
|---|---|---|
| album, track (`album.cover`) | `cover` | 80, 160, 320, 640, 1280 square (+ `origin.jpg`) |
| artist | `picture`, else `selectedAlbumCoverFallback` at album sizes | 160, 320, 480, 750 square |
| playlist square | `squareImage`, else `image` | 160, 320, 480, 640, 750, 1080 square |
| playlist wide | `image` | 160x107, 320x214, 480x320, 640x428, 750x500, 1080x720 |
| video | `imageId`, else `image` | 160x90, 320x180, 480x270, 640x360, 800x450, 1280x720 |
| mix | full URL in the payload | 320 / 640 / 1500 |
| genre / mood tiles | `imageId` | genre 460x306, 2048x512; mood 342x342, 2048x330 |

## Favorites writes (official client, not probed)

All writes carry the standard query params. Form bodies are `application/x-www-form-urlencoded; charset=UTF-8`.

| action | request |
|---|---|
| add track / album / artist / video / playlist | POST v1 `users/{uid}/favorites/{tracks\|albums\|artists\|videos\|playlists}`, form `trackIds\|albumIds\|artistIds\|videoIds\|uuids=<csv>&onArtifactNotFound=FAIL`. The official client retries a failed add up to 5 times, 1 s apart. On 400/404 it re-sends with `onArtifactNotFound=SKIP`. SONE and python-tidal send singular field names (`trackId`), which also work for them. |
| remove the same | DELETE v1 `users/{uid}/favorites/{kind}/{id1,id2,…}` |
| add mix | PUT v2 `favorites/mixes/add`, form `mixIds=<csv>&onArtifactNotFound=FAIL` |
| remove mix | PUT v2 `favorites/mixes/remove?mixIds=<csv>` (no body) |
| add playlist (the official playlist UI) | PUT v2 `my-collection/playlists/folders/add-favorites?folderId=root&uuids=<csv>`, `Content-Type: application/json`, no body |
| remove playlist | PUT v2 `my-collection/playlists/folders/remove?trns=trn:playlist:<uuid>[,…]`, the same header |

The official client loads all favorite ids at login and updates the sets locally after each write. TRNs: `trn:playlist:<uuid>`, `trn:folder:<id>`, `trn:artist:<id>`.

## Public API (openapi.tidal.com/v2) as a fallback

The android token gets **200** on `dynamicPages?filter[pageType]=HOME_STATIC&filter[subject]=users:<uid>&deviceType=DESKTOP&systemType=DESKTOP&clientVersion=2026.9.30&include=modules.items` (without `filter[subject]` → 400 "Subject ID filter is required unless page type is HOME_FREE"; it returns 6 modules plus included albums, playlists, tracks and artists). It also gets 200 on `userCollectionTracks/me/relationships/items` (20 per page, cursor in `links.meta.nextCursor`), `playlists/<uuid>/relationships/items` and `playlists/<uuid>?include=coverArt`. All are `no-store` with no ETag. The documented fallback (spec §16) works with our token; M2 does not use it.

## Changes for later tasks

- **Task 2:** keep `DESKTOP`/`DESKTOP`; the `x-tidal-client-version` header is mandatory on v2. A `viewAll` path already has a query (`?itemId=…`), so append the standard params to it.
- **Task 3:** v2 home, artist and view-all responses have no ETag (`no-store`), so they can only be served stale and refetched whole. v1 pages, playlists, favorites lists, `favorites/ids`, `mixes/*/items` and the v2 flattened list honour `If-None-Match` → 304. The v2 `favorites/mixes` (and `/ids`) ETag changes on every call.
- **Task 4:** (a) `paged_all` must step `offset` by the requested `limit` and stop at `totalNumberOfItems`, never by `items.len()`; favorites pages run short mid-list. (b) Page caps: playlist items 100, v1 `pages/data` 50, v2 folders 50; favorites accept ≥ 1000. (c) `viewAll` has no leading slash. (d) No `DISCOGRAPHY_TABS` was seen: the discography arrives as `HORIZONTAL_LIST` modules, so the fixture cannot back the `discography_tabs` test (use inline JSON or drop it). (e) The bio also needs `<br/>` → newline. (f) `pages/mix` already embeds all mix tracks, and `mixes/{id}/items` ignores `limit`/`offset`, so one request suffices for `mix()`. (g) A genre "View all" (`single-module-page`) embeds 50 items; more need `pages/data` paging at ≤ 50.
- **Task 5:** in the flattened list, favorited playlists are `itemType: "PLAYLIST"`, not `FAVORITE_PLAYLIST`. Get favorited playlist ids from `flattened?includeOnly=FAVORITE_PLAYLIST` (or v1 `favorites/ids` `PLAYLIST`, which is the same set). `favorite_playlists()` (own + favorited) uses `includeOnly=` and gets both as `PLAYLIST` items. The page cap is 50.

## Fixtures

Sanitized from the raw recordings, which were deleted afterwards. Every list and shelf is cut to ≤ 3 items; the page structures (home and artist `items`, v1 `rows`) are kept whole. Top-level paged lists (`favorites_*`, `playlist_items`, `mix_items`) have `totalNumberOfItems` set to the kept count, so each fixture is one complete page. User ids → `12345`, user names → `User`, `countryCode` → `US`, user playlists → titles `Playlist A…F` and uuids `00000000-0000-4000-8000-00000000000N` (the same uuid in every fixture), user playlist descriptions → `""`. Catalog ids, titles and image uuids are kept.

| fixture | source |
|---|---|
| `home_feed.json` | v2 home page 1: 10 modules, ≤ 3 items each, cursor and vibes kept |
| `view_all.json` | v2 home view-all `NEW_ALBUM_SUGGESTIONS` |
| `explore.json`, `genre_page.json` | v1 `pages/explore`, `pages/genre_pop` |
| `artist_page.json` | v2 `artist/8847` (Daft Punk) |
| `playlist.json`, `playlist_items.json` | v1 editorial playlist "Pop Classics" and its items |
| `mix_page.json`, `mix_items.json` | v1 `pages/mix` and `mixes/{id}/items` for Daft Punk's artist mix |
| `favorites_{tracks,albums,artists}.json` | v1 favorites lists |
| `favorites_mixes.json`, `mixes_ids.json` | v2 `favorites/mixes`, `favorites/mixes/ids` |
| `playlists_flattened.json` | v2 flattened list, `includeOnly=` (own and favorited playlists) |
| `favorites_ids.json` | v1 `favorites/ids` (its `PLAYLIST` uuids match `playlists_flattened.json`) |
