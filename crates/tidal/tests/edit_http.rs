#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::sync::Arc;
use tidal::{Client, ClientConfig, DiskCache, Error, FolderItem, MemoryStore, Session};
use wiremock::matchers::{
    body_json, body_string, header, header_exists, method, path, query_param,
    query_param_is_missing,
};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

const OWN: &str = include_str!("fixtures/own_playlists.json");
const ROOT: &str = include_str!("fixtures/folder_root.json");
const FOLDER: &str = include_str!("fixtures/folder_items.json");
const CREATED: &str = include_str!("fixtures/openapi_playlist_created.json");
const PLAYLIST_ITEMS: &str = include_str!("fixtures/playlist_items.json");

/// The S5 probe playlist, its folder and subfolder.
const UUID: &str = "00000000-0000-4000-8000-000000000001";
const FOLDER_ID: &str = "f0000000-0000-4000-8000-000000000001";
const SUBFOLDER_ID: &str = "f0000000-0000-4000-8000-000000000002";
const FOLDERS: &str = "/v2/my-collection/playlists/folders";
const JSON_API: &str = "application/vnd.api+json";

fn setup(server: &MockServer) -> Client {
    client_with(server, None)
}

fn client_with(server: &MockServer, cache: Option<Arc<DiskCache>>) -> Client {
    let url = |p: &str| format!("{}/{p}", server.uri()).parse().unwrap();
    let cfg = ClientConfig {
        api_base: url("v1/"),
        api_v2_base: url("v2/"),
        openapi_base: url("openapi/v2/"),
        auth_base: url("v1/"),
        ..ClientConfig::default()
    };
    let session = Session {
        access_token: "ACCESS".into(),
        refresh_token: "REFRESH".into(),
        expires_at: u64::MAX,
        user_id: 12345,
        country_code: "US".into(),
    };
    Client::new(cfg, session, Arc::new(MemoryStore::default()), cache).unwrap()
}

fn json_body(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

/// A TIDAL error body, as S5 recorded the 412 (7002).
fn stale_etag() -> ResponseTemplate {
    ResponseTemplate::new(412).set_body_json(json!({
        "status": 412, "subStatus": 7002,
        "userMessage": "You must send the correct Etag value in the If-None-Match header to modify a playlist"
    }))
}

/// The ETag GET answers `etag` for the next `times` requests.
async fn mock_etag(server: &MockServer, etag: &str, times: u64) {
    Mock::given(method("GET"))
        .and(path(format!("/v1/playlists/{UUID}")))
        .and(query_param("countryCode", "US"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", etag)
                .set_body_json(json!({"uuid": UUID, "title": "Tidash probe"})),
        )
        .up_to_n_times(times)
        .expect(times)
        .mount(server)
        .await;
}

/// A v1 item write behind `etag`.
fn item_write(verb: &str, rest: &str, etag: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(format!("/v1/playlists/{UUID}/items{rest}")))
        .and(header("authorization", "Bearer ACCESS"))
        .and(header("if-none-match", etag))
        .and(query_param("countryCode", "US"))
}

fn added(n: usize) -> ResponseTemplate {
    let ids: Vec<u64> = (0..n as u64).collect();
    ResponseTemplate::new(200)
        .insert_header("etag", "\"bogus\"")
        .set_body_json(json!({"lastUpdated": 1, "addedItemIds": ids}))
}

fn add_form(ids: &str) -> String {
    format!("onArtifactNotFound=FAIL&onDupes=SKIP&trackIds={ids}")
}

/// An openapi JSON:API request.
fn json_api(verb: &str, p: &str) -> MockBuilder {
    Mock::given(method(verb))
        .and(path(format!("/openapi/v2/{p}")))
        .and(header("authorization", "Bearer ACCESS"))
        .and(header("accept", JSON_API))
        .and(header("content-type", JSON_API))
        .and(header_exists("idempotency-key"))
        .and(query_param("countryCode", "US"))
        .and(query_param_is_missing("locale"))
        .and(query_param_is_missing("deviceType"))
}

/// A v2 folder PUT with no body.
fn folders_put(action: &str) -> MockBuilder {
    Mock::given(method("PUT"))
        .and(path(format!("{FOLDERS}/{action}")))
        .and(header("authorization", "Bearer ACCESS"))
        .and(header("x-tidal-client-version", "2026.9.30"))
        .and(header("content-type", "application/json"))
        .and(query_param("countryCode", "US"))
        .and(body_string(""))
}

async fn once(mock: MockBuilder, resp: ResponseTemplate, server: &MockServer) {
    mock.respond_with(resp).expect(1).mount(server).await;
}

fn no_content() -> ResponseTemplate {
    ResponseTemplate::new(204)
}

#[tokio::test]
async fn add_sends_skip_and_a_fresh_etag() {
    let server = MockServer::start().await;
    mock_etag(&server, "\"100\"", 1).await;
    let mock = item_write("POST", "", "\"100\"")
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string(add_form("1%2C2")));
    once(mock, added(1), &server).await;
    let n = setup(&server).add_to_playlist(UUID, &[1, 2]).await.unwrap();
    assert_eq!(n, 1, "a dupe is skipped");
}

#[tokio::test]
async fn add_sends_500_ids_per_request_each_behind_its_own_etag() {
    let server = MockServer::start().await;
    mock_etag(&server, "\"100\"", 1).await;
    mock_etag(&server, "\"200\"", 1).await;
    let ids: Vec<u64> = (1..=501).collect();
    let first: Vec<String> = ids[..500].iter().map(u64::to_string).collect();
    let mock = item_write("POST", "", "\"100\"").and(body_string(add_form(&first.join("%2C"))));
    once(mock, added(500), &server).await;
    let mock = item_write("POST", "", "\"200\"").and(body_string(add_form("501")));
    once(mock, added(1), &server).await;
    assert_eq!(
        setup(&server).add_to_playlist(UUID, &ids).await.unwrap(),
        501
    );
}

#[tokio::test]
async fn add_retries_once_on_412() {
    let server = MockServer::start().await;
    mock_etag(&server, "\"100\"", 1).await;
    mock_etag(&server, "\"200\"", 1).await;
    once(item_write("POST", "", "\"100\""), stale_etag(), &server).await;
    once(item_write("POST", "", "\"200\""), added(2), &server).await;
    let n = setup(&server).add_to_playlist(UUID, &[1, 2]).await.unwrap();
    assert_eq!(n, 2);
}

#[tokio::test]
async fn add_gives_up_after_one_retry() {
    let server = MockServer::start().await;
    mock_etag(&server, "\"100\"", 2).await;
    item_write("POST", "", "\"100\"")
        .respond_with(stale_etag())
        .expect(2)
        .mount(&server)
        .await;
    let err = setup(&server)
        .add_to_playlist(UUID, &[1])
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Status {
            status: 412,
            sub_status: Some(7002),
            ..
        }
    ));
}

/// `*` would skip TIDAL's check and `""` always fails it (S5): neither is sent, whether the
/// GET gave it (add) or the caller did (remove, move).
#[tokio::test]
async fn a_wildcard_or_empty_etag_is_never_sent() {
    for etag in ["*", "\"\"", ""] {
        let server = MockServer::start().await;
        mock_etag(&server, etag, 1).await;
        let client = setup(&server);
        assert!(client.add_to_playlist(UUID, &[1]).await.is_err());
        assert!(client.remove_from_playlist(UUID, etag, &[0]).await.is_err());
        assert!(client.move_in_playlist(UUID, etag, 0, 2).await.is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "{etag}");
    }
}

/// The page's positions belong to the listing it came from: a fresh ETag would let a stale
/// position through, and remove whatever now sits there.
#[tokio::test]
async fn remove_uses_the_listing_etag_not_a_fresh_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/v1/playlists/{UUID}")))
        .respond_with(ResponseTemplate::new(200).insert_header("etag", "\"L1\""))
        .expect(0)
        .mount(&server)
        .await;
    once(item_write("DELETE", "/1", "\"L0\""), stale_etag(), &server).await;
    let err = setup(&server)
        .remove_from_playlist(UUID, "\"L0\"", &[1])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Status { status: 412, .. }));
}

#[tokio::test]
async fn remove_sends_positions_in_index_order() {
    let server = MockServer::start().await;
    let mock = item_write("DELETE", "/0,2", "\"100\"")
        .and(query_param("order", "INDEX"))
        .and(query_param("orderDirection", "ASC"));
    once(mock, ResponseTemplate::new(200), &server).await;
    setup(&server)
        .remove_from_playlist(UUID, "\"100\"", &[0, 2])
        .await
        .unwrap();
}

#[tokio::test]
async fn remove_on_412_is_not_retried() {
    let server = MockServer::start().await;
    once(item_write("DELETE", "/1", "\"100\""), stale_etag(), &server).await;
    let err = setup(&server)
        .remove_from_playlist(UUID, "\"100\"", &[1])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Status { status: 412, .. }));
}

#[tokio::test]
async fn move_sends_index_after_removal() {
    let server = MockServer::start().await;
    for (from, to_index) in [(0, 2), (2, 0)] {
        let mock = item_write("POST", &format!("/{from}"), "\"100\"")
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string(format!("toIndex={to_index}")));
        once(mock, ResponseTemplate::new(200), &server).await;
    }
    let client = setup(&server);
    client
        .move_in_playlist(UUID, "\"100\"", 0, 3)
        .await
        .unwrap();
    client
        .move_in_playlist(UUID, "\"100\"", 2, 0)
        .await
        .unwrap();
}

#[tokio::test]
async fn move_on_412_is_not_retried() {
    let server = MockServer::start().await;
    once(item_write("POST", "/0", "\"100\""), stale_etag(), &server).await;
    let err = setup(&server)
        .move_in_playlist(UUID, "\"100\"", 0, 2)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Status { status: 412, .. }));
}

#[tokio::test]
async fn playlist_items_keep_positions_past_videos() {
    let server = MockServer::start().await;
    let mut items: Value = serde_json::from_str(PLAYLIST_ITEMS).unwrap();
    items["items"][1] = json!({"type": "video", "item": {"id": 9, "title": "A video"}});
    Mock::given(method("GET"))
        .and(path("/v1/playlists/p/items"))
        .and(query_param("order", "INDEX"))
        .and(query_param("orderDirection", "ASC"))
        .respond_with(ResponseTemplate::new(200).set_body_json(items))
        .expect(1)
        .mount(&server)
        .await;
    let items = setup(&server).playlist_items("p").await.unwrap();
    let ids: Vec<u64> = items.tracks.iter().map(|t| t.id).collect();
    assert_eq!(ids, [10582679, 555115]);
    assert_eq!(items.positions, [0, 2]);
    assert_eq!(items.etag, None, "TIDAL sent none");
}

/// The first page's ETag: from a 200, then the saved copy's on a 304 and when cache-only.
#[tokio::test]
async fn playlist_items_return_the_listing_etag() {
    let server = MockServer::start().await;
    let items = |offset: usize| {
        Mock::given(method("GET"))
            .and(path("/v1/playlists/p/items"))
            .and(query_param("offset", offset.to_string()))
    };
    // Mounted first, so it answers the revalidation; its 304 carries no ETag.
    once(
        items(0).and(header("if-none-match", "\"L0\"")),
        ResponseTemplate::new(304),
        &server,
    )
    .await;
    let mut first: Value = serde_json::from_str(PLAYLIST_ITEMS).unwrap();
    first["totalNumberOfItems"] = 101.into();
    let first = ResponseTemplate::new(200)
        .insert_header("etag", "\"L0\"")
        .set_body_json(first);
    once(items(0), first, &server).await;
    // A later page has its own ETag (the playlist changed while paging): the first one stays.
    let rest = json!({"limit": 100, "offset": 100, "totalNumberOfItems": 101, "items": []});
    let rest = ResponseTemplate::new(200)
        .insert_header("etag", "\"L1\"")
        .set_body_json(rest);
    items(100).respond_with(rest).expect(2).mount(&server).await;
    let dir = std::env::temp_dir()
        .join("tidal-edit-http")
        .join("listing-etag");
    let _ = std::fs::remove_dir_all(&dir);
    let cache = Arc::new(DiskCache::open(dir, 1 << 20, "json").unwrap());
    let client = client_with(&server, Some(cache));
    let fresh = client.playlist_items("p").await.unwrap();
    assert_eq!(fresh.etag.as_deref(), Some("\"L0\""));
    assert_eq!(fresh.tracks.len(), 3);
    let revalidated = client.playlist_items("p").await.unwrap();
    assert_eq!(revalidated, fresh);
    let offline = client.cache_only().playlist_items("p").await.unwrap();
    assert_eq!(offline.etag.as_deref(), Some("\"L0\""));
}

#[tokio::test]
async fn own_playlists_paged() {
    let server = MockServer::start().await;
    let mut first: Value = serde_json::from_str(OWN).unwrap();
    first["totalNumberOfItems"] = 51.into();
    let second = json!({"limit": 50, "offset": 50, "totalNumberOfItems": 51,
        "items": [{"uuid": "x", "title": "Playlist X", "creator": {"id": 12345}}]});
    for (offset, page) in [(0, first), (50, second)] {
        Mock::given(method("GET"))
            .and(path("/v1/users/12345/playlists"))
            .and(query_param("order", "DATE_UPDATED"))
            .and(query_param("orderDirection", "DESC"))
            .and(query_param("limit", "50"))
            .and(query_param("offset", offset.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .expect(1)
            .mount(&server)
            .await;
    }
    let playlists = setup(&server).own_playlists().await.unwrap();
    let titles: Vec<&str> = playlists.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Playlist B", "Playlist G", "Playlist H", "Playlist X"]
    );
    assert!(playlists.iter().all(|p| p.creator_id == Some(12345)));
}

/// The v2 listing of `folder` at `offset`.
fn listing(folder: &str, offset: usize) -> MockBuilder {
    Mock::given(method("GET"))
        .and(path(FOLDERS))
        .and(query_param("folderId", folder))
        .and(query_param("includeOnly", ""))
        .and(query_param("order", "DATE"))
        .and(query_param("orderDirection", "DESC"))
        .and(query_param("limit", "50"))
        .and(query_param("offset", offset.to_string()))
        .and(header("x-tidal-client-version", "2026.9.30"))
}

#[tokio::test]
async fn folder_items_parse_folders_and_playlists() {
    let server = MockServer::start().await;
    once(
        listing("root", 0).and(query_param("cursor", "")),
        json_body(ROOT),
        &server,
    )
    .await;
    once(listing(FOLDER_ID, 0), json_body(FOLDER), &server).await;
    let client = setup(&server);
    let root = client.folder("root").await.unwrap();
    let titles: Vec<&str> = root
        .iter()
        .map(|i| match i {
            FolderItem::Playlist(p) => p.title.as_str(),
            FolderItem::Folder { name, .. } => name.as_str(),
        })
        .collect();
    assert_eq!(titles, ["Playlist B", "Playlist C", "Playlist G"]);
    let folder = client.folder(FOLDER_ID).await.unwrap();
    assert!(matches!(&folder[0], FolderItem::Playlist(p) if p.uuid == UUID));
    assert_eq!(
        folder[1],
        FolderItem::Folder {
            id: SUBFOLDER_ID.into(),
            name: "Tidash probe subfolder".into(),
            count: 0,
        }
    );
}

/// A FOLDER item named `n`.
fn folder_entry(n: usize) -> Value {
    json!({"trn": format!("trn:folder:f{n}"), "itemType": "FOLDER", "name": format!("F{n}"),
           "parent": null, "data": {"id": format!("f{n}"), "name": format!("F{n}"),
           "totalNumberOfItems": n, "itemType": "FOLDER"}})
}

fn folder_page(ids: std::ops::Range<usize>, total: usize) -> Value {
    let items: Vec<Value> = ids.map(folder_entry).collect();
    json!({"items": items, "lastModifiedAt": "x", "totalNumberOfItems": total})
}

#[tokio::test]
async fn folder_pages_by_offset() {
    let server = MockServer::start().await;
    for (offset, ids) in [(0, 0..50), (50, 50..60)] {
        let page = ResponseTemplate::new(200).set_body_json(folder_page(ids, 60));
        once(listing("root", offset), page, &server).await;
    }
    let items = setup(&server).folder("root").await.unwrap();
    assert_eq!(items.len(), 60);
    assert_eq!(
        items[59],
        FolderItem::Folder {
            id: "f59".into(),
            name: "F59".into(),
            count: 59
        }
    );
}

#[tokio::test]
async fn folder_follows_a_cursor_if_one_appears() {
    let server = MockServer::start().await;
    let mut first = folder_page(0..50, 100);
    first["cursor"] = "c2".into();
    let page = ResponseTemplate::new(200).set_body_json(first);
    once(
        listing("root", 0).and(query_param("cursor", "")),
        page,
        &server,
    )
    .await;
    let page = ResponseTemplate::new(200).set_body_json(folder_page(50..52, 100));
    once(
        listing("root", 50).and(query_param("cursor", "c2")),
        page,
        &server,
    )
    .await;
    assert_eq!(setup(&server).folder("root").await.unwrap().len(), 52);
}

fn create_body() -> Value {
    json!({"data": {"type": "playlists", "attributes": {
        "name": "Tidash probe", "description": "", "accessType": "UNLISTED"}}})
}

#[tokio::test]
async fn create_playlist_posts_unlisted() {
    let server = MockServer::start().await;
    let created = ResponseTemplate::new(201).set_body_raw(CREATED, JSON_API);
    once(
        json_api("POST", "playlists").and(body_json(create_body())),
        created,
        &server,
    )
    .await;
    let uuid = setup(&server)
        .create_playlist("Tidash probe", "", None)
        .await
        .unwrap();
    assert_eq!(uuid, UUID);
}

#[tokio::test]
async fn create_playlist_in_folder_moves_it() {
    let server = MockServer::start().await;
    let created = ResponseTemplate::new(201).set_body_raw(CREATED, JSON_API);
    once(
        json_api("POST", "playlists").and(body_json(create_body())),
        created,
        &server,
    )
    .await;
    let mock = folders_put("move")
        .and(query_param("folderId", FOLDER_ID))
        .and(query_param("trns", format!("trn:playlist:{UUID}")));
    once(mock, no_content(), &server).await;
    let uuid = setup(&server)
        .create_playlist("Tidash probe", "", Some(FOLDER_ID))
        .await
        .unwrap();
    assert_eq!(uuid, UUID);
}

/// Through `edit_playlist`: an empty 204 is success (S5 saw 200 with a stale body; SONE saw 204).
#[tokio::test]
async fn json_api_204_is_ok() {
    let server = MockServer::start().await;
    let body = json!({"data": {"id": UUID, "type": "playlists",
        "attributes": {"name": "New name", "description": "New description"}}});
    let mock = json_api("PATCH", &format!("playlists/{UUID}")).and(body_json(body));
    once(mock, no_content(), &server).await;
    setup(&server)
        .edit_playlist(UUID, "New name", "New description")
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_playlist_removes_its_trn() {
    let server = MockServer::start().await;
    let mock = folders_put("remove").and(query_param("trns", format!("trn:playlist:{UUID}")));
    once(mock, no_content(), &server).await;
    setup(&server).delete_playlist(UUID).await.unwrap();
}

/// `create-folder`'s answer: a top-level folder item.
fn folder_created() -> ResponseTemplate {
    let listing: Value = serde_json::from_str(FOLDER).unwrap();
    ResponseTemplate::new(200).set_body_json(&listing["items"][1])
}

fn create_folder_mock() -> MockBuilder {
    folders_put("create-folder")
        .and(query_param("folderId", "root"))
        .and(query_param("name", "Tidash probe subfolder"))
        .and(query_param("trns", ""))
}

#[tokio::test]
async fn create_folder_at_root() {
    let server = MockServer::start().await;
    once(create_folder_mock(), folder_created(), &server).await;
    let id = setup(&server)
        .create_folder("Tidash probe subfolder", None)
        .await
        .unwrap();
    assert_eq!(id, SUBFOLDER_ID);
}

#[tokio::test]
async fn create_folder_with_a_parent_creates_at_root_then_moves() {
    let server = MockServer::start().await;
    once(create_folder_mock(), folder_created(), &server).await;
    let mock = folders_put("move")
        .and(query_param("folderId", FOLDER_ID))
        .and(query_param("trns", format!("trn:folder:{SUBFOLDER_ID}")));
    once(mock, no_content(), &server).await;
    let id = setup(&server)
        .create_folder("Tidash probe subfolder", Some(FOLDER_ID))
        .await
        .unwrap();
    assert_eq!(id, SUBFOLDER_ID);
}

#[tokio::test]
async fn rename_folder_sends_name_and_trn() {
    let server = MockServer::start().await;
    let mock = folders_put("rename")
        .and(query_param("name", "A & B"))
        .and(query_param("trn", format!("trn:folder:{FOLDER_ID}")));
    once(mock, no_content(), &server).await;
    setup(&server)
        .rename_folder(FOLDER_ID, "A & B")
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_folder_removes_its_trn() {
    let server = MockServer::start().await;
    let mock = folders_put("remove").and(query_param("trns", format!("trn:folder:{FOLDER_ID}")));
    once(mock, no_content(), &server).await;
    setup(&server).delete_folder(FOLDER_ID).await.unwrap();
}

#[tokio::test]
async fn move_to_folder_sends_20_trns_per_request() {
    let server = MockServer::start().await;
    let uuids: Vec<String> = (0..21).map(|i| format!("u{i}")).collect();
    let trns: Vec<String> = uuids.iter().map(|u| format!("trn:playlist:{u}")).collect();
    for batch in trns.chunks(20) {
        let mock = folders_put("move")
            .and(query_param("folderId", "root"))
            .and(query_param("trns", batch.join(",")));
        once(mock, no_content(), &server).await;
    }
    setup(&server).move_to_folder(&uuids, None).await.unwrap();
}
