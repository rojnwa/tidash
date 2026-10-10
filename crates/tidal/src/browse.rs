//! Browse pages as one model, a [`Page`] of [`Shelf`]s of [`Item`]s, whichever of TIDAL's two
//! page schemas they come from: v1 `pages/*` documents or v2 module feeds (S4).
//!
//! `type` strings are dispatched by hand on [`Value`], so an unknown module or item type, or
//! one item that fails to parse, is skipped and never fails the page.

use crate::{Album, Artist, Client, Mix, Playlist, Result, Track};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::task::JoinSet;

/// The largest page v1 `pages/data` serves; v2 view-alls use the same (S4).
const PAGE_LIMIT: usize = 50;
/// The most items a "View all" loads.
const VIEW_ALL_CAP: usize = 500;
/// Home feed requests, following `page.cursor`.
const HOME_REQUESTS: usize = 5;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Album(Album),
    Artist(Artist),
    Playlist(Playlist),
    Mix(Mix),
    Track(Track),
    /// A v1 page, for [`Client::page`].
    Link {
        title: String,
        api_path: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Cards,
    Tracks,
    Links,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum More {
    /// A v1 `apiPath`, for [`Client::page`].
    Page(String),
    /// A v2 `viewAll` path, for [`Client::view_all`].
    ViewAll(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shelf {
    pub title: String,
    pub layout: Layout,
    pub items: Vec<Item>,
    pub more: Option<More>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub title: String,
    pub shelves: Vec<Shelf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistPage {
    pub artist: Artist,
    pub bio: Option<String>,
    pub shelves: Vec<Shelf>,
}

/// A playlist's tracks, ready for editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaylistItems {
    pub tracks: Vec<Track>,
    /// Each track's 0-based position in the whole list, as item writes address them (S5);
    /// videos and broken tracks are skipped but counted.
    pub positions: Vec<u32>,
    /// The listing's ETag (its first page's), the guard for edits by position; `None` if
    /// TIDAL sent none.
    pub etag: Option<String>,
}

/// One page of a v1 list.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Paged<T> {
    #[serde(default = "Vec::new")]
    pub items: Vec<T>,
    #[serde(rename = "totalNumberOfItems", default)]
    pub total: usize,
}

impl Client {
    /// The v2 home feed: all its modules, following `page.cursor` (S4: `limit` counts modules,
    /// so one request usually holds the whole feed).
    pub async fn home(&self) -> Result<Page> {
        let mut shelves = Vec::new();
        let mut cursor = None;
        for _ in 0..HOME_REQUESTS {
            let mut query = vec![("limit", "50".to_owned()), ("timeOffset", time_offset())];
            query.extend(cursor.take().map(|c| ("cursor", c)));
            let mut feed: Value = self.get_v2("home/feed/static", &query).await?;
            shelves.extend(v2_shelves(take_list(&mut feed, "items")));
            cursor = feed["page"]["cursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(Page {
            title: String::new(),
            shelves,
        })
    }

    /// A v1 page (`pages/explore`, a genre, a `single-module-page` "View all"). A page of one
    /// module holds its first items; the rest come from its `dataApiPath`, up to 500.
    pub async fn page(&self, api_path: &str) -> Result<Page> {
        let mut doc: Value = self.get(api_path, &[]).await?;
        let mut modules = modules(&mut doc);
        if let [module] = &mut modules[..]
            && let Some(list) = module.get_mut("pagedList")
        {
            self.fill(list).await?;
        }
        Ok(Page {
            title: text(&doc["title"]),
            shelves: modules.into_iter().filter_map(v1_shelf).collect(),
        })
    }

    /// Appends to a v1 `pagedList` the items it lacks, from `dataApiPath`.
    async fn fill(&self, list: &mut Value) -> Result<()> {
        let count = |key: &str| list[key].as_u64().unwrap_or(0) as usize;
        let (total, limit) = (count("totalNumberOfItems"), count("limit"));
        let Some(path) = list["dataApiPath"].as_str().map(str::to_owned) else {
            return Ok(());
        };
        let Some(Value::Array(items)) = list.get_mut("items") else {
            return Ok(());
        };
        // Pages may run short, so the next one starts at `limit`, not at the items held.
        let start = items.len().max(limit);
        let offsets = (start..total.min(VIEW_ALL_CAP)).step_by(PAGE_LIMIT);
        items.extend(
            self.pages_at::<Value>(&path, &[], PAGE_LIMIT, offsets)
                .await?,
        );
        items.truncate(VIEW_ALL_CAP);
        Ok(())
    }

    /// A v2 `viewAll` path (`home/pages/X/view-all?itemId=…`, `artist/X/view-all?artistId=…`)
    /// as one shelf. No total comes back: a short page is the end (S4).
    pub async fn view_all(&self, path: &str) -> Result<Page> {
        let (mut title, mut items) = (String::new(), Vec::new());
        for offset in (0..VIEW_ALL_CAP).step_by(PAGE_LIMIT) {
            let query = [
                ("limit", PAGE_LIMIT.to_string()),
                ("offset", offset.to_string()),
            ];
            let mut page: Value = self.get_v2(path, &query).await?;
            if offset == 0 {
                title = text(&page["title"]);
            }
            let got = take_list(&mut page, "items");
            let short = got.len() < PAGE_LIMIT;
            items.extend(got.into_iter().filter_map(typed));
            if short {
                break;
            }
        }
        let layout = if items.iter().all(|i| matches!(i, Item::Track(_))) {
            Layout::Tracks
        } else {
            Layout::Cards
        };
        Ok(Page {
            title,
            shelves: shelf(String::new(), layout, items, None)
                .into_iter()
                .collect(),
        })
    }

    pub async fn artist_page(&self, id: u64) -> Result<ArtistPage> {
        let mut doc: Value = self.get_v2(&format!("artist/{id}"), &[]).await?;
        let artist = doc
            .pointer_mut("/item/data")
            .map(Value::take)
            .unwrap_or_default();
        Ok(ArtistPage {
            artist: serde_json::from_value(artist)?,
            bio: doc
                .pointer("/header/biography/text")
                .and_then(Value::as_str)
                .map(clean_bio)
                .filter(|b| !b.is_empty()),
            shelves: v2_shelves(take_list(&mut doc, "items")),
        })
    }

    pub async fn playlist(&self, uuid: &str) -> Result<Playlist> {
        self.get(&format!("playlists/{uuid}"), &[]).await
    }

    /// Its tracks in playlist order, with their positions and the listing's ETag for edits.
    // ponytail: positions count the items received; if TIDAL drops unavailable ones from a
    // page, as it does in favorites (S4), later positions shift. Anchor each page at its offset then.
    pub async fn playlist_items(&self, uuid: &str) -> Result<PlaylistItems> {
        let query = [
            ("order", "INDEX".to_owned()),
            ("orderDirection", "ASC".to_owned()),
        ];
        let (items, etag): (Vec<Value>, _) = self
            .paged_all_tagged(&format!("playlists/{uuid}/items"), &query, 100)
            .await?;
        let (tracks, positions) = items
            .into_iter()
            .zip(0..)
            .filter_map(|(e, i)| match typed(e) {
                Some(Item::Track(t)) => Some((t, i)),
                _ => None,
            })
            .unzip();
        Ok(PlaylistItems {
            tracks,
            positions,
            etag,
        })
    }

    /// The mix and all its tracks, from one request: `pages/mix` embeds them all (S4).
    pub async fn mix(&self, id: &str) -> Result<(Mix, Vec<Track>)> {
        let mut doc: Value = self.get("pages/mix", &[("mixId", id.to_owned())]).await?;
        let (mut mix, mut tracks) = (Value::Null, Vec::new());
        for mut m in modules(&mut doc) {
            match m["type"].as_str() {
                Some("MIX_HEADER") => mix = m.get_mut("mix").map(Value::take).unwrap_or_default(),
                Some("TRACK_LIST") => {
                    tracks = paged_items(&mut m)
                        .into_iter()
                        .filter_map(|t| serde_json::from_value(t).ok())
                        .collect();
                }
                _ => {}
            }
        }
        Ok((serde_json::from_value(mix)?, tracks))
    }

    /// Every item of a v1 list. The first page gives the total; the others, at offsets
    /// `limit, 2·limit, …`, run concurrently. Pages may hold fewer than `limit` (S4), so the
    /// offset steps by `limit`, never by the items received.
    pub(crate) async fn paged_all<T: DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
        query: &[(&'static str, String)],
        limit: usize,
    ) -> Result<Vec<T>> {
        Ok(self.paged_all_tagged(path, query, limit).await?.0)
    }

    /// [`Client::paged_all`] and the first page's ETag. That page is fetched before the
    /// others, so its ETag is the oldest: a change while paging only makes it stale.
    pub(crate) async fn paged_all_tagged<T: DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
        query: &[(&'static str, String)],
        limit: usize,
    ) -> Result<(Vec<T>, Option<String>)> {
        let (first, etag): (Paged<T>, _) =
            self.get_tagged(path, &page_query(query, limit, 0)).await?;
        let mut items = first.items;
        let rest = (limit..first.total).step_by(limit);
        items.extend(self.pages_at(path, query, limit, rest).await?);
        Ok((items, etag))
    }

    /// The pages of `path` at `offsets`, concurrently on clones of the client; items in
    /// offset order.
    async fn pages_at<T: DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
        query: &[(&'static str, String)],
        limit: usize,
        offsets: impl Iterator<Item = usize>,
    ) -> Result<Vec<T>> {
        let mut set = JoinSet::new();
        for (i, offset) in offsets.enumerate() {
            let (client, path) = (self.clone(), path.to_owned());
            let query = page_query(query, limit, offset);
            set.spawn(async move { (i, client.get::<Paged<T>>(&path, &query).await) });
        }
        let mut pages = set.join_all().await;
        pages.sort_by_key(|(i, _)| *i);
        let mut items = Vec::new();
        for (_, page) in pages {
            items.extend(page?.items);
        }
        Ok(items)
    }
}

fn page_query(
    query: &[(&'static str, String)],
    limit: usize,
    offset: usize,
) -> Vec<(&'static str, String)> {
    let mut query = query.to_vec();
    query.extend([("limit", limit.to_string()), ("offset", offset.to_string())]);
    query
}

/// The tracks among v1 list entries `{type, item}`; videos and broken tracks are skipped.
pub(crate) fn tracks(entries: Vec<Value>) -> Vec<Track> {
    entries
        .into_iter()
        .filter_map(|e| match typed(e) {
            Some(Item::Track(t)) => Some(t),
            _ => None,
        })
        .collect()
}

/// An entity by its `type` (any case), or `None`.
fn item(kind: &str, data: Value) -> Option<Item> {
    fn de<T: DeserializeOwned>(v: Value) -> Option<T> {
        serde_json::from_value(v).ok()
    }
    Some(match kind.to_ascii_uppercase().as_str() {
        "ALBUM" => Item::Album(de(data)?),
        "ARTIST" => Item::Artist(de(data)?),
        "PLAYLIST" => Item::Playlist(de(data)?),
        "MIX" => Item::Mix(de(data)?),
        "TRACK" => Item::Track(de(data)?),
        _ => return None,
    })
}

/// `{type, data}` (v2) or `{type, item}` (v1).
fn typed(v: Value) -> Option<Item> {
    let Value::Object(mut o) = v else {
        return None;
    };
    let data = o.remove("data").or_else(|| o.remove("item"))?;
    item(o.get("type")?.as_str()?, data)
}

fn link(v: Value) -> Option<Item> {
    Some(Item::Link {
        title: text(&v["title"]),
        api_path: v["apiPath"].as_str()?.to_owned(),
    })
}

/// `None` when empty: empty shelves are dropped.
fn shelf(title: String, layout: Layout, items: Vec<Item>, more: Option<More>) -> Option<Shelf> {
    (!items.is_empty()).then_some(Shelf {
        title,
        layout,
        items,
        more,
    })
}

fn v2_shelves(modules: Vec<Value>) -> Vec<Shelf> {
    let view_all = |v: &Value| v.as_str().map(|p| More::ViewAll(p.to_owned()));
    let typed_list = |v: &mut Value| -> Vec<Item> {
        take_list(v, "items")
            .into_iter()
            .filter_map(typed)
            .collect()
    };
    let mut shelves = Vec::new();
    for mut m in modules {
        let layout = match m["type"].as_str().unwrap_or_default() {
            "HORIZONTAL_LIST"
            | "HORIZONTAL_LIST_WITH_CONTEXT"
            | "SHORTCUT_LIST"
            | "COMPACT_GRID_CARD"
            | "GRID_CARD" => Layout::Cards,
            "TRACK_LIST" => Layout::Tracks,
            "DISCOGRAPHY_TABS" => {
                for mut tab in take_list(&mut m, "tabs") {
                    let (title, more) = (text(&tab["label"]), view_all(&tab["viewAll"]));
                    shelves.extend(shelf(title, Layout::Cards, typed_list(&mut tab), more));
                }
                continue;
            }
            _ => continue,
        };
        let (title, more) = (text(&m["title"]), view_all(&m["viewAll"]));
        shelves.extend(shelf(title, layout, typed_list(&mut m), more));
    }
    shelves
}

fn v1_shelf(mut m: Value) -> Option<Shelf> {
    let kind = m["type"].as_str()?.to_owned();
    let (layout, items) = match kind.as_str() {
        "ALBUM_LIST" | "ARTIST_LIST" | "PLAYLIST_LIST" | "MIX_LIST" | "TRACK_LIST" => {
            let entity = kind.trim_end_matches("_LIST");
            let layout = if entity == "TRACK" {
                Layout::Tracks
            } else {
                Layout::Cards
            };
            let items = paged_items(&mut m);
            (
                layout,
                items.into_iter().filter_map(|v| item(entity, v)).collect(),
            )
        }
        "MIXED_TYPES_LIST" => {
            let items = paged_items(&mut m);
            (Layout::Cards, items.into_iter().filter_map(typed).collect())
        }
        "PAGE_LINKS" | "PAGE_LINKS_CLOUD" => {
            let items = paged_items(&mut m);
            (Layout::Links, items.into_iter().filter_map(link).collect())
        }
        "HIGHLIGHT_MODULE" => {
            let highlights = take_list(&mut m, "highlights");
            let items = highlights
                .into_iter()
                .filter_map(|mut h| typed(h.get_mut("item")?.take()));
            (Layout::Cards, items.collect())
        }
        _ => return None,
    };
    let more = m["showMore"]["apiPath"]
        .as_str()
        .map(|p| More::Page(p.to_owned()));
    shelf(text(&m["title"]), layout, items, more)
}

/// A v1 document's modules, all rows in order.
fn modules(doc: &mut Value) -> Vec<Value> {
    take_list(doc, "rows")
        .into_iter()
        .flat_map(|mut row| take_list(&mut row, "modules"))
        .collect()
}

fn paged_items(module: &mut Value) -> Vec<Value> {
    module
        .get_mut("pagedList")
        .map(|list| take_list(list, "items"))
        .unwrap_or_default()
}

/// `v[key]` taken out as a list; empty when absent or not a list.
pub(crate) fn take_list(v: &mut Value, key: &str) -> Vec<Value> {
    match v.get_mut(key).map(Value::take) {
        Some(Value::Array(list)) => list,
        _ => Vec::new(),
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// `<br/>` becomes a newline; `[wimpLink …]` and `[/wimpLink]` go, their text stays.
fn clean_bio(bio: &str) -> String {
    let bio = bio.replace("<br/>", "\n").replace("[/wimpLink]", "");
    let (mut out, mut rest) = (String::with_capacity(bio.len()), bio.as_str());
    while let Some(start) = rest.find("[wimpLink")
        && let Some(end) = rest[start..].find(']')
    {
        out.push_str(&rest[..start]);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// The local UTC offset, as the home feed's `timeOffset` wants it.
fn time_offset() -> String {
    format_offset(jiff::Zoned::now().offset().seconds())
}

/// `±HH:MM`.
fn format_offset(seconds: i32) -> String {
    let (sign, s) = (if seconds < 0 { '-' } else { '+' }, seconds.abs());
    format!("{sign}{:02}:{:02}", s / 3600, s % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::{clean_bio, format_offset};

    #[test]
    fn offsets_format_as_sign_hours_minutes() {
        assert_eq!(format_offset(0), "+00:00");
        assert_eq!(format_offset(19_800), "+05:30");
        assert_eq!(format_offset(-18_000), "-05:00");
        assert_eq!(format_offset(-9_000), "-02:30");
    }

    #[test]
    fn bio_markup_goes() {
        let bio = r#"A<br/>[wimpLink albumId="1"]B[/wimpLink] [wimpLink broken"#;
        assert_eq!(clean_bio(bio), "A\nB [wimpLink broken");
    }
}
