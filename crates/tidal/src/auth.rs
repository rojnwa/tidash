//! PKCE login helpers.

use crate::{ClientConfig, Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngExt;
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
    /// `client_unique_key`: 64 random bits as 16 lowercase hex chars, sent on authorize and token exchange.
    pub unique_key: String,
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

impl Pkce {
    pub fn new() -> Self {
        let mut rng = rand::rng();
        let mut verifier = [0u8; 32];
        let mut state = [0u8; 16];
        rng.fill(&mut verifier);
        rng.fill(&mut state);
        let key: u64 = rng.random();
        let mut p = Self::from_parts(&b64(&verifier), &b64(&state));
        p.unique_key = format!("{key:016x}");
        p
    }

    /// Deterministic constructor for tests; `unique_key` is fixed.
    pub fn from_parts(verifier: &str, state: &str) -> Self {
        Self {
            verifier: verifier.into(),
            challenge: b64(&Sha256::digest(verifier.as_bytes())),
            state: state.into(),
            unique_key: "0000000000000000".into(),
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

pub fn authorize_url(cfg: &ClientConfig, pkce: &Pkce) -> Url {
    // The base is a fixed `https://.../` URL, so joining a literal cannot fail.
    let mut url = cfg.login_base.join("authorize").expect("valid join");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &cfg.redirect_uri)
        .append_pair("client_id", &cfg.client_id)
        .append_pair("lang", "EN")
        .append_pair("appMode", "android")
        .append_pair("client_unique_key", &pkce.unique_key)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("restrict_signup", "true")
        .append_pair("state", &pkce.state);
    url
}

/// Extract the authorization code from the pasted redirect URL, checking `state`.
pub fn code_from_redirect(redirect: &str, expected_state: &str) -> Result<String> {
    let url = Url::parse(redirect.trim())
        .map_err(|e| Error::Auth(format!("not a valid redirect URL: {e}")))?;
    let (mut code, mut state, mut error) = (None, None, None);
    for (k, v) in url.query_pairs() {
        match &*k {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if let Some(e) = error {
        return Err(Error::Auth(format!("login failed: {e}")));
    }
    let code = code.ok_or_else(|| Error::Auth("redirect URL has no code parameter".into()))?;
    if state.as_deref() != Some(expected_state) {
        return Err(Error::Auth("state mismatch in redirect URL".into()));
    }
    Ok(code)
}
#[cfg(test)]
mod tests {
    use crate::{ClientConfig, Error, Pkce, authorize_url, code_from_redirect};
    use std::collections::HashMap;

    #[test]
    fn challenge_matches_rfc7636_vector() {
        let p = Pkce::from_parts("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", "s");
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn authorize_url_carries_pkce_params() {
        let cfg = ClientConfig::default();
        let p = Pkce::from_parts("v", "st");
        let q: HashMap<_, _> = authorize_url(&cfg, &p).query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], p.challenge);
        assert_eq!(q["state"], "st");
        assert_eq!(q["client_id"], cfg.client_id);
        assert_eq!(q["redirect_uri"], cfg.redirect_uri);
        assert_eq!(q["client_unique_key"], p.unique_key);
        assert_eq!(q["appMode"], "android");
        assert_eq!(q["lang"], "EN");
        assert_eq!(q["restrict_signup"], "true");
        assert!(!q.contains_key("scope"));
    }

    #[test]
    fn redirect_code_extracted_and_state_checked() {
        assert_eq!(
            code_from_redirect("https://x.test/cb?code=abc&state=st", "st").unwrap(),
            "abc"
        );
        assert_eq!(
            code_from_redirect("  https://x.test/cb?code=abc&state=st\n", "st").unwrap(),
            "abc"
        );
        assert!(matches!(
            code_from_redirect("https://x.test/cb?code=abc&state=evil", "st"),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            code_from_redirect("https://x.test/cb?error=access_denied&state=st", "st"),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            code_from_redirect("not a url", "st"),
            Err(Error::Auth(_))
        ));
    }

    #[test]
    fn verifiers_are_unique_and_long_enough() {
        let (a, b) = (Pkce::new(), Pkce::new());
        assert_ne!(a.verifier, b.verifier);
        assert!((43..=128).contains(&a.verifier.len()));
        assert_eq!(a.unique_key.len(), 16);
        assert!(
            a.unique_key
                .chars()
                .all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        );
    }
}
