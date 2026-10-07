//! TIDAL service client: auth, catalog and playback manifests.

mod auth;
mod catalog;
mod client;
mod config;
mod error;

pub use auth::{Pkce, Session, authorize_url, code_from_redirect, exchange_code, refresh};
pub use catalog::{Album, AlbumRef, Artist, SearchResults, Track, image_url};
pub use client::Client;
pub use config::ClientConfig;
pub use error::{Error, Result};

mod store;
pub use store::{KeyringStore, MemoryStore, TokenStore, default_store};
