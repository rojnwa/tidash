//! Size-capped LRU directory of byte blobs: API bodies (with their ETag) and artwork.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

const SWEEP_EVERY: u64 = 64;

/// Blocking file I/O: call from plain threads or `spawn_blocking`. Files are named by the
/// sha256 of their key, so no key text reaches the disk.
pub struct DiskCache {
    dir: PathBuf,
    cap: u64,
    ext: &'static str,
    writes: AtomicU64,
    /// Held across a whole `write`, so one write's body and ETag land together.
    // ponytail: in-process only; add a file lock if two app instances ever share the dir.
    write_lock: Mutex<()>,
}

#[derive(Debug, PartialEq)]
pub struct Entry {
    pub etag: Option<String>,
    pub body: Vec<u8>,
}

impl DiskCache {
    /// Creates `dir` and trims it to `cap_bytes`.
    pub fn open(dir: PathBuf, cap_bytes: u64, ext: &'static str) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let cache = Self {
            dir,
            cap: cap_bytes,
            ext,
            writes: AtomicU64::new(0),
            write_lock: Mutex::new(()),
        };
        cache.sweep()?;
        Ok(cache)
    }

    /// Where `key`'s body lives (present or not).
    pub fn path(&self, key: &str) -> PathBuf {
        let hex: String = Sha256::digest(key.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.dir.join(format!("{hex}.{}", self.ext))
    }

    /// Marks the entry recently used. The ETag is read before the body: writes are serialized
    /// and put the body first, so a read racing a write (or a crash between its two files) can
    /// only pair a new body with an old ETag, which the server answers with a 200, not a 304.
    pub fn read(&self, key: &str) -> Option<Entry> {
        let path = self.path(key);
        let etag_path = path.with_extension("etag");
        let etag = fs::read_to_string(&etag_path).ok();
        let body = fs::read(&path).ok()?;
        touch(&path);
        if etag.is_some() {
            touch(&etag_path);
        }
        Some(Entry { etag, body })
    }

    /// Body first, then the ETag (see `read`), under the write lock: two writes of one key never
    /// interleave into one's body beside the other's ETag. Each file lands whole or not at all.
    pub fn write(&self, key: &str, entry: &Entry) -> io::Result<()> {
        let n = self.writes.fetch_add(1, Ordering::Relaxed) + 1;
        let path = self.path(key);
        {
            let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            put(&path, &entry.body, n)?;
            let etag_path = path.with_extension("etag");
            match &entry.etag {
                Some(etag) => put(&etag_path, etag.as_bytes(), n)?,
                None => match fs::remove_file(&etag_path) {
                    Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
                    _ => {}
                },
            }
        }
        if n.is_multiple_of(SWEEP_EVERY) {
            let _ = self.sweep(); // the entry is written; the next sweep retries
        }
        Ok(())
    }

    pub fn clear(&self) -> io::Result<()> {
        for e in fs::read_dir(&self.dir)? {
            fs::remove_file(e?.path())?;
        }
        Ok(())
    }

    /// Deletes the least recently used files until the directory fits in the cap.
    fn sweep(&self) -> io::Result<()> {
        let mut files: Vec<(SystemTime, u64, PathBuf)> = fs::read_dir(&self.dir)?
            .filter_map(|e| {
                let e = e.ok()?;
                let m = e.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort();
        for (_, len, path) in files {
            if total <= self.cap {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total -= len;
            }
        }
        Ok(())
    }
}

/// Writes a uniquely named `.part` beside `path`, then renames it over `path`.
fn put(path: &Path, bytes: &[u8], n: u64) -> io::Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(format!(".{}-{n}.part", std::process::id()));
    fs::write(&part, bytes)?;
    fs::rename(&part, path).inspect_err(|_| {
        let _ = fs::remove_file(&part);
    })
}

fn touch(path: &Path) {
    let _ = fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("tidal-cache").join(name);
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn entry(etag: Option<&str>, body: &[u8]) -> Entry {
        Entry {
            etag: etag.map(Into::into),
            body: body.into(),
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    fn set_mtime(p: &Path, t: SystemTime) {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn mtime(p: &Path) -> SystemTime {
        fs::metadata(p).unwrap().modified().unwrap()
    }

    #[test]
    fn write_then_read_round_trips() {
        let c = DiskCache::open(tmp("round"), 1 << 20, "json").unwrap();
        assert!(c.read("k").is_none());
        let e = entry(Some("\"a\""), b"{\"x\":1}");
        c.write("k", &e).unwrap();
        assert_eq!(c.read("k"), Some(e));
        // A newer copy without an ETag drops the old one.
        let e = entry(None, b"[]");
        c.write("k", &e).unwrap();
        assert_eq!(c.read("k"), Some(e));
    }

    #[test]
    fn racing_writes_keep_body_and_etag_together() {
        let c = DiskCache::open(tmp("race"), 1 << 20, "json").unwrap();
        for round in 0..300 {
            let go = std::sync::Barrier::new(2);
            std::thread::scope(|s| {
                for v in ["a", "b"] {
                    let (c, go) = (&c, &go);
                    s.spawn(move || {
                        go.wait();
                        c.write("k", &entry(Some(v), v.as_bytes())).unwrap();
                    });
                }
            });
            let e = c.read("k").unwrap();
            assert_eq!(e.etag.unwrap().as_bytes(), &e.body[..], "round {round}");
        }
    }

    #[test]
    fn file_name_is_sha256_hex_of_key_with_ext() {
        let dir = tmp("name");
        let c = DiskCache::open(dir.clone(), 1 << 20, "json").unwrap();
        let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"; // sha256("abc")
        assert_eq!(c.path("abc"), dir.join(format!("{hex}.json")));
        c.write("abc", &entry(Some("e"), b"body")).unwrap();
        assert_eq!(names(&dir), [format!("{hex}.etag"), format!("{hex}.json")]);
    }

    #[test]
    fn sweep_deletes_oldest_until_under_cap() {
        let dir = tmp("sweep");
        let c = DiskCache::open(dir.clone(), 25, "json").unwrap();
        let now = SystemTime::now();
        for (i, k) in ["old", "mid", "new"].into_iter().enumerate() {
            c.write(k, &entry(None, &[b'x'; 10])).unwrap();
            set_mtime(&c.path(k), now - Duration::from_secs(300 - 100 * i as u64));
        }
        let c = DiskCache::open(dir, 25, "json").unwrap();
        assert!(c.read("old").is_none());
        assert!(c.read("mid").is_some());
        assert!(c.read("new").is_some());
    }

    #[test]
    fn sweeps_after_every_64th_write() {
        let dir = tmp("sweep64");
        let c = DiskCache::open(dir.clone(), 25, "json").unwrap();
        for i in 0..63 {
            c.write(&i.to_string(), &entry(None, &[b'x'; 10])).unwrap();
        }
        assert_eq!(names(&dir).len(), 63);
        c.write("63", &entry(None, &[b'x'; 10])).unwrap();
        assert_eq!(names(&dir).len(), 2);
    }

    #[test]
    fn read_refreshes_mtime() {
        let c = DiskCache::open(tmp("touch"), 1 << 20, "json").unwrap();
        c.write("k", &entry(None, b"v")).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        set_mtime(&c.path("k"), old);
        c.read("k").unwrap();
        assert!(mtime(&c.path("k")) > old);
    }

    #[test]
    fn partial_write_is_never_read() {
        let dir = tmp("part");
        let c = DiskCache::open(dir.clone(), 1 << 20, "json").unwrap();
        let mut stray = c.path("k").into_os_string();
        stray.push(".1-1.part");
        fs::write(&stray, b"half").unwrap();
        assert!(c.read("k").is_none());
        c.write("j", &entry(Some("e"), b"whole")).unwrap();
        let parts = names(&dir).into_iter().filter(|n| n.ends_with(".part"));
        assert_eq!(parts.count(), 1, "only the stray one");
    }

    #[test]
    fn clear_empties_dir() {
        let dir = tmp("clear");
        let c = DiskCache::open(dir.clone(), 1 << 20, "json").unwrap();
        c.write("a", &entry(Some("e"), b"1")).unwrap();
        c.write("b", &entry(None, b"2")).unwrap();
        c.clear().unwrap();
        assert!(names(&dir).is_empty());
        assert!(c.read("a").is_none());
    }
}
