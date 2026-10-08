//! The user's collection: favorite tracks, albums, artists, mixes and playlists, the favorite
//! ids that draw hearts, and adding or removing a favorite, as the official client does (S4).

use crate::browse::take_list;
use crate::{Album, Artist, Client, Mix, Playlist, Result, Track};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashSet;
use std::hash::Hash;

/// v1 favorites accept pages of 1000 or more (S4).
const FAVORITES_LIMIT: usize = 1000;
/// Requests per cursor list.
// ponytail: a fixed cap stops a cursor that never ends; 200 pages is 10 000 playlists or mixes.
const CURSOR_PAGES: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Favorite {
    Track(u64),
    Album(u64),
    Artist(u64),
    Playlist(String),
    Mix(String),
}

/// `playlists` holds favorited playlists only, not the user's own (S4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FavoriteIds {
    pub tracks: HashSet<u64>,
    pub albums: HashSet<u64>,
    pub artists: HashSet<u64>,
    pub playlists: HashSet<String>,
    pub mixes: HashSet<String>,
}

impl FavoriteIds {
    pub fn contains(&self, f: &Favorite) -> bool {
        match f {
            Favorite::Track(id) => self.tracks.contains(id),
            Favorite::Album(id) => self.albums.contains(id),
            Favorite::Artist(id) => self.artists.contains(id),
            Favorite::Playlist(uuid) => self.playlists.contains(uuid),
            Favorite::Mix(id) => self.mixes.contains(id),
        }
    }

    /// Adds `f` when `on`, else removes it.
    pub fn set(&mut self, f: &Favorite, on: bool) {
        fn put<T: Clone + Eq + Hash>(set: &mut HashSet<T>, v: &T, on: bool) {
            if on {
                set.insert(v.clone());
            } else {
                set.remove(v);
            }
        }
        match f {
            Favorite::Track(id) => put(&mut self.tracks, id, on),
            Favorite::Album(id) => put(&mut self.albums, id, on),
            Favorite::Artist(id) => put(&mut self.artists, id, on),
            Favorite::Playlist(uuid) => put(&mut self.playlists, uuid, on),
            Favorite::Mix(id) => put(&mut self.mixes, id, on),
        }
    }
}

/// v1 `favorites/ids`: string ids by kind (`VIDEO` too, unused).
#[derive(Deserialize)]
#[serde(rename_all = "UPPERCASE")]
struct RawIds {
    #[serde(default)]
    track: Vec<String>,
    #[serde(default)]
    album: Vec<String>,
    #[serde(default)]
    artist: Vec<String>,
    #[serde(default)]
    playlist: Vec<String>,
}

impl Client {
    pub async fn favorite_tracks(&self) -> Result<Vec<Track>> {
        self.favorites("tracks").await
    }

    pub async fn favorite_albums(&self) -> Result<Vec<Album>> {
        self.favorites("albums").await
    }

    pub async fn favorite_artists(&self) -> Result<Vec<Artist>> {
        self.favorites("artists").await
    }

    pub async fn favorite_mixes(&self) -> Result<Vec<Mix>> {
        let query = [
            ("limit", "50".to_owned()),
            ("order", "DATE".to_owned()),
            ("orderDirection", "DESC".to_owned()),
        ];
        let items = self.cursor_all("favorites/mixes", &query, "items").await?;
        Ok(items.into_iter().filter_map(parse).collect())
    }

    /// The user's own playlists and the ones they favorited, newest first; folders skipped.
    pub async fn favorite_playlists(&self) -> Result<Vec<Playlist>> {
        let query = [
            ("includeOnly", String::new()),
            ("order", "DATE".to_owned()),
            ("orderDirection", "DESC".to_owned()),
            ("limit", "50".to_owned()),
        ];
        let items = self
            .cursor_all("my-collection/playlists/folders/flattened", &query, "items")
            .await?;
        Ok(items
            .into_iter()
            .filter(|i| i["itemType"] != "FOLDER")
            .filter_map(|mut i| parse(i.get_mut("data")?.take()))
            .collect())
    }

    /// Every favorite's id: v1 `favorites/ids`, plus the mixes from v2.
    pub async fn favorite_ids(&self) -> Result<FavoriteIds> {
        let uid = self.session().user_id;
        let raw: RawIds = self.get(&format!("users/{uid}/favorites/ids"), &[]).await?;
        let mixes = self
            .cursor_all("favorites/mixes/ids", &[("limit", "500".into())], "content")
            .await?;
        let numbers = |ids: Vec<String>| ids.iter().filter_map(|id| id.parse().ok()).collect();
        Ok(FavoriteIds {
            tracks: numbers(raw.track),
            albums: numbers(raw.album),
            artists: numbers(raw.artist),
            playlists: raw.playlist.into_iter().collect(),
            mixes: mixes
                .iter()
                .filter_map(|id| Some(id.as_str()?.to_owned()))
                .collect(),
        })
    }

    /// Adds (`on`) or removes a favorite with the official client's requests.
    pub async fn set_favorite(&self, f: &Favorite, on: bool) -> Result<()> {
        let fail = || ("onArtifactNotFound", "FAIL".to_owned());
        let favorites = format!("users/{}/favorites", self.session().user_id);
        match (f, on) {
            (Favorite::Track(id), true) => {
                let form = [("trackIds", id.to_string()), fail()];
                self.post_form(&format!("{favorites}/tracks"), &form).await
            }
            (Favorite::Album(id), true) => {
                let form = [("albumIds", id.to_string()), fail()];
                self.post_form(&format!("{favorites}/albums"), &form).await
            }
            (Favorite::Artist(id), true) => {
                let form = [("artistIds", id.to_string()), fail()];
                self.post_form(&format!("{favorites}/artists"), &form).await
            }
            (Favorite::Track(id), false) => self.delete(&format!("{favorites}/tracks/{id}")).await,
            (Favorite::Album(id), false) => self.delete(&format!("{favorites}/albums/{id}")).await,
            (Favorite::Artist(id), false) => {
                self.delete(&format!("{favorites}/artists/{id}")).await
            }
            (Favorite::Mix(id), true) => {
                let form = [("mixIds", id.clone()), fail()];
                self.put_v2("favorites/mixes/add", &[], Some(&form)).await
            }
            (Favorite::Mix(id), false) => {
                let query = [("mixIds", id.clone())];
                self.put_v2("favorites/mixes/remove", &query, None).await
            }
            (Favorite::Playlist(uuid), true) => {
                let query = [("folderId", "root".to_owned()), ("uuids", uuid.clone())];
                let path = "my-collection/playlists/folders/add-favorites";
                self.put_v2(path, &query, None).await
            }
            (Favorite::Playlist(uuid), false) => {
                let query = [("trns", format!("trn:playlist:{uuid}"))];
                self.put_v2("my-collection/playlists/folders/remove", &query, None)
                    .await
            }
        }
    }

    /// A v1 favorites list, newest first: entries `{created, item}`, unparsable ones skipped.
    async fn favorites<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<T>> {
        let path = format!("users/{}/favorites/{kind}", self.session().user_id);
        let query = [
            ("order", "DATE".to_owned()),
            ("orderDirection", "DESC".to_owned()),
        ];
        let entries: Vec<Value> = self.paged_all(&path, &query, FAVORITES_LIMIT).await?;
        Ok(entries
            .into_iter()
            .filter_map(|mut e| parse(e.get_mut("item")?.take()))
            .collect())
    }

    /// Every entry of `page[key]` across a v2 list's pages, following `cursor` until null. The
    /// first request sends an empty `cursor`, as the official client does.
    async fn cursor_all(
        &self,
        path: &str,
        query: &[(&'static str, String)],
        key: &str,
    ) -> Result<Vec<Value>> {
        let (mut items, mut cursor) = (Vec::new(), String::new());
        for _ in 0..CURSOR_PAGES {
            let mut q = query.to_vec();
            q.push(("cursor", cursor));
            let mut page: Value = self.get_v2(path, &q).await?;
            items.extend(take_list(&mut page, key));
            match page["cursor"].as_str() {
                Some(next) if !next.is_empty() => cursor = next.to_owned(),
                _ => break,
            }
        }
        Ok(items)
    }
}

fn parse<T: DeserializeOwned>(v: Value) -> Option<T> {
    serde_json::from_value(v).ok()
}
