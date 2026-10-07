//! Crate error type.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("tidal returned status {status} (sub-status {sub_status:?}): {message}")]
    Status {
        status: u16,
        sub_status: Option<u32>,
        message: String,
    },
    #[error("auth error: {0}")]
    Auth(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("token store error: {0}")]
    Store(String),
}

pub type Result<T> = std::result::Result<T, Error>;
