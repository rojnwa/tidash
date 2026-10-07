//! Forward-only byte stream -> seekable, temp-file-backed `MediaSource`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use symphonia::core::io::MediaSource;

const CHUNK: usize = 64 * 1024;

#[derive(Default)]
struct State {
    downloaded: u64,
    done: bool,
    stop: bool,
    error: Option<(io::ErrorKind, String)>,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

fn lock(s: &Shared) -> MutexGuard<'_, State> {
    s.0.lock().unwrap_or_else(|e| e.into_inner())
}

// Positional I/O: the writer and reader handles share one OS file cursor, so neither may use it.
#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, off)
}
#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, off)
}
#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(f, buf, off)
}
#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(f, buf, off)
}

fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = write_at(f, buf, off)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

fn copy_loop(mut src: Box<dyn Read + Send>, file: &File, shared: &Shared) -> io::Result<()> {
    let mut buf = vec![0u8; CHUNK];
    let mut off = 0u64;
    loop {
        if lock(shared).stop {
            return Ok(());
        }
        let n = src.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        // Positional writes hit the OS file directly (no userspace buffer), so the
        // reader's cloned handle sees the bytes as soon as `downloaded` is published.
        write_all_at(file, &buf[..n], off)?;
        off += n as u64;
        lock(shared).downloaded = off;
        shared.1.notify_all();
    }
}

/// A seekable view over a stream that is still downloading into an anonymous temp file.
///
/// Reads block (on a `Condvar`, never spinning) until the bytes exist, the download ended
/// (short read / 0 at EOF) or it failed (the stored error is returned with the same kind and
/// message). `seek` never blocks, except `SeekFrom::End` with an unknown length, which waits
/// for the download to finish.
// ponytail: sequential fetch, seek-ahead waits; add HTTP Range requests if slow links complain.
pub struct ProgressiveFile {
    reader: File,
    pos: u64,
    len: Option<u64>,
    shared: Shared,
}

impl ProgressiveFile {
    /// Start copying `reader` into a temp file on a background thread.
    pub fn spawn(reader: Box<dyn Read + Send>, len: Option<u64>) -> io::Result<Self> {
        let writer = tempfile::tempfile()?;
        let read_handle = writer.try_clone()?;
        let shared: Shared = Arc::default();
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("progressive-file".into())
            .spawn(move || {
                let res = copy_loop(reader, &writer, &thread_shared);
                let mut st = lock(&thread_shared);
                match res {
                    Ok(()) => st.done = true,
                    Err(e) => st.error = Some((e.kind(), e.to_string())),
                }
                drop(st);
                thread_shared.1.notify_all();
            })?;
        Ok(Self {
            reader: read_handle,
            pos: 0,
            len,
            shared,
        })
    }

    /// Wait until `ready(state)`, or the download ended / failed.
    fn wait(&self, ready: impl Fn(&State) -> bool) -> MutexGuard<'_, State> {
        self.shared
            .1
            .wait_while(lock(&self.shared), |s| {
                !ready(s) && !s.done && s.error.is_none()
            })
            .unwrap_or_else(|e| e.into_inner())
    }
}

impl Read for ProgressiveFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let pos = self.pos;
        let avail = {
            let st = self.wait(|s| s.downloaded > pos);
            if st.downloaded <= pos {
                return match &st.error {
                    Some((kind, msg)) => Err(io::Error::new(*kind, msg.clone())),
                    None => Ok(0),
                };
            }
            st.downloaded - pos
        };
        let n = buf.len().min(usize::try_from(avail).unwrap_or(usize::MAX));
        let n = read_at(&self.reader, &mut buf[..n], pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ProgressiveFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => {
                let len = match self.len {
                    Some(l) => l,
                    None => {
                        let st = self.wait(|_| false);
                        if let Some((kind, msg)) = &st.error {
                            return Err(io::Error::new(*kind, msg.clone()));
                        }
                        st.downloaded
                    }
                };
                len.checked_add_signed(d)
            }
        };
        self.pos = target
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        Ok(self.pos)
    }
}

impl MediaSource for ProgressiveFile {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.len.or_else(|| {
            let st = lock(&self.shared);
            (st.done && st.error.is_none()).then_some(st.downloaded)
        })
    }
}

impl Drop for ProgressiveFile {
    /// Flags the writer to stop at its next chunk boundary. Not joined: it may be blocked
    /// inside a slow/endless `read()`; it exits (closing the temp file) once that returns.
    fn drop(&mut self) {
        lock(&self.shared).stop = true;
        self.shared.1.notify_all();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use symphonia::core::io::MediaSource;

    /// Yields `data` in `chunk`-byte pieces with a short sleep; errors once `fail_at` bytes were served.
    struct SlowReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
        fail_at: Option<usize>,
    }

    impl Read for SlowReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(2));
            if self.fail_at.is_some_and(|f| self.pos >= f) {
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, "boom"));
            }
            let end = (self.pos + self.chunk.min(buf.len())).min(self.data.len());
            let n = end - self.pos;
            buf[..n].copy_from_slice(&self.data[self.pos..end]);
            self.pos = end;
            Ok(n)
        }
    }

    fn pseudo_random(n: usize) -> Vec<u8> {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    fn slow(data: Vec<u8>, chunk: usize, fail_at: Option<usize>) -> Box<SlowReader> {
        Box::new(SlowReader {
            data,
            pos: 0,
            chunk,
            fail_at,
        })
    }

    #[test]
    fn reads_everything_in_order() {
        let src = pseudo_random(1 << 20);
        let mut f = ProgressiveFile::spawn(slow(src.clone(), 50_000, None), None).unwrap();
        assert_eq!(f.byte_len(), None);
        let mut out = Vec::new();
        f.read_to_end(&mut out).unwrap();
        assert_eq!(out, src);
        assert_eq!(f.byte_len(), Some(src.len() as u64));
        assert!(f.is_seekable());
    }

    #[test]
    fn seek_ahead_waits_then_reads() {
        let src = pseudo_random(1 << 20);
        let mut f = ProgressiveFile::spawn(slow(src.clone(), 65_536, None), Some(src.len() as u64))
            .unwrap();
        assert_eq!(f.seek(SeekFrom::Start(900_000)).unwrap(), 900_000);
        let mut buf = [0u8; 1000];
        f.read_exact(&mut buf).unwrap();
        assert_eq!(buf[..], src[900_000..901_000]);
        // End-relative seek uses the known length; seeking back works.
        assert_eq!(f.seek(SeekFrom::End(-10)).unwrap(), src.len() as u64 - 10);
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_exact(&mut buf).unwrap();
        assert_eq!(buf[..], src[..1000]);
    }

    #[test]
    fn seek_end_waits_for_unknown_length() {
        let src = pseudo_random(200_000);
        let mut f = ProgressiveFile::spawn(slow(src, 65_536, None), None).unwrap();
        assert_eq!(f.seek(SeekFrom::End(0)).unwrap(), 200_000);
    }

    #[test]
    fn error_mid_stream_surfaces() {
        let src = pseudo_random(1 << 20);
        let mut f = ProgressiveFile::spawn(slow(src.clone(), 10_000, Some(100_000)), None).unwrap();
        let mut got = vec![0u8; 100_000];
        f.read_exact(&mut got).unwrap();
        assert_eq!(got, src[..100_000]);
        let e = f.read(&mut [0u8; 16]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        assert!(e.to_string().contains("boom"));
    }

    struct Endless(Arc<AtomicBool>);

    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(5));
            buf[..10].fill(7);
            Ok(10)
        }
    }
    impl Drop for Endless {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn drop_stops_writer() {
        let exited = Arc::new(AtomicBool::new(false));
        let f = ProgressiveFile::spawn(Box::new(Endless(exited.clone())), None).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let t = Instant::now();
        drop(f);
        assert!(t.elapsed() < Duration::from_secs(1));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !exited.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(exited.load(Ordering::SeqCst), "writer thread did not exit");
    }
}
