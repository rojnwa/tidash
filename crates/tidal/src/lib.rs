//! TIDAL service client: auth, catalog and playback manifests.

mod auth;
mod config;
mod error;

pub use auth::{Pkce, authorize_url, code_from_redirect};
pub use config::ClientConfig;
pub use error::{Error, Result};
