# S5: TIDAL editing endpoints

Live probing on 2026-10-10 with the human's subscribed account (country NG) and the `android` client token (scopes `r_usr w_usr w_sub`), using the recorder, which now sends writes: `cargo run -p tidal --example record -- <v1|v2|openapi> <path?query> <name> [--method M] [--form k=v]… [--json '<body>'] [--header 'k: v']…` (`crates/tidal/examples/record.rs`). Fixtures: `crates/tidal/tests/fixtures/` (sanitized, see the end).

Every write named only a playlist or folder created in the same run: the playlist `Tidash probe`, the folder `Tidash probe folder` and two subfolders. All of them were deleted. Afterwards the account's own-playlist list (v1 `users/{uid}/playlists`) was byte-identical to before, and the root folder listing held the same items.

What the official client does comes from reading the official desktop web client builds 2026.09.30 (the one Tidash mirrors) and 2026.10.08, python-tidal and SONE, without network calls. Rows marked *official client, not probed* rest on that reading alone.

Test ids, all Daft Punk because they resolve in NG: track 20115564 "Get Lucky" (no lyrics in NG), 1550546 "One More Time", 20115561 "Instant Crush", album 20115556 *Random Access Memories*, artist 8847. Track 283039788 (Mahler) has no mix.

## Request conventions for writes

| | v1 `api.tidal.com/v1/` | v2 `api.tidal.com/v2/` | openapi `openapi.tidal.com/v2/` |
|---|---|---|---|
| method, body | POST/DELETE, form `application/x-www-form-urlencoded` (the official client adds `; charset=UTF-8`, reqwest's `.form` does not; both work) | `PUT`, no body: `Content-Type: application/json`, `Content-Length: 0`, everything in the query | POST/PATCH, JSON:API body, `Content-Type: application/vnd.api+json` |
| headers | `If-None-Match: <playlist ETag>` on item writes | `x-tidal-client-version` | `Accept: application/vnd.api+json`, `Idempotency-Key: <UUID v4>` |
| query | standard v1 params | standard v2 params | `countryCode` |
| success | 200: JSON on add; **empty body** on move and remove; a new `etag` header | 200 + JSON on `create-folder`; **204, empty** on the rest | 201 + `data` on create; 200 + `data` on PATCH |

Write responses carry `cache-control: must-revalidate, no-transform` (v1) or `max-age=0, proxy-revalidate` (v2).

## Operations

| # | request | status | answer |
|---|---|---|---|
| 1 | openapi POST `playlists`, `{"data":{"type":"playlists","attributes":{"name":"Tidash probe","description":"","accessType":"UNLISTED"}}}` | 201 | **The android token can write to openapi.** Body `{data:{id, type:"playlists", attributes:{name, description, bounded, externalLinks[{href, meta.type}], createdAt, lastModifiedAt, accessType, playlistType:"USER", numberOfFollowers}}, links:{self}}`, plus `Location: /playlists/<uuid>`. The new uuid is `data.id`. No ETag. |
| 2 | v1 GET `playlists/{uuid}` | 200 | `ETag: "<epoch ms>"`, the playlist's `lastUpdated` (`"1791588769114"` ↔ `…23:32:49.114`). `/items` carries the same ETag. `cache-control: no-cache`. |
| 3 | v1 POST `playlists/{uuid}/items`, form `onArtifactNotFound=FAIL&onDupes=FAIL&trackIds=20115564,1550546,20115561`, `If-None-Match` | 200 | `{lastUpdated: <epoch ms>, addedItemIds: [20115564, 1550546, 20115561]}` and an `etag` equal to the next GET's. Item `index` is a sparse sort key (100000, 200000, 300000), not a position. |
| 4 | the same with 1550546 again, `onDupes=FAIL` | **409** | `{status:409, subStatus:7008, userMessage:"One or more item(s) already exists in the playlist."}` |
| 5 | the same with `onDupes=SKIP` | 200 | `{lastUpdated, addedItemIds: []}`: the dupes are left out. **Its `etag` header is not the playlist's ETag.** Nothing changed, a GET still returns the old ETag, and a write sent with this one gets 412. |
| 6 | add with `toIndex=1` | 200 | Inserted before position 1 (`index` 250000, between 200000 and 300000). |
| 7 | v1 POST `playlists/{uuid}/items/0`, form `toIndex=2`, on [A, B, C] | 200 | **[B, C, A]**: `toIndex` is the position after the moved rows are taken out (a slot counted before removal would give [B, A, C]). Empty body, new `etag`. Then `items/2` with `toIndex=0` gave [A, B, C]. |
| 8 | `items/0` with `toIndex=3` on 3 items | 200 | Clamped: the row moved to the end ([B, C, A]). |
| 9 | v1 DELETE `playlists/{uuid}/items/0?order=INDEX&orderDirection=DESC` on [B, C, A] | 200 | A was removed: **positions follow `order`/`orderDirection`**. Empty body (200, not 204), new `etag`. |
| 10 | DELETE `items/1?order=INDEX&orderDirection=ASC`; later `items/0,2` on [B, A, C] | 200 | The second row was removed. `0,2` removed B and C: the positions refer to the list before removal. |
| 11 | add with the first (stale) ETag, and with the ETag from row 5 | **412** | `{status:412, subStatus:7002, userMessage:"You must send the correct Etag value in the If-None-Match header to modify a playlist"}`. Nothing changed. |
| 12 | add without `If-None-Match`; with `If-None-Match: ""` | **412** | The same 7002 (not 428). |
| 13 | add with `If-None-Match: *` | 200 | **Accepted: the wildcard skips the check.** Never send it. |
| 14 | openapi PATCH `playlists/{uuid}`, `{"data":{"id":"<uuid>","type":"playlists","attributes":{"name":…,"description":…}}}` | 200 | Renamed: v1 GET shows the new title and description, and `""` clears the description. **The response body is the playlist before the PATCH** (old name, old `lastModifiedAt`), so don't read it back. The PATCH changes the v1 ETag (`lastUpdated`). |
| 15 | v2 PUT `my-collection/playlists/folders/create-folder?folderId=root&name=Tidash%20probe%20folder&trns=` | 200 | A **top-level folder item** (not wrapped in `data`; see Folders). The id is `data.id` (a UUID), also the `trn` suffix. `parent: null`. |
| 16 | v2 PUT `…/folders/move?folderId=<id>&trns=trn:playlist:<uuid>` | 204 | Empty. The playlist leaves root; its item's `addedAt` in the folder is the time of the move. |
| 17 | v2 GET `…/folders?folderId=<id>&includeOnly=&order=DATE&orderDirection=DESC&limit=50&offset=0&cursor=` | 200 | `{items, lastModifiedAt, totalNumberOfItems}`, with **no `cursor` key** (root has none either). Items have a `parent` object (see Folders). ETag. |
| 18 | v2 PUT `create-folder?folderId=<id>&…`, and again with `folderId=trn:folder:<id>` | 200 | **Both landed at root**: `create-folder` ignores a non-root `folderId`. |
| 19 | v2 PUT `move?folderId=<id>&trns=trn:folder:<sub>` | 204 | Nesting works by move: the folder's listing then held the subfolder and the playlist. |
| 20 | v2 PUT `…/folders/rename?name=<urlencoded>&trn=trn:folder:<id>` | 204 | Empty. The root listing shows the new name in `name` and `data.name`. |
| 21 | v2 PUT `…/folders/remove?trns=trn:folder:<a>,trn:folder:<b>` | 204 | Both deleted: a csv of trns works. Listing a deleted folder's id afterwards gives 200 with 0 items. |
| 22 | v2 PUT `move?folderId=root&trns=trn:playlist:<uuid>`, then `remove?trns=trn:folder:<id>` | 204, 204 | The playlist is back at root and the folder is gone. |
| 23 | v2 PUT `…/folders/remove?trns=trn:playlist:<uuid>` | 204 | Afterwards v1 GET `playlists/{uuid}` gives **404** subStatus 2001 "Playlist not found", and openapi GET gives 404 `{errors:[{code:"NOT_FOUND", detail}]}`. The playlist is deleted, not just dropped from the collection. |
| 24 | v2 GET `…/folders/flattened?includeOnly=` with the folder present | 200 | The list holds **FOLDER items** and the playlists inside folders (with their `parent`). M2's `favorite_playlists()` already drops FOLDER items. |

## The nine open questions of the M4 research

1. **Openapi write scope:** yes, create gave 201 and PATCH gave 200. The python-tidal fallbacks (`PUT v2 …/folders/create-playlist`, `POST v1 playlists/{uuid}` form `title&description`, `PUT v2 playlists/{uuid}/set-public|set-private`) were not needed and were not tried.
2. **Openapi response:** create gives 201 with `data` (and `Location`); PATCH gives 200 with `data`, but that `data` is the pre-PATCH state. Neither gave 204, but keep treating 204 as success (SONE saw one on PATCH). `countryCode` was sent on both, as the constraints require; calls without it were not tried.
3. **`If-None-Match`:** without it → 412 (7002), not 428. `""` → 412. **`*` → 200** (the check is skipped). A stale ETag → 412 7002. Every write that changes the playlist returns a fresh `etag` equal to the next GET's. A SKIP add that adds nothing returns an etag the server rejects.
4. **Duplicates:** `onDupes=FAIL` → 409 subStatus 7008 (body in row 4). `SKIP` → 200, with `addedItemIds` leaving out the dupes (here `[]`).
5. **Move:** confirmed. `toIndex` is the position after removal; past the end it clamps.
6. **Remove:** the positions follow `order`/`orderDirection`; the status is 200 with an empty body.
7. **Delete via `folders/remove?trns=trn:playlist:<uuid>`:** it really deletes (v1 and openapi both give 404).
8. **Folders:**
   - `create-folder` returns a top-level folder item.
   - `rename`/`move`/`remove` return 204 with an empty body.
   - An item in a folder has `parent: {id, name, atRoot: false, parentId, parentIdAsNodeId, parentIdAsReferenceId: "trn:folder:<id>", parentIdAsTidalResourceName: "trn:folder:<id>"}`; at root it is `null`.
   - `create-folder` cannot nest; `move` can.
   - The live FOLDER item shape is under Folders.
9. **A track with plain lyrics but no sync:** not found. All 11 tracks with lyrics (One More Time and 10 more from *Random Access Memories*, the 10-try limit) had synced `subtitles`, and every line matched `^\[\d{2}:\d{2}\.\d{2}\] `. Keep treating `""`, null and absent as "unsynced".

## Lyrics

- **v1 `GET tracks/{id}/lyrics`** (python-tidal and SONE use it; the official client gets the same Musixmatch data as an openapi `include=lyrics` on the track).
- 200 `{trackId, lyricsProvider: "MUSIXMATCH", providerCommontrackId, providerLyricsId, lyrics, subtitles, isRightToLeft}`, `cache-control: max-age=3600, s-maxage=3600`, no ETag.
- `subtitles` is LRC with **centisecond** precision: `[mm:ss.xx] text`, one cue per `\n` line, with **a space after `]`** (openapi `lrcText` has none). Empty cues such as `[00:33.83] ` are instrumental gaps (1–7 per track here). seconds = `mm*60 + ss + xx/100`.
- **No lyrics → 404 with a non-TIDAL body** (no `subStatus`): `{"timestamp", "status":404, "error":"Not Found", "path":"/lyrics/public/<id>"}`. Treat any 404 here as "no lyrics", whatever its body.
- `lyrics` is the plain text, `\n`-separated with blank lines between stanzas.
- `isRightToLeft` drives the text direction. The official client shows the Musixmatch logo for third-party lyrics. *Official client, not probed.*

## Credits

- **Per track:** v1 `GET tracks/{id}/credits?limit=100&includeContributors=true` → a **bare JSON array** `[{type: "Producer", contributors: [{name, id}]}]` (17 role groups for Get Lucky), `cache-control: public, max-age=300`, ETag.
- **Per album:** v1 `GET albums/{id}/items/credits?replace=true&includeContributors=true&limit=100&offset=N` → `{limit, offset, totalNumberOfItems, items: [{type: "track", item: <v1 track>, credits: [{type, contributors}]}]}`. Re-recorded here (200, 13 items, `cache-control: public, max-age=300`, ETag). From the research GETs of 2026-10-10: the max page is 100 (`limit=101` → 400 subStatus 1001), and without `includeContributors=true` about a third of the contributors come back with no `id`.
- Roles seen: Producer, Co-Producer, Composer, Lyricist, Arranger, Conductor, Vocal, Associated Performer, Bass, Drums, Guitar, Steel Guitar, Keyboards, Piano, Synthesizer, Percussion, Engineer, Assistant Engineer, Recording Engineer, Mixing Engineer, Mastering Engineer, Consultant, Orchestra. Show `type` verbatim.
- The official now-playing panel uses openapi `tracks/{id}/relationships/credits?include=credits,credits.artist`, which pages by 20. Skip it.

## Radio (mix ids)

The official client has no `/radio` call. **Radio is the mix page** (v1 `pages/mix?mixId=`, built in M2) for the item's mix id.

| payload | mix id |
|---|---|
| v1 `tracks/{id}`, `albums/{id}/items`, `playlists/{uuid}/items`, favorite tracks | `mixes.TRACK_MIX` (`mixes: {}` or absent = none) |
| v2 artist page | `item.data.mixes.ARTIST_MIX`; top-track items `items[].data.mixes.TRACK_MIX` |
| v1 `GET tracks/{id}/mix` | `{"id": "<mixId>"}` (`cache-control: must-revalidate`); no mix → 404 subStatus 2001 "TrackMixId for mixId: [id] not found" |
| v1 `GET artists/{id}/mix` | `{"id": "<mixId>"}`, the same id as the artist page's `ARTIST_MIX` |

- "Go to track radio" is disabled without `TRACK_MIX`; "Go to artist radio" is shown only with `ARTIST_MIX`. Autoplay at the queue's end plays `tracks/{lastId}/mix`. *Official client, not probed.*
- v1 `tracks/{id}/radio` and `artists/{id}/radio` (python-tidal) still return 200 with a paged track list, but the official client doesn't use them.

## Playlists

**ETag rules (rows 2–13).**
- The ETag is `"<lastUpdated as epoch ms>"` from v1 GET `playlists/{uuid}` (or `/items`).
- Item writes send it as **`If-None-Match`** (not If-Match).
- Missing, `""` or stale → 412 subStatus 7002. `*` is accepted, which defeats the guard.
- A write that changes the playlist returns the next ETag. A SKIP add that adds nothing returns a bogus one.
- An openapi PATCH also moves the ETag.
- Taking a fresh ETag from an uncached GET before each write is always right.

**Positions.**
- Remove and move address 0-based positions in the order given by `order=INDEX&orderDirection=ASC`, never the item's `index` field (a sparse sort key).
- For a move from position `from` to the slot `to` (`0..=len`, counted before the move), send `toIndex = if to > from { to - 1 } else { to }`. That is the position after the row is taken out (row 7), as the official client computes it.
- `toIndex` on an add inserts before that position; without it the tracks are appended.

**Own playlists ("Add to playlist").**
- v1 `GET users/{uid}/playlists?order=DATE_UPDATED&orderDirection=DESC&limit=50&offset=N` → `{limit, offset, totalNumberOfItems, items: [v1 playlist]}`, `cache-control: no-cache`, ETag.
- Live, it returns exactly the user's own playlists (3), with `creator: {id}` only and `publicPlaylist`.
- **Own = `creator.id == uid`.** `type: "USER"` is not enough, because favorited playlists by other users are `USER` too. The root folder listing and the flattened list hold own **and** favorited playlists (live: 3 own + 2 favorited at root).

**Visibility markers.** v1 `publicPlaylist`; v2 folder item `data.sharingLevel` `PRIVATE|PUBLIC`; openapi `attributes.accessType` `UNLISTED|PUBLIC`. Visibility changes were not probed (the probe stayed UNLISTED).

**Newer official client (2026.10.08), not used.** Item writes on openapi `playlists/{uuid}/relationships/items`, keyed by `meta.itemId` (= v1 `itemUuid`), with no ETag. *Official client, not probed.*

## Folders

Item shapes (live):

```
FOLDER   {trn: "trn:folder:<id>", itemType: "FOLDER", addedAt, lastModifiedAt, name, parent,
          data: {trn, name, createdAt, lastModifiedAt, totalNumberOfItems, id, itemType: "FOLDER"}}
PLAYLIST {trn: "trn:playlist:<uuid>", itemType: "PLAYLIST", addedAt, lastModifiedAt, name, parent,
          data: <v1 playlist> + {trn, itemType, sharingLevel, curators, creator{id, name, picture, type}, …}}
parent   null at root; in a folder {id, name, atRoot: false, parentId, parentIdAsNodeId,
          parentIdAsReferenceId: "trn:folder:<id>", parentIdAsTidalResourceName: "trn:folder:<id>"}
```

- Folder ids are UUIDs. Listings take the raw id (`folderId=<id>`); writes take TRNs (`trn=`, `trns=`). `move` takes a raw `folderId` or `root`.
- A listing is `{items, lastModifiedAt, totalNumberOfItems}` with no `cursor` key at `limit=50`; the max page is 50 (S4). The folder's `data.totalNumberOfItems` counts its direct children.
- `create-folder` puts the folder at root whatever `folderId` says. To make a subfolder, create it and then `move?folderId=<parent>&trns=trn:folder:<new>`.
- The official client sends `trns=` empty on `create-folder` when nothing moves in at creation. *Official client, not probed:* it batches ≤ 20 trns per `move`.
- Quirks: listing a deleted folder's id gives 200 with no items. An unknown non-UUID `folderId` gives the root's contents with the root's ETag (research, 2026-10-10). `includeOnly=FOLDER` keeps the unfiltered `totalNumberOfItems` (S4).
- Delete a folder only once it is empty: the probe always moved its contents out first, and what `remove` does to a non-empty folder's playlists was not tried.

## Endpoints for M4

Mirror official client 2026.09.30. v1 for items (ETag), v2 for folders and delete, openapi only for create and edit.

| action | request |
|---|---|
| lyrics | v1 `GET tracks/{id}/lyrics`; 404 = none |
| track credits | v1 `GET tracks/{id}/credits?limit=100&includeContributors=true` (bare array) |
| album credits | v1 `GET albums/{id}/items/credits?replace=true&includeContributors=true&limit=100&offset=N` |
| track / artist radio | `mixes.TRACK_MIX` / `ARTIST_MIX`, else v1 `GET tracks\|artists/{id}/mix` → `{id}` (404 = none) → v1 `pages/mix?mixId=` |
| own playlists | v1 `GET users/{uid}/playlists?order=DATE_UPDATED&orderDirection=DESC&limit=50&offset=N` |
| create playlist | openapi `POST playlists`, `{data:{type:"playlists", attributes:{name, description, accessType:"UNLISTED"}}}` → 201, `data.id`; in a folder, then v2 `move` |
| rename / description | openapi `PATCH playlists/{uuid}`, `{data:{id, type:"playlists", attributes:{name, description}}}` → 200 (ignore the body) |
| delete playlist | v2 `PUT my-collection/playlists/folders/remove?trns=trn:playlist:<uuid>` → 204 |
| ETag | v1 `GET playlists/{uuid}` → `ETag` |
| add tracks | v1 `POST playlists/{uuid}/items`, form `onArtifactNotFound=FAIL&onDupes=FAIL\|SKIP\|ADD&trackIds=<csv ≤ 500>[&toIndex=N]`, `If-None-Match` → `{lastUpdated, addedItemIds}`; 409 (7008) = dupes |
| remove items | v1 `DELETE playlists/{uuid}/items/{pos,…}?order=INDEX&orderDirection=ASC`, `If-None-Match` → 200, empty |
| move items | v1 `POST playlists/{uuid}/items/{pos,…}`, form `toIndex=<position after removal>`, `If-None-Match` → 200, empty |
| list a folder | v2 `GET my-collection/playlists/folders?folderId=<id\|root>&includeOnly=&order=DATE&orderDirection=DESC&limit=50&offset=0&cursor=` |
| create folder | v2 `PUT …/folders/create-folder?folderId=root&name=<name>&trns=` → 200, folder item, `data.id` |
| rename folder | v2 `PUT …/folders/rename?name=<name>&trn=trn:folder:<id>` → 204 |
| delete folder | v2 `PUT …/folders/remove?trns=trn:folder:<id>` → 204 |
| move to folder / root | v2 `PUT …/folders/move?folderId=<id\|root>&trns=<csv of trn:playlist:… or trn:folder:…>` → 204 |

## Changes for later tasks

- **Task 2:** no change. `lyrics.json` has 6 cues, two of them empty (gaps); `lyrics_404.json` is the non-TIDAL 404 body; `track.json` carries `mixes.TRACK_MIX`; the S4 `artist_page.json` already carries `item.data.mixes.ARTIST_MIX` (the same id `artists/8847/mix` returns).
- **Task 3:**
  - (a) Openapi writes work, so create and edit go through `json_api` as planned, with no fallback.
  - (b) PATCH answers 200 with a stale `data`: `edit_playlist` ignores the body.
  - (c) v1 move and remove answer **200 with an empty body**; v2 folder PUTs answer 204, except `create-folder` (200 + item). Nothing but add and `create-folder` should parse a body.
  - (d) Take the ETag fresh before every write, as planned. Never reuse the `etag` of an add whose `addedItemIds` is empty, and never send `*` or `""`.
  - (e) `add_to_playlist` returns the sum of `addedItemIds.len()`. With `SKIP` there is no 409.
  - (f) `move_in_playlist`'s `toIndex = if to > from { to - 1 } else { to }` is confirmed.
  - (g) **`create_folder(name, Some(parent))` must create at root and then `move?folderId=<parent>&trns=trn:folder:<new id>`**, because `create-folder` ignores the parent. The signature is unchanged.
  - (h) `folder()` sees no `cursor` key: absent means the end.
  - (i) The 412 body (subStatus 7002) and the 409 body (7008) are in the Operations table for the wiremock tests.
- **Task 7:** the root listing and the flattened list mix own and favorited playlists, and the flattened list also holds folders and in-folder playlists. A tree of "folders plus own playlists" filters on `creator.id == uid`.

## Fixtures

Sanitized from the raw recordings, which were deleted afterwards.
- Lists are cut to ≤ 3 items and lyrics to 6 cues (and the first 6 lines of `lyrics`). Paged lists have `totalNumberOfItems` set to the kept count.
- The account's user id → `12345`; other users' ids → `67890` (S4 mapped both to 12345, which hides the own/favorited split). User names → `User`; `countryCode` → `US`; user playlist descriptions → `""`.
- Playlists keep S4's placeholders where S4 already named them (`…0002` Playlist B, `…0003` Playlist C). The two own playlists S4 did not have are `…0007` Playlist G and `…0008` Playlist H.
- The probe playlist is `00000000-0000-4000-8000-000000000001` and its folder is `f0000000-0000-4000-8000-000000000001`, keeping their titles. In S4's `playlists_flattened.json` and `favorites_ids.json`, `…0001` is Playlist A. `folder_root.json` drops the root's first item (Playlist A) so that no M4 fixture gives `…0001` to two playlists.
- Catalog ids, titles and image uuids are kept.

| fixture | source |
|---|---|
| `lyrics.json` | v1 `tracks/1550546/lyrics` (One More Time) |
| `lyrics_404.json` | v1 `tracks/20115564/lyrics` (Get Lucky, none in NG): the 404 body |
| `track_credits.json` | v1 `tracks/20115564/credits?limit=100&includeContributors=true` |
| `track.json` | v1 `tracks/20115564`, with `mixes.TRACK_MIX` |
| `own_playlists.json` | v1 `users/{uid}/playlists?order=DATE_UPDATED&orderDirection=DESC&limit=50`: 3 own playlists |
| `folder_root.json` | v2 root listing before the probe: items 2–4 (2 own, 1 favorited) |
| `folder_items.json` | v2 listing of the probe folder (the probe playlist, with `parent`), plus the FOLDER item as the root listing showed it (`parent: null`, `totalNumberOfItems: 1`); `totalNumberOfItems` 2 |
| `openapi_playlist_created.json` | openapi `POST playlists` (201) |
