//! Workload-side client library for the Calorimeter `CALO_CHANNEL` text protocol (spec §9.4).

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

const LINE_BUF_CAP: usize = 4096;

thread_local! {
    static THREAD_STATE: RefCell<ThreadScratch> = RefCell::new(ThreadScratch::new());
}

struct ThreadScratch {
    tid: u64,
    buf: [u8; LINE_BUF_CAP],
}

impl ThreadScratch {
    fn new() -> Self {
        Self {
            tid: current_os_tid(),
            buf: [0u8; LINE_BUF_CAP],
        }
    }
}

#[cfg(target_os = "linux")]
fn current_os_tid() -> u64 {
    // SAFETY: `SYS_gettid` is a read-only Linux syscall taking no pointers and returning the caller's thread ID.
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    if tid > 0 {
        tid as u64
    } else {
        1
    }
}

#[cfg(not(target_os = "linux"))]
fn current_os_tid() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TID: AtomicU64 = AtomicU64::new(1);
    NEXT_TID.fetch_add(1, Ordering::Relaxed)
}

/// Handle to the `CALO_CHANNEL` FIFO/file; becomes a zero-cost no-op when `CALO_CHANNEL` is unset (spec §9.4).
#[derive(Clone, Default)]
pub struct Channel {
    file: Option<Arc<File>>,
}

impl Channel {
    /// Opens the channel from the `CALO_CHANNEL` environment variable, or returns a no-op channel if unset (spec §9.4).
    pub fn from_env() -> Self {
        match std::env::var_os("CALO_CHANNEL") {
            Some(path) if !path.is_empty() => Self::from_path(path).unwrap_or_default(),
            _ => Self::default(),
        }
    }

    /// Opens a `Channel` writing directly to `path` (e.g. a FIFO or pipe).
    pub fn from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let f = OpenOptions::new().write(true).open(path)?;
        Ok(Self {
            file: Some(Arc::new(f)),
        })
    }

    /// Wraps an already-opened `File` (such as the write end of an OS pipe in tests).
    pub fn from_file(file: File) -> Self {
        Self {
            file: Some(Arc::new(file)),
        }
    }

    /// Returns `true` if this channel is a no-op (disabled).
    pub fn is_noop(&self) -> bool {
        self.file.is_none()
    }

    /// Starts a named workload phase, returning an RAII `PhaseGuard` that emits `end` on drop (spec §9.4).
    pub fn phase<'a>(&'a self, name: &str) -> PhaseGuard<'a> {
        if self.file.is_some() {
            self.write_phase(name, "start");
        }
        PhaseGuard {
            channel: self,
            name: name.to_string(),
        }
    }

    /// Enters a nestable per-thread region, returning an RAII `RegionGuard` that emits `end` on drop (spec §9.4).
    pub fn region<'a>(&'a self, name: &str) -> RegionGuard<'a> {
        if self.file.is_some() {
            self.write_region(name, "start");
        }
        RegionGuard {
            channel: self,
            name: name.to_string(),
        }
    }

    /// Emits a numeric workload metric with optional unit (`metric <key>=<value> [unit]\n`, spec §9.4).
    pub fn metric(&self, key: &str, value: f64, unit: Option<&str>) {
        let Some(file) = &self.file else {
            return;
        };
        THREAD_STATE.with(|cell| {
            let mut scratch = cell.borrow_mut();
            let mut cur = StackCursor::new(&mut scratch.buf);
            cur.push_bytes(b"metric ");
            cur.push_sanitized(key);
            cur.push_bytes(b"=");
            let _ = write!(&mut cur, "{value}");
            if let Some(u) = unit {
                if !u.is_empty() {
                    cur.push_bytes(b" ");
                    cur.push_sanitized(u);
                }
            }
            cur.push_bytes(b"\n");
            let len = cur.len();
            let _ = (&**file).write(&scratch.buf[..len]);
        });
    }

    /// Emits a string tag (`tag <key>=<value>\n`, spec §9.4).
    pub fn tag(&self, key: &str, value: &str) {
        let Some(file) = &self.file else {
            return;
        };
        THREAD_STATE.with(|cell| {
            let mut scratch = cell.borrow_mut();
            let mut cur = StackCursor::new(&mut scratch.buf);
            cur.push_bytes(b"tag ");
            cur.push_sanitized(key);
            cur.push_bytes(b"=");
            cur.push_sanitized(value);
            cur.push_bytes(b"\n");
            let len = cur.len();
            let _ = (&**file).write(&scratch.buf[..len]);
        });
    }

    fn write_phase(&self, name: &str, kind: &str) {
        let Some(file) = &self.file else {
            return;
        };
        THREAD_STATE.with(|cell| {
            let mut scratch = cell.borrow_mut();
            let mut cur = StackCursor::new(&mut scratch.buf);
            cur.push_bytes(b"phase ");
            cur.push_sanitized(name);
            cur.push_bytes(b" ");
            cur.push_bytes(kind.as_bytes());
            cur.push_bytes(b"\n");
            let len = cur.len();
            let _ = (&**file).write(&scratch.buf[..len]);
        });
    }

    fn write_region(&self, name: &str, kind: &str) {
        let Some(file) = &self.file else {
            return;
        };
        THREAD_STATE.with(|cell| {
            let mut scratch = cell.borrow_mut();
            let tid = scratch.tid;
            let mut cur = StackCursor::new(&mut scratch.buf);
            let _ = write!(&mut cur, "tid={tid} region ");
            cur.push_sanitized(name);
            cur.push_bytes(b" ");
            cur.push_bytes(kind.as_bytes());
            cur.push_bytes(b"\n");
            let len = cur.len();
            let _ = (&**file).write(&scratch.buf[..len]);
        });
    }
}

/// RAII guard for a `phase <name> start|end` span (spec §9.4).
pub struct PhaseGuard<'a> {
    channel: &'a Channel,
    name: String,
}

impl Drop for PhaseGuard<'_> {
    fn drop(&mut self) {
        if self.channel.file.is_some() {
            self.channel.write_phase(&self.name, "end");
        }
    }
}

/// RAII guard for a nestable `tid=<n> region <name> start|end` span (spec §9.4).
pub struct RegionGuard<'a> {
    channel: &'a Channel,
    name: String,
}

impl Drop for RegionGuard<'_> {
    fn drop(&mut self) {
        if self.channel.file.is_some() {
            self.channel.write_region(&self.name, "end");
        }
    }
}

struct StackCursor<'a> {
    buf: &'a mut [u8; LINE_BUF_CAP],
    pos: usize,
}

impl<'a> StackCursor<'a> {
    fn new(buf: &'a mut [u8; LINE_BUF_CAP]) -> Self {
        Self { buf, pos: 0 }
    }

    const fn len(&self) -> usize {
        self.pos
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        let avail = self.buf.len().saturating_sub(self.pos);
        let n = bytes.len().min(avail);
        self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
        self.pos += n;
    }

    fn push_sanitized(&mut self, s: &str) {
        for &b in s.as_bytes() {
            if self.pos >= self.buf.len().saturating_sub(1) {
                break;
            }
            let clean = if b.is_ascii_whitespace() || b.is_ascii_control() {
                b'_'
            } else {
                b
            };
            self.buf[self.pos] = clean;
            self.pos += 1;
        }
    }
}

impl Write for StackCursor<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.push_bytes(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
