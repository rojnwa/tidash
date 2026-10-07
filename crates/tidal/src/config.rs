//! Client identity and service endpoints (values from docs/spikes/s1-auth-manifests.md).

use url::Url;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    /// Sent literally in the token exchange; the form encoder escapes the plus signs.
    pub scopes: String,
    pub api_base: Url,
    pub auth_base: Url,
    pub login_base: Url,
    pub image_base: Url,
    /// Play-event (SQS batch) endpoint.
    pub events_url: Url,
}

impl Default for ClientConfig {
    fn default() -> Self {
        // Constants are valid URLs; the unit tests exercise this path.
        let url = |s: &str| Url::parse(s).expect("valid constant URL");
        Self {
            client_id: "6BDSRdpK9hqEBTgU".into(),
            client_secret: None,
            redirect_uri: "https://tidal.com/android/login/auth".into(),
            scopes: "r_usr+w_usr+w_sub".into(),
            api_base: url("https://api.tidal.com/v1/"),
            auth_base: url("https://auth.tidal.com/v1/"),
            login_base: url("https://login.tidal.com/"),
            image_base: url("https://resources.tidal.com/images/"),
            events_url: url("https://ec.tidal.com/api/event-batch"),
        }
    }
}
