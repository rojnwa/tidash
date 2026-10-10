//! The user's playlists and folders: listing and editing them as the official client does
//! (S5). Openapi creates and edits playlists, v1 changes their items behind the playlist's
//! ETag, and v2 handles folders and deletes.

use crate::browse::take_list;
use crate::{Client, Error, Playlist, Result};
use reqwest::Method;
use serde_json::{Value, json};

const FOLDERS: &str = "my-collection/playlists/folders";
/// Track ids per add.
const ADD_BATCH: usize = 500;
/// TRNs per folder move, as the official client batches them.
const MOVE_BATCH: usize = 20;
/// The largest page of own playlists and folder listings (S4).
const PAGE_LIMIT: usize = 50;
/// Requests per folder listing.
// ponytail: a fixed cap, as for cursor lists; 200 pages is 10 000 items.
const FOLDER_PAGES: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub enum FolderItem {
    /// `count` is its direct children.
    Folder {
        id: String,
        name: String,
        count: u32,
    },
    Playlist(Playlist),
}

impl Client {
    /// The playlists the user made, last updated first.
    pub async fn own_playlists(&self) -> Result<Vec<Playlist>> {
        let path = format!("users/{}/playlists", self.session().user_id);
        let query = [
            ("order", "DATE_UPDATED".to_owned()),
            ("orderDirection", "DESC".to_owned()),
        ];
        let items: Vec<Value> = self.paged_all(&path, &query, PAGE_LIMIT).await?;
        Ok(items
            .into_iter()
            .filter_map(|p| serde_json::from_value(p).ok())
            .collect())
    }

    /// The folders and playlists (own and favorited) in folder `id`, or `"root"`, newest
    /// first. Listings carry no cursor (S5): a short page or the total ends them.
    pub async fn folder(&self, id: &str) -> Result<Vec<FolderItem>> {
        let (mut items, mut cursor) = (Vec::new(), String::new());
        for page in 0..FOLDER_PAGES {
            let query = [
                ("folderId", id.to_owned()),
                ("includeOnly", String::new()),
                ("order", "DATE".to_owned()),
                ("orderDirection", "DESC".to_owned()),
                ("limit", PAGE_LIMIT.to_string()),
                ("offset", (page * PAGE_LIMIT).to_string()),
                ("cursor", cursor.clone()),
            ];
            let mut doc: Value = self.get_v2(FOLDERS, &query).await?;
            let got = take_list(&mut doc, "items");
            let short = got.len() < PAGE_LIMIT;
            items.extend(got);
            let total = doc["totalNumberOfItems"].as_u64();
            if short || total.is_some_and(|t| items.len() as u64 >= t) {
                break;
            }
            if let Some(next) = doc["cursor"].as_str() {
                cursor = next.to_owned();
            }
        }
        Ok(items.into_iter().filter_map(folder_item).collect())
    }

    /// A new UNLISTED playlist, moved into `folder` if given; its uuid.
    pub async fn create_playlist(
        &self,
        title: &str,
        description: &str,
        folder: Option<&str>,
    ) -> Result<String> {
        let body = json!({"data": {"type": "playlists", "attributes": {
            "name": title, "description": description, "accessType": "UNLISTED"}}});
        let uuid = data_id(&self.json_api(Method::POST, "playlists", body).await?)?;
        if let Some(folder) = folder {
            self.move_trns(&[format!("trn:playlist:{uuid}")], folder)
                .await?;
        }
        Ok(uuid)
    }

    /// The answer holds the playlist as it was before the edit (S5), so it is ignored.
    pub async fn edit_playlist(&self, uuid: &str, title: &str, description: &str) -> Result<()> {
        let body = json!({"data": {"id": uuid, "type": "playlists",
            "attributes": {"name": title, "description": description}}});
        let path = format!("playlists/{uuid}");
        self.json_api(Method::PATCH, &path, body).await.map(drop)
    }

    /// Deletes the playlist itself, not just its place in the collection (S5).
    pub async fn delete_playlist(&self, uuid: &str) -> Result<()> {
        let query = [("trns", format!("trn:playlist:{uuid}"))];
        self.put_v2(&format!("{FOLDERS}/remove"), &query, None)
            .await
    }

    /// Appends the tracks, skipping those already in; the number added. 500 per request, each
    /// behind a fresh ETag; a stale one (412) is taken again once.
    pub async fn add_to_playlist(&self, uuid: &str, track_ids: &[u64]) -> Result<usize> {
        let mut added = 0;
        for ids in track_ids.chunks(ADD_BATCH) {
            let form = [
                ("onArtifactNotFound", "FAIL".to_owned()),
                ("onDupes", "SKIP".to_owned()),
                ("trackIds", csv(ids)),
            ];
            let add = || self.item_write(uuid, Method::POST, "", &[], Some(&form));
            let resp = match add().await {
                Err(Error::Status { status: 412, .. }) => add().await?,
                resp => resp?,
            };
            let body: Value = serde_json::from_slice(&resp.bytes().await?)?;
            added += body["addedItemIds"].as_array().map_or(0, Vec::len);
        }
        Ok(added)
    }

    /// Removes the items at `positions`, from [`Client::playlist_items`]. A stale ETag (412)
    /// is returned, not retried: the positions may be stale too.
    pub async fn remove_from_playlist(&self, uuid: &str, positions: &[u32]) -> Result<()> {
        if positions.is_empty() {
            return Ok(());
        }
        let query = [
            ("order", "INDEX".to_owned()),
            ("orderDirection", "ASC".to_owned()),
        ];
        let rest = format!("/{}", csv(positions));
        self.item_write(uuid, Method::DELETE, &rest, &query, None)
            .await
            .map(drop)
    }

    /// Moves the item at position `from` to the slot `to` (`0..=len`, counted before the
    /// move). TIDAL's `toIndex` is the position once the item is out (S5). A 412 is returned.
    pub async fn move_in_playlist(&self, uuid: &str, from: u32, to: u32) -> Result<()> {
        let to_index = if to > from { to - 1 } else { to };
        let form = [("toIndex", to_index.to_string())];
        let rest = format!("/{from}");
        self.item_write(uuid, Method::POST, &rest, &[], Some(&form))
            .await
            .map(drop)
    }

    /// A new folder in `parent` (`None`: at the top); its id. TIDAL creates every folder at
    /// the top, so one with a parent is moved there after (S5).
    pub async fn create_folder(&self, name: &str, parent: Option<&str>) -> Result<String> {
        let query = [
            ("folderId", "root".to_owned()),
            ("name", name.to_owned()),
            ("trns", String::new()),
        ];
        let path = format!("{FOLDERS}/create-folder");
        let resp = self
            .write(Method::PUT, &path, &query, None, true, None)
            .await?;
        let id = data_id(&serde_json::from_slice(&resp.bytes().await?)?)?;
        if let Some(parent) = parent {
            self.move_trns(&[format!("trn:folder:{id}")], parent)
                .await?;
        }
        Ok(id)
    }

    pub async fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        let query = [
            ("name", name.to_owned()),
            ("trn", format!("trn:folder:{id}")),
        ];
        self.put_v2(&format!("{FOLDERS}/rename"), &query, None)
            .await
    }

    /// Empty it first: what removing a folder does to the playlists in it is untested (S5).
    pub async fn delete_folder(&self, id: &str) -> Result<()> {
        let query = [("trns", format!("trn:folder:{id}"))];
        self.put_v2(&format!("{FOLDERS}/remove"), &query, None)
            .await
    }

    /// Moves playlists into `folder` (`None`: to the top).
    pub async fn move_to_folder(&self, uuids: &[String], folder: Option<&str>) -> Result<()> {
        let trns: Vec<String> = uuids.iter().map(|u| format!("trn:playlist:{u}")).collect();
        self.move_trns(&trns, folder.unwrap_or("root")).await
    }

    /// v2 `move` into folder id `folder` or `"root"`, 20 TRNs per request.
    async fn move_trns(&self, trns: &[String], folder: &str) -> Result<()> {
        for batch in trns.chunks(MOVE_BATCH) {
            let query = [("folderId", folder.to_owned()), ("trns", batch.join(","))];
            self.put_v2(&format!("{FOLDERS}/move"), &query, None)
                .await?;
        }
        Ok(())
    }

    /// A v1 write to `playlists/{uuid}/items{rest}` behind the playlist's current ETag.
    async fn item_write(
        &self,
        uuid: &str,
        method: Method,
        rest: &str,
        query: &[(&str, String)],
        form: Option<&[(&str, String)]>,
    ) -> Result<reqwest::Response> {
        let etag = self.etag(&format!("playlists/{uuid}")).await?;
        let path = format!("playlists/{uuid}/items{rest}");
        self.write(method, &path, query, form, false, Some(&etag))
            .await
    }
}

fn csv<T: ToString>(values: &[T]) -> String {
    let values: Vec<String> = values.iter().map(T::to_string).collect();
    values.join(",")
}

/// `data.id` of a created playlist (openapi) or folder (a v2 folder item).
fn data_id(created: &Value) -> Result<String> {
    Ok(serde_json::from_value(created["data"]["id"].clone())?)
}

/// A folder listing's item; other kinds and broken ones are skipped.
fn folder_item(mut v: Value) -> Option<FolderItem> {
    let data = v.get_mut("data")?.take();
    match v["itemType"].as_str()? {
        "FOLDER" => Some(FolderItem::Folder {
            id: data["id"].as_str()?.to_owned(),
            name: data["name"].as_str()?.to_owned(),
            count: data["totalNumberOfItems"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
        }),
        "PLAYLIST" => serde_json::from_value(data).ok().map(FolderItem::Playlist),
        _ => None,
    }
}
