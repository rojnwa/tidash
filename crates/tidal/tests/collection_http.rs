#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::sync::Arc;
use tidal::{Client, ClientConfig, Favorite, FavoriteIds, MemoryStore, Session};
use wiremock::matchers::{body_string, header, method, path, query_param};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

const TRACKS: &str = include_str!("fixtures/favorites_tracks.json");
const ALBUMS: &str = include_str!("fixtures/favorites_albums.json");
const ARTISTS: &str = include_str!("fixtures/favorites_artists.json");
const MIXES: &str = include_str!("fixtures/favorites_mixes.json");
const PLAYLISTS: &str = include_str!("fixtures/playlists_flattened.json");
const IDS: &str = include_str!("fixtures/favorites_ids.json");
const MIX_IDS: &str = include_str!("fixtures/mixes_ids.json");

fn setup(server: &MockServer) -> Client {
    let base = format!("{}/v1/", server.uri());
    let cfg = ClientConfig {
        api_base: base.parse().unwrap(),
        api_v2_base: format!("{}/v2/", server.uri()).parse().unwrap(),
        auth_base: base.parse().unwrap(),
        ..ClientConfig::default()
    };
    let session = Session {
        access_token: "ACCESS".into(),
        refresh_token: "REFRESH".into(),
        expires_at: u64::MAX,
        user_id: 12345,
        country_code: "US".into(),
    };
    Client::new(cfg, session, Arc::new(MemoryStore::default()), None).unwrap()
}

fn json_body(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

/// A v1 favorites list, newest first, one page of 1000.
async fn mock_favorites(server: &MockServer, kind: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/v1/users/12345/favorites/{kind}")))
        .and(query_param("order", "DATE"))
        .and(query_param("orderDirection", "DESC"))
        .and(query_param("limit", "1000"))
        .and(query_param("offset", "0"))
        .respond_with(json_body(body))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn favorite_tracks_use_date_desc_and_unwrap_items() {
    let server = MockServer::start().await;
    mock_favorites(&server, "tracks", TRACKS).await;
    let tracks = setup(&server).favorite_tracks().await.unwrap();
    let ids: Vec<u64> = tracks.iter().map(|t| t.id).collect();
    assert_eq!(ids, [547494409, 526366074, 326242566]);
    assert_eq!(tracks[0].title, "critic");
}

#[tokio::test]
async fn favorite_albums_and_artists_unwrap_items() {
    let server = MockServer::start().await;
    mock_favorites(&server, "albums", ALBUMS).await;
    mock_favorites(&server, "artists", ARTISTS).await;
    let client = setup(&server);
    let albums: Vec<u64> = client
        .favorite_albums()
        .await
        .unwrap()
        .iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(albums, [428817523, 116334456]);
    let artists: Vec<u64> = client
        .favorite_artists()
        .await
        .unwrap()
        .iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(artists, [21078379, 33953262, 5905356]);
}

#[tokio::test]
async fn favorite_mixes_page_by_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/favorites/mixes"))
        .and(query_param("limit", "50"))
        .and(query_param("cursor", ""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "first", "title": "First", "subTitle": "Sub"}], "cursor": "next"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/favorites/mixes"))
        .and(query_param("cursor", "next"))
        .respond_with(json_body(MIXES))
        .expect(1)
        .mount(&server)
        .await;
    let mixes = setup(&server).favorite_mixes().await.unwrap();
    let ids: Vec<&str> = mixes.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "first",
            "001630ec416425e180d18273a59246",
            "002c20f9756af3547934ad0e57ab38",
            "002c2dd49e6c588d521d134e915284"
        ]
    );
    assert_eq!(mixes[2].title, "My Mix 3");
}

/// A cursor that answers with itself forever: its page is taken once, then the list ends.
#[tokio::test]
async fn a_repeated_cursor_ends_the_list() {
    let server = MockServer::start().await;
    let page =
        |id: &str| json!({"items": [{"id": id, "title": id, "subTitle": ""}], "cursor": "stuck"});
    for (cursor, id) in [("", "first"), ("stuck", "second")] {
        Mock::given(method("GET"))
            .and(path("/v2/favorites/mixes"))
            .and(query_param("cursor", cursor))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(id)))
            .expect(1)
            .mount(&server)
            .await;
    }
    let mixes = setup(&server).favorite_mixes().await.unwrap();
    let ids: Vec<&str> = mixes.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["first", "second"]);
}

#[tokio::test]
async fn favorite_playlists_skip_folders() {
    let server = MockServer::start().await;
    let mut page: Value = serde_json::from_str(PLAYLISTS).unwrap();
    // Its data would parse as a playlist: only `itemType` keeps it out.
    page["items"].as_array_mut().unwrap().insert(
        1,
        json!({"trn": "trn:folder:f1", "itemType": "FOLDER", "name": "Folder", "parent": null,
               "data": {"id": "f1", "uuid": "f1", "title": "Folder", "name": "Folder"}}),
    );
    Mock::given(method("GET"))
        .and(path("/v2/my-collection/playlists/folders/flattened"))
        .and(query_param("includeOnly", ""))
        .and(query_param("order", "DATE"))
        .and(query_param("orderDirection", "DESC"))
        .and(query_param("limit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page))
        .expect(1)
        .mount(&server)
        .await;
    let playlists = setup(&server).favorite_playlists().await.unwrap();
    let titles: Vec<&str> = playlists.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["Playlist A", "Playlist B", "Playlist C"]);
    assert_eq!(playlists[0].uuid, "00000000-0000-4000-8000-000000000001");
    // The user's own: S4 says only `creator.id` tells them from favorited ones.
    let creator = (playlists[0].creator.as_deref(), playlists[0].creator_id);
    assert_eq!(creator, (Some("User"), Some(12345)));
}

#[tokio::test]
async fn favorite_ids_merges_two_sources() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/users/12345/favorites/ids"))
        .respond_with(json_body(IDS))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/favorites/mixes/ids"))
        .and(query_param("limit", "500"))
        .and(query_param("cursor", ""))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"content": ["m1", "m2"], "cursor": "c2"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/favorites/mixes/ids"))
        .and(query_param("limit", "500"))
        .and(query_param("cursor", "c2"))
        .respond_with(json_body(MIX_IDS))
        .expect(1)
        .mount(&server)
        .await;
    let ids = setup(&server).favorite_ids().await.unwrap();
    let set = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(
        ids,
        FavoriteIds {
            tracks: [100101798, 101030721, 103527943].into(),
            albums: [116334456, 428817523].into(),
            artists: [10177263, 10229434, 10372].into(),
            playlists: set(&[
                "00000000-0000-4000-8000-000000000003",
                "00000000-0000-4000-8000-000000000001"
            ]),
            mixes: set(&[
                "m1",
                "m2",
                "000a518083dfdb0d30d1b4cfb033f0",
                "001203f8c1ed715f17fde5cc52e834",
                "001630ec416425e180d18273a59246"
            ]),
        }
    );
}

#[test]
fn favorite_ids_set_and_contains() {
    let mut ids = FavoriteIds::default();
    let all = [
        Favorite::Track(1),
        Favorite::Album(1),
        Favorite::Artist(1),
        Favorite::Playlist("p".into()),
        Favorite::Mix("m".into()),
    ];
    for f in &all {
        assert!(!ids.contains(f));
        ids.set(f, true);
        assert!(ids.contains(f), "{f:?}");
    }
    // Kinds don't share ids.
    assert!(!ids.contains(&Favorite::Track(2)) && !ids.contains(&Favorite::Mix("p".into())));
    ids.set(&all[0], true); // idempotent
    ids.set(&all[0], false);
    assert!(!ids.contains(&all[0]) && ids.contains(&all[1]));
    ids.set(&all[0], false);
    assert!(!ids.contains(&all[0]));
}

/// `set_favorite(f, on)` sends exactly the one request `mock` matches.
async fn writes(mock: MockBuilder, f: Favorite, on: bool) {
    let server = MockServer::start().await;
    mock.and(header("authorization", "Bearer ACCESS"))
        .and(query_param("countryCode", "US"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    setup(&server).set_favorite(&f, on).await.unwrap();
}

fn v1_add(kind: &str, body: &str) -> MockBuilder {
    Mock::given(method("POST"))
        .and(path(format!("/v1/users/12345/favorites/{kind}")))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string(body))
}

fn v1_remove(kind: &str, id: u64) -> MockBuilder {
    Mock::given(method("DELETE"))
        .and(path(format!("/v1/users/12345/favorites/{kind}/{id}")))
        .and(body_string(""))
}

/// A v2 PUT with no body.
fn v2_put_empty(p: &str) -> MockBuilder {
    Mock::given(method("PUT"))
        .and(path(p))
        .and(header("x-tidal-client-version", "2026.9.30"))
        .and(header("content-type", "application/json"))
        .and(body_string(""))
}

#[tokio::test]
async fn add_track() {
    let body = "trackIds=7&onArtifactNotFound=FAIL";
    writes(v1_add("tracks", body), Favorite::Track(7), true).await;
}

#[tokio::test]
async fn remove_track() {
    writes(v1_remove("tracks", 7), Favorite::Track(7), false).await;
}

#[tokio::test]
async fn add_album() {
    let body = "albumIds=8&onArtifactNotFound=FAIL";
    writes(v1_add("albums", body), Favorite::Album(8), true).await;
}

#[tokio::test]
async fn remove_album() {
    writes(v1_remove("albums", 8), Favorite::Album(8), false).await;
}

#[tokio::test]
async fn add_artist() {
    let body = "artistIds=9&onArtifactNotFound=FAIL";
    writes(v1_add("artists", body), Favorite::Artist(9), true).await;
}

#[tokio::test]
async fn remove_artist() {
    writes(v1_remove("artists", 9), Favorite::Artist(9), false).await;
}

#[tokio::test]
async fn add_mix() {
    let mock = Mock::given(method("PUT"))
        .and(path("/v2/favorites/mixes/add"))
        .and(header("x-tidal-client-version", "2026.9.30"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("mixIds=m1&onArtifactNotFound=FAIL"));
    writes(mock, Favorite::Mix("m1".into()), true).await;
}

#[tokio::test]
async fn remove_mix() {
    let mock = v2_put_empty("/v2/favorites/mixes/remove").and(query_param("mixIds", "m1"));
    writes(mock, Favorite::Mix("m1".into()), false).await;
}

#[tokio::test]
async fn add_playlist() {
    let mock = v2_put_empty("/v2/my-collection/playlists/folders/add-favorites")
        .and(query_param("folderId", "root"))
        .and(query_param("uuids", "u-1"));
    writes(mock, Favorite::Playlist("u-1".into()), true).await;
}

#[tokio::test]
async fn remove_playlist() {
    let mock = v2_put_empty("/v2/my-collection/playlists/folders/remove")
        .and(query_param("trns", "trn:playlist:u-1"));
    writes(mock, Favorite::Playlist("u-1".into()), false).await;
}
