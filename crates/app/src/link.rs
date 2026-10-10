//! TIDAL links from outside the app: a launch's argument, a later launch (`instance`), MPRIS
//! `OpenUri`. Parsed strictly, as their ids go into authenticated API paths.

use reqwest::Url;

/// Longer input is not a link.
const MAX_LEN: usize = 2048;
/// The web hosts whose links open here; exactly these, no other subdomain.
const HOSTS: [&str; 3] = ["tidal.com", "www.tidal.com", "listen.tidal.com"];

/// What a link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Track(u64),
    Album(u64),
    Artist(u64),
    Playlist(String),
    Mix(String),
}

/// `tidal://<kind>/<id>`, or a web link on `HOSTS` with an optional `browse/`; the query, the
/// fragment and any segment after the id are dropped. Ids: track, album and artist decimal,
/// a playlist 36 of `[0-9a-f-]`, a mix 1–64 of `[0-9a-zA-Z]`, so a percent-escape never
/// passes. `None` for anything else, and for input over `MAX_LEN` bytes.
pub fn parse(text: &str) -> Option<Link> {
    if text.len() > MAX_LEN {
        return None;
    }
    let url = Url::parse(text).ok()?;
    let host = url.host_str()?;
    let mut path = url.path_segments()?;
    let kind = match url.scheme() {
        "tidal" => host,
        "http" | "https" if HOSTS.contains(&host) => match path.next()? {
            "browse" => path.next()?,
            kind => kind,
        },
        _ => return None,
    };
    let id = path.next()?;
    let uuid = || {
        id.bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
    };
    let mix = || id.bytes().all(|b| b.is_ascii_alphanumeric());
    match kind {
        "track" => number(id).map(Link::Track),
        "album" => number(id).map(Link::Album),
        "artist" => number(id).map(Link::Artist),
        "playlist" if id.len() == 36 && uuid() => Some(Link::Playlist(id.into())),
        "mix" if (1..=64).contains(&id.len()) && mix() => Some(Link::Mix(id.into())),
        _ => None,
    }
}

/// A decimal id: digits only, as `u64::from_str` takes a sign too.
fn number(id: &str) -> Option<u64> {
    id.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| id.parse().ok())?
}

/// The link as `tidal://<kind>/<id>`, which `parse` reads back.
pub fn to_uri(link: &Link) -> String {
    match link {
        Link::Track(id) => format!("tidal://track/{id}"),
        Link::Album(id) => format!("tidal://album/{id}"),
        Link::Artist(id) => format!("tidal://artist/{id}"),
        Link::Playlist(id) => format!("tidal://playlist/{id}"),
        Link::Mix(id) => format!("tidal://mix/{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "0f6e1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";

    #[test]
    fn parse_accepts_known_forms() {
        let playlist = || Link::Playlist(UUID.into());
        let mix = || Link::Mix("001a520287326eaf8639f1b722a74f".into());
        let (app, web) = (
            format!("tidal://playlist/{UUID}"),
            format!("https://tidal.com/browse/playlist/{UUID}"),
        );
        let forms = [
            ("tidal://track/123", Link::Track(123)),
            ("tidal://album/123", Link::Album(123)),
            ("tidal://artist/123", Link::Artist(123)),
            (app.as_str(), playlist()),
            ("tidal://mix/001a520287326eaf8639f1b722a74f", mix()),
            ("https://tidal.com/browse/track/123", Link::Track(123)),
            ("https://tidal.com/browse/album/123", Link::Album(123)),
            ("https://tidal.com/browse/artist/123", Link::Artist(123)),
            (web.as_str(), playlist()),
            (
                "https://tidal.com/browse/mix/001a520287326eaf8639f1b722a74f",
                mix(),
            ),
            ("https://tidal.com/track/123", Link::Track(123)),
            ("https://tidal.com/track/123/u", Link::Track(123)),
            ("https://tidal.com/browse/track/123?u", Link::Track(123)),
            ("https://tidal.com/album/123/u?u#top", Link::Album(123)),
            ("https://listen.tidal.com/album/123", Link::Album(123)),
            ("https://www.tidal.com/album/123", Link::Album(123)),
            ("http://tidal.com/artist/123", Link::Artist(123)),
            ("tidal://track/123?u", Link::Track(123)),
        ];
        for (text, want) in forms {
            assert_eq!(parse(text), Some(want.clone()), "{text}");
            assert_eq!(parse(&to_uri(&want)), Some(want), "{text}: canonical");
        }
        assert_eq!(to_uri(&Link::Album(123)), "tidal://album/123");
        assert_eq!(to_uri(&playlist()), format!("tidal://playlist/{UUID}"));
    }

    #[test]
    fn parse_rejects_hostile_input() {
        let long = format!("https://tidal.com/album/1?{}", "a".repeat(20_000));
        let long_mix = format!("tidal://mix/{}", "a".repeat(65));
        let hostile = [
            "tidal://track/1%2F..%2Fusers",
            "tidal://track/abc",
            "tidal://track/+1",
            "tidal://track/18446744073709551616", // past u64
            "https://evil.com/track/1",
            "https://tidal.com.evil.com/album/1",
            "https://tidal.com@evil.com/album/1",
            "https://evil.com/?tidal.com/album/1",
            "tidal://video/1",
            "https://tidal.com/video/1",
            long.as_str(),
            "tidal://album/",
            "https://tidal.com/album/",
            "https://tidal.com/album",
            "https://tidal.com/browse/",
            "tidal://playlist/0F6E1A2B-3C4D-4E5F-8A9B-0C1D2E3F4A5B", // not lowercase
            "tidal://playlist/0f6e1a2b",
            "tidal://playlist/0f6e1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5%2f",
            "tidal://mix/abc%41",
            long_mix.as_str(),
            "spotify:track:1",
            "file:///etc/passwd",
            "",
            "rm -rf",
        ];
        for text in hostile {
            let shown = &text[..text.len().min(60)];
            assert_eq!(parse(text), None, "{shown}");
        }
    }
}
