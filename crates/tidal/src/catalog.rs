//! Catalog models and endpoints: search, album, album tracks.

use crate::{Client, Image, ImageKind, Result, browse::tracks};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub selected_album_cover_fallback: Option<String>,
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub uuid: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The creator's name; `None` is TIDAL. Playlists in v1 page modules carry
    /// `creators: []` instead.
    #[serde(default, deserialize_with = "creator_name")]
    pub creator: Option<String>,
    #[serde(default)]
    pub number_of_tracks: u32,
    #[serde(default)]
    pub duration: u32,
    #[serde(default)]
    pub square_image: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

fn creator_name<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct Creator {
        #[serde(default)]
        name: Option<String>,
    }
    Ok(Option::<Creator>::deserialize(d)?.and_then(|c| c.name))
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(from = "RawMix")]
pub struct Mix {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub images: Option<Image>,
}

/// A mix in any of its three shapes (S4): v1 `title`/`subTitle` + `images{SMALL,…}`; v2
/// `titleTextInfo`/`subtitleTextInfo` + `mixImages[]`; v2 favorites `subTitleTextInfo` + `images`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMix {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    sub_title: Option<String>,
    #[serde(default)]
    title_text_info: Option<TextInfo>,
    #[serde(default)]
    subtitle_text_info: Option<TextInfo>,
    #[serde(default)]
    sub_title_text_info: Option<TextInfo>,
    #[serde(default)]
    images: Option<HashMap<String, Rendition>>,
    #[serde(default)]
    mix_images: Option<Vec<Rendition>>,
}

#[derive(Deserialize)]
struct TextInfo {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct Rendition {
    #[serde(default)]
    size: Option<String>,
    url: String,
}

impl From<RawMix> for Mix {
    fn from(r: RawMix) -> Self {
        let text = |t: Option<TextInfo>| t.and_then(|t| t.text);
        let mut urls: HashMap<String, String> = r
            .images
            .unwrap_or_default()
            .into_iter()
            .map(|(size, i)| (size, i.url))
            .chain(
                r.mix_images
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|i| Some((i.size?, i.url))),
            )
            .collect();
        let images = match (
            urls.remove("SMALL"),
            urls.remove("MEDIUM"),
            urls.remove("LARGE"),
        ) {
            (Some(small), Some(medium), Some(large)) => Some(Image::Urls {
                small,
                medium,
                large,
            }),
            _ => None,
        };
        Mix {
            id: r.id,
            title: r.title.or(text(r.title_text_info)).unwrap_or_default(),
            subtitle: r
                .sub_title
                .or(text(r.subtitle_text_info))
                .or(text(r.sub_title_text_info))
                .unwrap_or_default(),
            images,
        }
    }
}

fn tidal_image(id: Option<&str>, kind: ImageKind) -> Option<Image> {
    id.map(|id| Image::Tidal {
        id: id.to_owned(),
        kind,
    })
}

impl Artist {
    /// The picture, else the cover of an album of theirs.
    pub fn image(&self) -> Option<Image> {
        tidal_image(self.picture.as_deref(), ImageKind::Artist).or_else(|| {
            tidal_image(
                self.selected_album_cover_fallback.as_deref(),
                ImageKind::Album,
            )
        })
    }
}

impl Album {
    pub fn image(&self) -> Option<Image> {
        tidal_image(self.cover.as_deref(), ImageKind::Album)
    }
}

impl AlbumRef {
    pub fn image(&self) -> Option<Image> {
        tidal_image(self.cover.as_deref(), ImageKind::Album)
    }
}

impl Playlist {
    /// The square image, else the wide one.
    pub fn image(&self) -> Option<Image> {
        tidal_image(self.square_image.as_deref(), ImageKind::Playlist)
            .or_else(|| tidal_image(self.image.as_deref(), ImageKind::PlaylistWide))
    }
}

impl Mix {
    pub fn image(&self) -> Option<Image> {
        self.images.clone()
    }
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
        let items = self
            .paged_all(&format!("albums/{id}/items"), &[], PAGE)
            .await?;
        Ok(tracks(items))
    }
}
