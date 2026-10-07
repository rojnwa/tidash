//! Catalog models and endpoints: search, album, album tracks, cover URLs.

use crate::{Client, ClientConfig, Result};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRef {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub number_of_tracks: u32,
    #[serde(default)]
    pub duration: u32,
    #[serde(default)]
    pub release_date: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub duration: u32,
    #[serde(default)]
    pub track_number: u32,
    #[serde(default)]
    pub volume_number: u32,
    #[serde(default)]
    pub artists: Vec<Artist>,
    pub album: AlbumRef,
    #[serde(default)]
    pub stream_ready: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchResults {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

#[derive(Deserialize)]
struct Section<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

#[derive(Deserialize)]
struct RawSearch {
    artists: Option<Section<Artist>>,
    albums: Option<Section<Album>>,
    tracks: Option<Section<Track>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Items {
    #[serde(default)]
    total_number_of_items: usize,
    #[serde(default)]
    items: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: String,
    item: serde_json::Value,
}

const PAGE: usize = 50;

impl Client {
    pub async fn search(&self, query: &str, limit: u32) -> Result<SearchResults> {
        let raw: RawSearch = self
            .get(
                "search",
                &[
                    ("query", query.to_string()),
                    ("types", "ARTISTS,ALBUMS,TRACKS".into()),
                    ("limit", limit.to_string()),
                ],
            )
            .await?;
        Ok(SearchResults {
            artists: raw.artists.map(|s| s.items).unwrap_or_default(),
            albums: raw.albums.map(|s| s.items).unwrap_or_default(),
            tracks: raw.tracks.map(|s| s.items).unwrap_or_default(),
        })
    }

    pub async fn album(&self, id: u64) -> Result<Album> {
        self.get(&format!("albums/{id}"), &[]).await
    }

    pub async fn album_tracks(&self, id: u64) -> Result<Vec<Track>> {
        let mut tracks = Vec::new();
        let mut offset = 0;
        loop {
            let page: Items = self
                .get(
                    &format!("albums/{id}/items"),
                    &[("limit", PAGE.to_string()), ("offset", offset.to_string())],
                )
                .await?;
            // Zero items guards against a server that over-reports the total.
            if page.items.is_empty() {
                break;
            }
            offset += page.items.len();
            for e in page.items.into_iter().filter(|e| e.kind == "track") {
                tracks.push(serde_json::from_value(e.item)?);
            }
            if offset >= page.total_number_of_items {
                break;
            }
        }
        Ok(tracks)
    }
}

/// `image_base` + cover with `-` as `/` + `/{size}x{size}.jpg`.
pub fn image_url(cfg: &ClientConfig, cover: &str, size: u32) -> String {
    format!(
        "{}{}/{size}x{size}.jpg",
        cfg.image_base,
        cover.replace('-', "/")
    )
}
