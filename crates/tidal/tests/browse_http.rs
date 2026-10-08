#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::sync::Arc;
use tidal::{
    Client, ClientConfig, Image, ImageKind, Item, Layout, MemoryStore, Mix, More, Session,
};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HOME: &str = include_str!("fixtures/home_feed.json");
const EXPLORE: &str = include_str!("fixtures/explore.json");
const GENRE: &str = include_str!("fixtures/genre_page.json");
const ARTIST: &str = include_str!("fixtures/artist_page.json");
const PLAYLIST: &str = include_str!("fixtures/playlist.json");
const PLAYLIST_ITEMS: &str = include_str!("fixtures/playlist_items.json");
const MIX_PAGE: &str = include_str!("fixtures/mix_page.json");
const FAVORITE_MIXES: &str = include_str!("fixtures/favorites_mixes.json");

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
        user_id: 1,
        country_code: "US".into(),
    };
    Client::new(cfg, session, Arc::new(MemoryStore::default()), None).unwrap()
}

fn json_body(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

fn value(v: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(v)
}

fn album(id: u64) -> Value {
    json!({"id": id, "title": format!("Album {id}")})
}

fn track(id: u64) -> Value {
    json!({"id": id, "title": format!("Track {id}"), "album": album(1)})
}

/// A one-page home feed holding `modules`.
async fn mock_home(server: &MockServer, modules: Value) {
    Mock::given(method("GET"))
        .and(path("/v2/home/feed/static"))
        .respond_with(value(json!({"page": {"cursor": null}, "items": modules})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn home_feed_becomes_shelves() {
    let server = MockServer::start().await;
    let fixture: Value = serde_json::from_str(HOME).unwrap();
    let cursor = fixture["page"]["cursor"].as_str().unwrap().to_owned();
    Mock::given(method("GET"))
        .and(path("/v2/home/feed/static"))
        .and(query_param("limit", "50"))
        .and(query_param_is_missing("cursor"))
        .respond_with(json_body(HOME))
        .expect(1)
        .mount(&server)
        .await;
    let page2 = json!({"page": {"cursor": null}, "items": [
        {"type": "HORIZONTAL_LIST", "title": "Page two", "items": [{"type": "ALBUM", "data": album(7)}]}
    ]});
    Mock::given(method("GET"))
        .and(path("/v2/home/feed/static"))
        .and(query_param("cursor", cursor.as_str()))
        .respond_with(value(page2))
        .expect(1)
        .mount(&server)
        .await;

    let page = setup(&server).home().await.unwrap();
    let modules = fixture["items"].as_array().unwrap();
    assert_eq!(page.shelves.len(), modules.len() + 1);
    assert_eq!(page.shelves[0].title, "Shortcuts");
    assert_eq!(page.shelves[1].title, "Custom mixes");
    assert_eq!(page.shelves.last().unwrap().title, "Page two");
    // Every fixture item is a known type with the fields it needs.
    let items: usize = modules
        .iter()
        .map(|m| m["items"].as_array().unwrap().len())
        .sum();
    let parsed: usize = page.shelves[..modules.len()]
        .iter()
        .map(|s| s.items.len())
        .sum();
    assert_eq!(parsed, items);
    assert_eq!(
        page.shelves[1].more,
        Some(More::ViewAll("home/pages/DAILY_MIXES/view-all".into()))
    );
    assert!(page.shelves.iter().all(|s| s.layout == Layout::Cards));

    for req in server.received_requests().await.unwrap() {
        let offset = req
            .url
            .query_pairs()
            .find(|(k, _)| k == "timeOffset")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        let b = offset.as_bytes();
        assert!(
            b.len() == 6 && (b[0] == b'+' || b[0] == b'-') && b[3] == b':',
            "timeOffset {offset}"
        );
    }
}

#[tokio::test]
async fn unknown_module_and_item_types_are_skipped() {
    let server = MockServer::start().await;
    mock_home(
        &server,
        json!([
            {"type": "SOMETHING_NEW", "title": "New", "items": [{"type": "ALBUM", "data": album(1)}]},
            {"type": "HORIZONTAL_LIST", "title": "Kept", "items": [
                {"type": "MAGAZINE", "data": {"id": "m"}},
                {"type": "ALBUM", "data": album(2)},
                {"type": "ALBUM", "data": {"title": "no id"}}
            ]},
            {"type": "HORIZONTAL_LIST", "title": "Empty", "items": [{"type": "MAGAZINE", "data": {}}]}
        ]),
    )
    .await;
    let page = setup(&server).home().await.unwrap();
    assert_eq!(page.shelves.len(), 1);
    assert_eq!(page.shelves[0].title, "Kept");
    assert!(matches!(&page.shelves[0].items[..], [Item::Album(a)] if a.id == 2));
}

#[tokio::test]
async fn item_missing_optional_fields_parses() {
    let server = MockServer::start().await;
    mock_home(
        &server,
        json!([{"type": "HORIZONTAL_LIST", "title": "Bare", "items": [
            {"type": "ALBUM", "data": {"id": 1, "title": "No cover"}},
            {"type": "ARTIST", "data": {"id": 2, "name": "No picture"}}
        ]}]),
    )
    .await;
    let page = setup(&server).home().await.unwrap();
    let [Item::Album(album), Item::Artist(artist)] = &page.shelves[0].items[..] else {
        panic!("{:?}", page.shelves[0].items);
    };
    assert_eq!((album.cover.as_deref(), album.image()), (None, None));
    assert_eq!((artist.picture.as_deref(), artist.image()), (None, None));
}

#[tokio::test]
async fn explore_has_link_shelf() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/pages/explore"))
        .respond_with(json_body(EXPLORE))
        .expect(1)
        .mount(&server)
        .await;
    let page = setup(&server).page("pages/explore").await.unwrap();
    assert_eq!(page.shelves.len(), 4);
    let genres = &page.shelves[0];
    assert_eq!(
        (genres.title.as_str(), genres.layout),
        ("Genres", Layout::Links)
    );
    assert_eq!(genres.more, Some(More::Page("pages/genre_page".into())));
    assert!(matches!(&genres.items[0],
        Item::Link { title, api_path } if title == "Hip-Hop" && api_path == "pages/genre_hip_hop"));
    assert!(page.shelves.iter().all(|s| s.layout == Layout::Links));
}

#[tokio::test]
async fn genre_page_skips_videos_and_reads_creators() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/pages/genre_pop"))
        .respond_with(json_body(GENRE))
        .mount(&server)
        .await;
    let page = setup(&server).page("pages/genre_pop").await.unwrap();
    assert_eq!(page.title, "Pop");
    let titles: Vec<_> = page.shelves.iter().map(|s| s.title.as_str()).collect();
    assert!(!titles.contains(&"New Music Videos"), "{titles:?}");
    assert_eq!(page.shelves.len(), 8);
    let tracks = page
        .shelves
        .iter()
        .find(|s| s.title == "New Tracks")
        .unwrap();
    assert_eq!(tracks.layout, Layout::Tracks);
    let Item::Playlist(p) = &page.shelves[0].items[0] else {
        panic!("{:?}", page.shelves[0].items[0]);
    };
    assert_eq!((&p.creator, p.creator_id), (&None, None)); // `creators: []`
    assert!(
        matches!(&page.shelves[0].more, Some(More::Page(p)) if p.starts_with("pages/single-module-page/"))
    );
}

#[tokio::test]
async fn single_module_page_fills_from_data_api_path() {
    let server = MockServer::start().await;
    let doc = json!({"title": "New Albums", "rows": [{"modules": [{
        "type": "ALBUM_LIST", "title": "New Albums",
        "pagedList": {"dataApiPath": "pages/data/abc", "limit": 2, "offset": 0,
                      "totalNumberOfItems": 5, "items": [album(1), album(2)]}
    }]}]});
    Mock::given(method("GET"))
        .and(path("/v1/pages/single-module-page/x/1"))
        .respond_with(value(doc))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/pages/data/abc"))
        .and(query_param("offset", "2"))
        .and(query_param("limit", "50"))
        .respond_with(value(
            json!({"limit": 50, "offset": 2, "totalNumberOfItems": 5,
                                   "items": [album(3), album(4), album(5)]}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let page = setup(&server)
        .page("pages/single-module-page/x/1")
        .await
        .unwrap();
    let ids: Vec<u64> = page.shelves[0]
        .items
        .iter()
        .map(|i| match i {
            Item::Album(a) => a.id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
}

#[tokio::test]
async fn artist_page_bio_and_discography() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/artist/8847"))
        .respond_with(json_body(ARTIST))
        .expect(1)
        .mount(&server)
        .await;
    let page = setup(&server).artist_page(8847).await.unwrap();
    assert_eq!(page.artist.name, "Daft Punk");
    assert!(matches!(
        page.artist.image(),
        Some(Image::Tidal {
            kind: ImageKind::Artist,
            ..
        })
    ));
    let bio = page.bio.unwrap();
    assert!(!bio.contains("<br/>") && !bio.contains("wimpLink"), "{bio}");
    assert!(bio.contains("follow\u{ad}up, Discovery, launched") && bio.contains('\n'));
    let albums = page.shelves.iter().find(|s| s.title == "Albums").unwrap();
    assert_eq!(albums.layout, Layout::Cards);
    assert_eq!(
        albums.more,
        Some(More::ViewAll(
            "artist/ARTIST_ALBUMS/view-all?artistId=8847".into()
        ))
    );
    assert_eq!(page.shelves[0].layout, Layout::Tracks); // Top Tracks
    let titles: Vec<_> = page.shelves.iter().map(|s| s.title.as_str()).collect();
    assert!(
        !titles.contains(&"Videos") && !titles.contains(&"Credits"),
        "{titles:?}"
    );
}

#[tokio::test]
async fn discography_tabs_become_shelves() {
    let server = MockServer::start().await;
    let doc = json!({"type": "ARTIST", "item": {"type": "ARTIST", "data": {"id": 1, "name": "A"}},
    "header": {"biography": null},
    "items": [{"type": "DISCOGRAPHY_TABS", "title": "Discography", "tabs": [
        {"label": "Albums", "selected": true, "viewAll": "artist/ALBUMS/view-all?artistId=1",
         "items": [{"type": "ALBUM", "data": album(1)}]},
        {"label": "Singles", "selected": false, "items": [{"type": "ALBUM", "data": album(2)}]}
    ]}]});
    Mock::given(method("GET"))
        .and(path("/v2/artist/1"))
        .respond_with(value(doc))
        .mount(&server)
        .await;
    let page = setup(&server).artist_page(1).await.unwrap();
    assert_eq!(page.bio, None);
    let titles: Vec<_> = page.shelves.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Albums", "Singles"]);
    assert_eq!(
        page.shelves[0].more,
        Some(More::ViewAll("artist/ALBUMS/view-all?artistId=1".into()))
    );
}

/// A playlist items page: `{type: track, item}` for each id.
fn playlist_page(ids: std::ops::Range<u64>, total: usize) -> Value {
    let items: Vec<_> = ids
        .map(|id| json!({"type": "track", "cut": null, "item": track(id)}))
        .collect();
    json!({"limit": 100, "offset": 0, "totalNumberOfItems": total, "items": items})
}

#[tokio::test]
async fn paged_all_steps_by_limit_through_short_pages() {
    let server = MockServer::start().await;
    // Pages run short mid-list (S4): 90 of 100, then 100, then the last 50.
    for (offset, ids) in [(0, 0..90), (100, 100..200), (200, 200..250)] {
        Mock::given(method("GET"))
            .and(path("/v1/playlists/p/items"))
            .and(query_param("offset", offset.to_string()))
            .and(query_param("limit", "100"))
            .and(query_param("order", "INDEX"))
            .and(query_param("orderDirection", "ASC"))
            .respond_with(value(playlist_page(ids, 250)))
            .expect(1)
            .mount(&server)
            .await;
    }
    let tracks = setup(&server).playlist_tracks("p").await.unwrap();
    assert_eq!(tracks.len(), 240);
    let ids: Vec<u64> = tracks.iter().map(|t| t.id).collect();
    assert!(ids.is_sorted(), "offset order");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn playlist_and_all_tracks() {
    let server = MockServer::start().await;
    let uuid = "edb69697-fc65-4580-b938-55f0f4bd7c54";
    Mock::given(method("GET"))
        .and(path(format!("/v1/playlists/{uuid}")))
        .respond_with(json_body(PLAYLIST))
        .mount(&server)
        .await;
    let mut items: Value = serde_json::from_str(PLAYLIST_ITEMS).unwrap();
    items["items"].as_array_mut().unwrap().insert(
        1,
        json!({"type": "video", "item": {"id": 9, "title": "A video"}}),
    );
    items["totalNumberOfItems"] = 4.into();
    Mock::given(method("GET"))
        .and(path(format!("/v1/playlists/{uuid}/items")))
        .respond_with(value(items))
        .expect(1)
        .mount(&server)
        .await;
    let client = setup(&server);
    let p = client.playlist(uuid).await.unwrap();
    assert_eq!(
        (p.title.as_str(), p.number_of_tracks),
        ("Pop Classics", 235)
    );
    assert_eq!((&p.creator, p.creator_id), (&None, Some(0))); // `creator: {id: 0}` is TIDAL
    assert_eq!(
        p.image(),
        Some(Image::Tidal {
            id: "595a4cb8-2656-4157-a7b6-f2d84405b956".into(),
            kind: ImageKind::Playlist
        })
    );
    let tracks = client.playlist_tracks(uuid).await.unwrap();
    let ids: Vec<u64> = tracks.iter().map(|t| t.id).collect();
    assert_eq!(ids, [10582679, 558208631, 555115]);
}

#[tokio::test]
async fn mix_from_one_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/pages/mix"))
        .and(query_param("mixId", "0002737adb8d077f4591c4cdb7ba60"))
        .respond_with(json_body(MIX_PAGE))
        .expect(1)
        .mount(&server)
        .await;
    let (mix, tracks) = setup(&server)
        .mix("0002737adb8d077f4591c4cdb7ba60")
        .await
        .unwrap();
    assert_eq!(
        (mix.title.as_str(), mix.subtitle.as_str()),
        ("Daft Punk", "Artist Radio")
    );
    assert!(matches!(mix.image(), Some(Image::Urls { .. })));
    assert_eq!(tracks.len(), 3);
}

#[test]
fn mix_parses_all_three_shapes() {
    let page: Value = serde_json::from_str(MIX_PAGE).unwrap();
    let v1 = page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["modules"].as_array().unwrap())
        .find(|m| m["type"] == "MIX_HEADER")
        .unwrap()["mix"]
        .clone();
    let home: Value = serde_json::from_str(HOME).unwrap();
    let v2 = home["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["items"].as_array().unwrap())
        .find(|i| i["type"] == "MIX")
        .unwrap()["data"]
        .clone();
    let favorites: Value = serde_json::from_str(FAVORITE_MIXES).unwrap();
    let favorite = favorites["items"][0].clone();

    for (raw, title, subtitle) in [
        (v1, "Daft Punk", "Artist Radio"),
        (v2, "405 Anthem", "Mqx"),
        (
            favorite.clone(),
            favorite["title"].as_str().unwrap(),
            favorite["subTitle"].as_str().unwrap(),
        ),
    ] {
        let mix: Mix = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(mix.id, raw["id"].as_str().unwrap());
        assert_eq!(
            (mix.title.as_str(), mix.subtitle.as_str()),
            (title, subtitle)
        );
        let Some(Image::Urls {
            small,
            medium,
            large,
        }) = mix.images
        else {
            panic!("{:?}", mix.images);
        };
        assert!(
            [small, medium, large]
                .iter()
                .all(|u| u.starts_with("https://"))
        );
    }
}

#[tokio::test]
async fn view_all_pages_until_short_page() {
    let server = MockServer::start().await;
    for (offset, n) in [(0u64, 50u64), (50, 50), (100, 5)] {
        let items: Vec<_> = (offset..offset + n)
            .map(|id| json!({"type": "ALBUM", "data": album(id)}))
            .collect();
        Mock::given(method("GET"))
            .and(path("/v2/home/pages/NEW_ALBUM_SUGGESTIONS/view-all"))
            .and(query_param("itemId", "abc"))
            .and(query_param("offset", offset.to_string()))
            .and(query_param("limit", "50"))
            .respond_with(value(json!({"title": "Suggested", "items": items})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let page = setup(&server)
        .view_all("home/pages/NEW_ALBUM_SUGGESTIONS/view-all?itemId=abc")
        .await
        .unwrap();
    assert_eq!(page.title, "Suggested");
    let [shelf] = &page.shelves[..] else {
        panic!("{:?}", page.shelves);
    };
    assert_eq!((shelf.title.as_str(), shelf.layout), ("", Layout::Cards));
    assert_eq!(shelf.items.len(), 105);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
