//! TIDAL service client: auth, catalog, browse pages, collection, playlist and folder editing,
//! lyrics and credits, and playback manifests.

mod auth;
mod browse;
mod cache;
mod catalog;
mod client;
mod collection;
mod config;
mod edit;
mod error;
mod events;
mod image;
mod lyrics;
mod playback;
mod store;

pub use auth::{Pkce, Session, authorize_url, code_from_redirect, exchange_code, refresh};
pub use browse::{ArtistPage, Item, Layout, More, Page, Paged, PlaylistItems, Shelf};
pub use cache::{DiskCache, Entry};
pub use catalog::{Album, AlbumRef, Artist, Mix, Mixes, Playlist, SearchResults, Track};
pub use client::Client;
pub use collection::{Favorite, FavoriteIds};
pub use config::ClientConfig;
pub use edit::FolderItem;
pub use error::{Error, Result};
pub use events::{PlaybackReport, new_session_id};
pub use image::{Image, ImageKind};
pub use lyrics::{Contributor, Credit, LyricLine, Lyrics, parse_lrc};
pub use playback::{Failure, Quality, StreamInfo, classify, dash_segments};
pub use store::{KeyringStore, MemoryStore, TokenStore, default_store};
