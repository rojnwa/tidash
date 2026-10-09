//! Forward-only byte stream -> seekable, temp-file-backed `MediaSource`.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use symphonia::core::io::MediaSource;

const CHUNK: usize = 64 * 1024;

#[derive(Default)]
struct State {
    downloaded: u64,
    done: bool,
    stop: bool,
    error: Option<(io::ErrorKind, String)>,
}

/// A download's state and the condvar its readers wait on.
type Progress = (Mutex<State>, Condvar);
type Shared = Arc<Progress>;

/// Payload of the error a waiting read returns while an [`Interrupt`] is raised. Not
/// `ErrorKind::Interrupted`: std's and symphonia's `read_exact` retry that kind, so the read
/// would block again inside the decoder.
#[derive(Debug)]
struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("read interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Whether `e` is a read given up because of an [`Interrupt`].
pub(crate) fn is_interrupt(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|e| e.is::<Interrupted>())
}

/// Makes reads that wait for the network give up, so the reading thread can see to something
/// else. One per reading thread, shared by every [`ProgressiveFile`] spawned with it, so it
/// also reaches a file that does not exist yet. Cheap to clone; usable from any thread.
#[derive(Clone, Default)]
pub struct Interrupt(Arc<Raised>);

#[derive(Default)]
struct Raised {
    /// `interrupt`s not yet `handled`; while any are, waiting reads fail.
    pending: AtomicU32,
    /// The files spawned with this interrupt (dead ones pruned at the next spawn): a waiting
    /// read can be blocked on any of them.
    file: Mutex<Vec<Weak<Progress>>>,
}

impl Interrupt {
    /// From now until the matching [`handled`](Self::handled), a read that has to wait for
    /// bytes (including one waiting already) fails with an interrupt error instead.
    pub fn interrupt(&self) {
        self.0.pending.fetch_add(1, SeqCst);
        let files = self.0.file.lock().unwrap_or_else(PoisonError::into_inner);
        for file in files.iter().filter_map(Weak::upgrade) {
            // Taking the lock orders this after a reader's check of `pending`: no lost wakeup.
            drop(lock(&file));
            file.1.notify_all();
        }
    }

    /// The reason for one `interrupt` has been dealt with; once all are, reads wait again.
    pub fn handled(&self) {
        let _ = self
            .0
            .pending
            .try_update(SeqCst, SeqCst, |n| n.checked_sub(1));
    }

    pub(crate) fn raised(&self) -> bool {
        self.0.pending.load(SeqCst) > 0
    }
}

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
        let n = match src.read(&mut buf) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            r => r?,
        };
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

/// Held by the writer thread. Wakes the readers when it ends; if it ends without finishing or
/// failing (it panicked), fails them, so no read waits forever.
struct WriterEnded(Shared);

impl Drop for WriterEnded {
    fn drop(&mut self) {
        let mut st = lock(&self.0);
        if !st.done && st.error.is_none() {
            st.error = Some((io::ErrorKind::Other, "download thread panicked".into()));
        }
        drop(st);
        self.0.1.notify_all();
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
    interrupt: Interrupt,
    /// The original, not a `forward` view: dropping it stops the download.
    owner: bool,
}

impl ProgressiveFile {
    /// Start copying `reader` into a temp file on a background thread. Waiting reads give up
    /// while `interrupt` is raised.
    pub fn spawn(
        reader: Box<dyn Read + Send>,
        len: Option<u64>,
        interrupt: &Interrupt,
    ) -> io::Result<Self> {
        let writer = tempfile::tempfile()?;
        let read_handle = writer.try_clone()?;
        let shared: Shared = Arc::default();
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("progressive-file".into())
            .spawn(move || {
                let ended = WriterEnded(thread_shared);
                let res = copy_loop(reader, &writer, &ended.0);
                let mut st = lock(&ended.0);
                match res {
                    Ok(()) => st.done = true,
                    Err(e) => st.error = Some((e.kind(), e.to_string())),
                }
            })?;
        let mut files = interrupt
            .0
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        files.retain(|f| f.strong_count() > 0);
        files.push(Arc::downgrade(&shared));
        Ok(Self {
            reader: read_handle,
            pos: 0,
            len,
            shared,
            interrupt: interrupt.clone(),
            owner: true,
        })
    }

    /// A forward-only view from the first byte (see [`Forward`]); the download stays with `self`.
    pub fn forward(&self) -> io::Result<Forward> {
        Ok(Forward(ProgressiveFile {
            reader: self.reader.try_clone()?,
            pos: 0,
            len: self.len,
            shared: Arc::clone(&self.shared),
            interrupt: self.interrupt.clone(),
            owner: false,
        }))
    }

    /// Wait until `ready(state)`, the download ended / failed, or an interrupt is raised.
    fn wait(&self, ready: impl Fn(&State) -> bool) -> MutexGuard<'_, State> {
        self.shared
            .1
            .wait_while(lock(&self.shared), |s| {
                !ready(s) && !s.done && s.error.is_none() && !self.interrupt.raised()
            })
            .unwrap_or_else(|e| e.into_inner())
    }
}

/// A forward-only view of a [`ProgressiveFile`]. symphonia streams it (seeking forward by
/// skipping) instead of first seeking to the end, which waits for the whole download when the
/// length is unknown (DASH).
pub struct Forward(ProgressiveFile);

impl Read for Forward {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for Forward {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.0.seek(from)
    }
}

impl MediaSource for Forward {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Why a wait that did not get its bytes ended: the download's error, or an interrupt.
fn not_ready(st: &State) -> io::Error {
    match &st.error {
        Some((kind, msg)) => io::Error::new(*kind, msg.clone()),
        None => io::Error::other(Interrupted),
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
                return if st.done && st.error.is_none() {
                    Ok(0)
                } else {
                    Err(not_ready(&st))
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
                        if !st.done || st.error.is_some() {
                            return Err(not_ready(&st));
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
        if self.owner {
            lock(&self.shared).stop = true;
            self.shared.1.notify_all();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use symphonia::core::io::MediaSource;

    fn spawn(reader: Box<dyn Read + Send>, len: Option<u64>) -> io::Result<ProgressiveFile> {
        ProgressiveFile::spawn(reader, len, &Interrupt::default())
    }

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
        let mut f = spawn(slow(src.clone(), 50_000, None), None).unwrap();
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
        let mut f = spawn(slow(src.clone(), 65_536, None), Some(src.len() as u64)).unwrap();
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
        let mut f = spawn(slow(src, 65_536, None), None).unwrap();
        assert_eq!(f.seek(SeekFrom::End(0)).unwrap(), 200_000);
    }

    #[test]
    fn error_mid_stream_surfaces() {
        let src = pseudo_random(1 << 20);
        let mut f = spawn(slow(src.clone(), 10_000, Some(100_000)), None).unwrap();
        let mut got = vec![0u8; 100_000];
        f.read_exact(&mut got).unwrap();
        assert_eq!(got, src[..100_000]);
        let e = f.read(&mut [0u8; 16]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        assert!(e.to_string().contains("boom"));
    }

    /// Blocks until the test lets it serve 4 bytes; ends when the test drops the sender.
    struct Gate(std::sync::mpsc::Receiver<()>);

    impl Read for Gate {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.recv().is_err() {
                return Ok(0);
            }
            buf[..4].fill(9);
            Ok(4)
        }
    }

    /// Runs `f` on a thread. Wait on the result with `recv_timeout`, so a read that never
    /// returns fails the test instead of hanging it.
    fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx
    }

    fn read_on_thread(
        mut f: ProgressiveFile,
    ) -> mpsc::Receiver<(ProgressiveFile, io::Result<usize>, Instant)> {
        on_thread(move || {
            let r = f.read(&mut [0u8; 16]);
            (f, r, Instant::now())
        })
    }

    fn read_to_end_on_thread(mut f: ProgressiveFile) -> mpsc::Receiver<io::Result<Vec<u8>>> {
        on_thread(move || {
            let mut out = Vec::new();
            f.read_to_end(&mut out).map(|_| out)
        })
    }

    const STUCK: Duration = Duration::from_secs(2);

    #[test]
    fn interrupt_unblocks_a_waiting_read_until_handled() {
        let (go, gate) = mpsc::channel();
        let interrupt = Interrupt::default();
        let f = ProgressiveFile::spawn(Box::new(Gate(gate)), None, &interrupt).unwrap();
        let waiting = read_on_thread(f);
        std::thread::sleep(Duration::from_millis(50)); // let it block
        let at = Instant::now();
        interrupt.interrupt();
        let (f, r, done) = waiting.recv_timeout(STUCK).unwrap();
        assert!(is_interrupt(&r.unwrap_err()));
        assert!(done - at < Duration::from_millis(100), "{:?}", done - at);
        // Handled: reads wait for bytes again rather than failing.
        interrupt.handled();
        let waiting = read_on_thread(f);
        std::thread::sleep(Duration::from_millis(50));
        go.send(()).unwrap();
        let (_, r, _) = waiting.recv_timeout(STUCK).unwrap();
        assert_eq!(r.unwrap(), 4);
    }

    #[test]
    fn interrupt_wakes_a_reader_on_an_older_file() {
        let interrupt = Interrupt::default();
        let (_go_a, gate_a) = mpsc::channel();
        let (_go_b, gate_b) = mpsc::channel();
        let a = ProgressiveFile::spawn(Box::new(Gate(gate_a)), None, &interrupt).unwrap();
        let _b = ProgressiveFile::spawn(Box::new(Gate(gate_b)), None, &interrupt).unwrap();
        let waiting = read_on_thread(a);
        std::thread::sleep(Duration::from_millis(50)); // let it block
        let at = Instant::now();
        interrupt.interrupt();
        let (_, r, done) = waiting.recv_timeout(STUCK).unwrap();
        assert!(is_interrupt(&r.unwrap_err()));
        assert!(done - at < Duration::from_millis(100), "{:?}", done - at);
    }

    /// Panics on every read.
    struct Panics;

    impl Read for Panics {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("boom")
        }
    }

    #[test]
    fn a_panicking_source_fails_reads() {
        let f = spawn(Box::new(io::repeat(1).take(10).chain(Panics)), None).unwrap();
        let t = Instant::now();
        let e = read_to_end_on_thread(f)
            .recv_timeout(STUCK)
            .unwrap()
            .unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        assert_eq!(e.kind(), io::ErrorKind::Other);
        assert!(e.to_string().contains("panicked"), "{e}");
    }

    /// Fails its first read with `ErrorKind::Interrupted`, then is at EOF.
    struct EintrOnce(bool);

    impl Read for EintrOnce {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            if std::mem::replace(&mut self.0, true) {
                Ok(0)
            } else {
                Err(io::ErrorKind::Interrupted.into())
            }
        }
    }

    #[test]
    fn eintr_is_retried() {
        let src = pseudo_random(200_000);
        let reader = io::Cursor::new(src[..100_000].to_vec())
            .chain(EintrOnce(false))
            .chain(io::Cursor::new(src[100_000..].to_vec()));
        let f = spawn(Box::new(reader), None).unwrap();
        let out = read_to_end_on_thread(f)
            .recv_timeout(STUCK)
            .unwrap()
            .unwrap();
        assert_eq!(out, src);
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
        let f = spawn(Box::new(Endless(exited.clone())), None).unwrap();
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
