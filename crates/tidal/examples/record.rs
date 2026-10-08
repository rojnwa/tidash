//! Records one live, read-only TIDAL GET for fixtures and spikes (run by hand, logged in):
//! `cargo run -p tidal --example record -- <v1|v2|openapi> <path?query> <name> ["Header: value"...]`
//!
//! `{uid}` in the path becomes the session's user id. The standard query params are added
//! unless the path already sets them; an extra header with an empty value drops a default one.
//! Writes `target/record/<name>.json` (body) and `<name>.txt` (status, URL, cache headers).
//! Never prints or writes the token.
#![allow(clippy::print_stdout)]

use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

type BoxError = Box<dyn std::error::Error>;

const CLIENT_VERSION: &str = "2026.9.30";

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [api, path, name, headers @ ..] = args.as_slice() else {
        return Err(
            "usage: record <v1|v2|openapi> <path?query> <name> [\"Header: value\"...]".into(),
        );
    };
    // Blocking keyring read, before the async runtime exists.
    let (store, session) = tidal::default_store();
    let session = session?.ok_or("not logged in")?;
    tokio::runtime::Runtime::new()?.block_on(run(api, path, name, headers, store, session))
}

async fn run(
    api: &str,
    path: &str,
    name: &str,
    headers: &[String],
    store: Arc<dyn tidal::TokenStore>,
    session: tidal::Session,
) -> Result<(), BoxError> {
    let uid = session.user_id;
    let client = tidal::Client::new(tidal::ClientConfig::default(), session, store, None)?;
    // Refreshes (and saves) an expired token before the raw request.
    let _: serde_json::Value = client.get(&format!("users/{uid}"), &[]).await?;
    let session = client.session();

    let std_v1 = [("locale", "en_US"), ("deviceType", "DESKTOP")];
    let (base, std): (&str, &[(&str, &str)]) = match api {
        "v1" => ("https://api.tidal.com/v1/", &std_v1),
        "v2" => (
            "https://api.tidal.com/v2/",
            &[
                ("locale", "en_US"),
                ("deviceType", "DESKTOP"),
                ("platform", "DESKTOP"),
            ],
        ),
        "openapi" => ("https://openapi.tidal.com/v2/", &[]),
        _ => return Err("api must be v1, v2 or openapi".into()),
    };
    let base = url::Url::parse(base)?;
    let mut url = base.join(&path.replace("{uid}", &uid.to_string()))?;
    // An absolute path would take the bearer token to another host.
    if url.origin() != base.origin() {
        return Err("the path must stay on the API host".into());
    }
    let present: HashSet<String> = url.query_pairs().map(|(k, _)| k.into_owned()).collect();
    let cc = [("countryCode", session.country_code.as_str())];
    for (k, v) in cc.iter().chain(std) {
        if !present.contains(*k) {
            url.query_pairs_mut().append_pair(k, v);
        }
    }

    let mut hdrs = HeaderMap::new();
    let accept = if api == "openapi" {
        "application/vnd.api+json"
    } else {
        "application/json"
    };
    hdrs.insert(ACCEPT, HeaderValue::from_static(accept));
    if api == "v2" {
        hdrs.insert(
            "x-tidal-client-version",
            HeaderValue::from_static(CLIENT_VERSION),
        );
    }
    for h in headers {
        let (k, v) = h.split_once(':').ok_or("a header is `Name: value`")?;
        let k: HeaderName = k.trim().parse()?;
        if v.trim().is_empty() {
            hdrs.remove(&k);
        } else {
            hdrs.insert(k, v.trim().parse()?);
        }
    }

    let resp = reqwest::Client::new()
        .get(url.clone())
        .headers(hdrs)
        .bearer_auth(&session.access_token)
        .send()
        .await?;
    let status = resp.status().as_u16();
    let mut meta = format!("status: {status}\nurl: {url}\n");
    for h in ["cache-control", "etag", "vary", "content-type"] {
        if let Some(v) = resp.headers().get(h) {
            writeln!(meta, "{h}: {}", v.to_str().unwrap_or("?"))?;
        }
    }
    let body = resp.bytes().await?;
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/record");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{name}.json")), &body)?;
    std::fs::write(dir.join(format!("{name}.txt")), meta)?;
    println!("{name}: {status} ({} bytes)", body.len());
    Ok(())
}
