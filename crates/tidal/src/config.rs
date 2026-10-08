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
    pub api_v2_base: Url,
    /// `x-tidal-client-version` on v2: required there (400 without it), value unchecked.
    pub client_version: String,
    /// Standard query params, as the official desktop client sends them (S4).
    pub locale: String,
    pub device_type: String,
    /// v2 only.
    pub platform: String,
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
            api_v2_base: url("https://api.tidal.com/v2/"),
            client_version: "2026.9.30".into(),
            locale: "en_US".into(),
            device_type: "DESKTOP".into(),
            platform: "DESKTOP".into(),
            auth_base: url("https://auth.tidal.com/v1/"),
            login_base: url("https://login.tidal.com/"),
            image_base: url("https://resources.tidal.com/images/"),
            events_url: url("https://ec.tidal.com/api/event-batch"),
        }
    }
}
