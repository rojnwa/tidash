//! Crate error type.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Without the request's URL (its query has the user's country), as errors reach the UI.
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),
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
    /// A cache-only GET found no cached copy.
    #[error("not in the cache")]
    NotCached,
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e.without_url())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
