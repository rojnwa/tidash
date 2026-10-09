//! Stream resolution: a track's stream info, and the segment reader the player reads it from.

use player::Source;
use std::io::{self, Read};
use std::sync::OnceLock;
use std::time::Duration;
use tidal::{Client, Failure, Quality, StreamInfo, classify};

type Body = Box<dyn Read + Send>;
type Open = Box<dyn FnMut(&str) -> io::Result<Body> + Send>;

/// Reads each URL's body in order, as one stream; the next is opened when the current ends.
pub struct SegmentReader {
    urls: std::vec::IntoIter<String>,
    open: Open,
    body: Option<Body>,
}

impl SegmentReader {
    pub fn new(
        urls: Vec<String>,
        open: impl FnMut(&str) -> io::Result<Body> + Send + 'static,
    ) -> SegmentReader {
        SegmentReader {
            urls: urls.into_iter(),
            open: Box::new(open),
            body: None,
        }
    }
}

impl Read for SegmentReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(body) = &mut self.body {
                let n = body.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
                self.body = None;
            }
            let Some(url) = self.urls.next() else {
                return Ok(0);
            };
            self.body = Some((self.open)(&url)?);
        }
    }
}

/// Segment downloads. Blocking, so it is built in `spawn_blocking` and used only on the
/// player's download thread; a static is never dropped, so never on a runtime thread either.
static SEGMENTS: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// The player source for a track's segment URLs, at `gain` (linear). Call off the async
/// runtime.
pub(super) fn segment_source(id: u64, urls: Vec<String>, gain: f32) -> io::Result<Source> {
    let http = match SEGMENTS.get() {
        Some(http) => http,
        None => {
            let http = reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                // The blocking client applies this to the send and to each read, not the whole body.
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(io::Error::other)?;
            SEGMENTS.get_or_init(|| http)
        }
    };
    // Segment URLs are pre-signed (S1): no Authorization header.
    let open = move |url: &str| -> io::Result<Body> {
        // The URL carries its signature: keep it out of errors, which get logged.
        let resp = http
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| io::Error::other(e.without_url()))?;
        Ok(Box::new(resp))
    };
    Ok(Source {
        id,
        reader: Box::new(SegmentReader::new(urls, open)),
        len: None,
        gain,
    })
}

/// `stream_info`, retried once after 2 s unless the failure is terminal.
pub(super) async fn stream_info(
    c: &Client,
    id: u64,
    quality: Quality,
) -> tidal::Result<StreamInfo> {
    match c.stream_info(id, quality).await {
        Err(e) if classify(&e) == Failure::Retryable => {
            log::debug!("stream info for {id}: {e}; retrying");
            tokio::time::sleep(Duration::from_secs(2)).await;
            c.stream_info(id, quality).await
        }
        r => r,
    }
}

/// A track's stream, ready for the player.
pub struct Resolved {
    pub(super) source: Source,
    pub(super) quality: String,
    pub(super) codec: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    /// A reader over `segments` (url -> bytes) whose `open` logs each URL and fails on `bad`.
    fn reader(
        segments: &[(&str, &[u8])],
        bad: Option<&'static str>,
    ) -> (SegmentReader, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let data: Vec<(String, Vec<u8>)> = segments
            .iter()
            .map(|(u, b)| (u.to_string(), b.to_vec()))
            .collect();
        let urls = data.iter().map(|(u, _)| u.clone()).collect();
        let opened = log.clone();
        let r = SegmentReader::new(urls, move |url| {
            opened.lock().expect("log").push(url.to_string());
            if Some(url) == bad {
                return Err(io::Error::other("403"));
            }
            let bytes = data
                .iter()
                .find(|(u, _)| u == url)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            Ok(Box::new(Cursor::new(bytes)) as Body)
        });
        (r, log)
    }

    #[test]
    fn segment_reader_concatenates_in_order() {
        let (mut r, log) = reader(&[("init", b"ab"), ("1", b"cde"), ("2", b"f")], None);
        assert!(
            log.lock().expect("log").is_empty(),
            "nothing opened before the first read"
        );
        let mut first = [0u8; 1];
        assert_eq!(r.read(&mut first).expect("read"), 1);
        assert_eq!(*log.lock().expect("log"), ["init"], "opened lazily");
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).expect("read_to_end");
        assert_eq!([&first[..], &rest].concat(), b"abcdef");
        assert_eq!(*log.lock().expect("log"), ["init", "1", "2"]);
    }

    #[test]
    fn segment_reader_surfaces_open_error() {
        let (mut r, _) = reader(&[("init", b"ab"), ("1", b"cd"), ("2", b"ef")], Some("1"));
        let mut got = Vec::new();
        assert!(r.read_to_end(&mut got).is_err());
        assert_eq!(
            got, b"ab",
            "the first segment's bytes come before the error"
        );
    }
}
