// I8: unsafe is confined to this file and perf.rs.
use super::record::{decode, Event, SAMPLE_TYPE};
use perf_event_open_sys as sys;
use sys::bindings::*;
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, thiserror::Error)]
pub enum RingError {
    #[error("ring corrupt: record size {size} with {avail} bytes available")]
    Corrupt { size: usize, avail: u64 },
}

/// Which clock drives sampling. Cycles needs a PMU; CpuClock works in VMs.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    Cycles,
    CpuClock,
}

/// Mapped perf buffer: 1 metadata page + 2^n data pages.
pub struct Ring {
    base: *mut u8,
    map_len: usize,
    page: usize,
    data_size: usize,
    owns_map: bool,
    scratch: Vec<u8>,
}

// SAFETY: the mapping is only touched through &mut self.
unsafe impl Send for Ring {}

/// Copy `dst.len()` bytes from the circular data area at stream position `pos`.
///
/// # Safety
/// `data` must point to `mask + 1` readable bytes; `dst.len() <= mask + 1`.
unsafe fn copy_out(data: *const u8, mask: u64, pos: u64, dst: &mut [u8]) {
    let size = mask as usize + 1;
    let off = (pos & mask) as usize;
    let first = dst.len().min(size - off);
    ptr::copy_nonoverlapping(data.add(off), dst.as_mut_ptr(), first);
    ptr::copy_nonoverlapping(data, dst.as_mut_ptr().add(first), dst.len() - first);
}

impl Ring {
    /// # Safety
    /// `base` must point to a region of `page + data_size` bytes, 8-aligned,
    /// laid out like a perf mmap (metadata page then data), valid for the
    /// lifetime of the Ring. `data_size` must be a power of two.
    pub unsafe fn from_raw_parts(base: *mut u8, page: usize, data_size: usize, owns_map: bool) -> Self {
        Ring { base, map_len: page + data_size, page, data_size, owns_map, scratch: Vec::new() }
    }

    fn meta(&self) -> *mut perf_event_mmap_page {
        self.base.cast()
    }

    /// Drain every complete record, calling `f` with each one (header included).
    /// Returns the number of records. Advances data_tail with Release ordering
    /// so the kernel may reuse the space only after we finished reading it.
    pub fn drain(&mut self, mut f: impl FnMut(&[u8])) -> Result<usize, RingError> {
        // SAFETY: base points to a live perf mmap (constructor contract).
        let (head_a, tail_a) = unsafe {
            (
                AtomicU64::from_ptr(ptr::addr_of_mut!((*self.meta()).data_head)),
                AtomicU64::from_ptr(ptr::addr_of_mut!((*self.meta()).data_tail)),
            )
        };
        let head = head_a.load(Ordering::Acquire);
        let mut tail = tail_a.load(Ordering::Relaxed);
        let mask = self.data_size as u64 - 1;
        // SAFETY: data area starts one page after base.
        let data = unsafe { self.base.add(self.page) } as *const u8;
        let mut n = 0;
        while head.wrapping_sub(tail) >= 8 {
            let mut hdr = [0u8; 8];
            // SAFETY: data/mask describe the data area; 8 <= data_size.
            unsafe { copy_out(data, mask, tail, &mut hdr) };
            let size = u16::from_ne_bytes([hdr[6], hdr[7]]) as usize;
            let avail = head - tail;
            if size < 8 || size as u64 > avail || size > self.data_size {
                tail_a.store(head, Ordering::Release); // resync, drop the rest
                return Err(RingError::Corrupt { size, avail });
            }
            self.scratch.resize(size, 0);
            // SAFETY: as above, size <= data_size.
            unsafe { copy_out(data, mask, tail, &mut self.scratch) };
            f(&self.scratch);
            tail += size as u64;
            n += 1;
        }
        tail_a.store(tail, Ordering::Release);
        Ok(n)
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        if self.owns_map {
            // SAFETY: base/map_len came from a successful mmap.
            unsafe { libc::munmap(self.base.cast(), self.map_len) };
        }
    }
}

/// A sampling event on a child plus its mapped buffer (spec §9.3 B7-B11).
pub struct Sampler {
    fd: OwnedFd,
    ring: Ring,
}

impl Sampler {
    /// Open a sampling event on `pid`. Disabled until the child execs.
    /// `pages_log2` = log2 of data pages (6 -> 256 KiB with 4 KiB pages).
    pub fn open(pid: libc::pid_t, source: Source, freq_hz: u64, pages_log2: u32) -> io::Result<Self> {
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let data_size = page << pages_log2;

        let mut attr = perf_event_attr::default();
        attr.size = std::mem::size_of::<perf_event_attr>() as u32;
        match source {
            Source::Cycles => {
                attr.type_ = PERF_TYPE_HARDWARE;
                attr.config = PERF_COUNT_HW_CPU_CYCLES as u64;
            }
            Source::CpuClock => {
                attr.type_ = PERF_TYPE_SOFTWARE;
                attr.config = PERF_COUNT_SW_CPU_CLOCK as u64;
            }
        }
        attr.__bindgen_anon_1.sample_freq = freq_hz;
        attr.set_freq(1);
        attr.sample_type = SAMPLE_TYPE;
        attr.set_disabled(1);
        attr.set_enable_on_exec(1);
        attr.set_inherit(1);
        attr.set_exclude_kernel(1);
        attr.set_exclude_hv(1);
        attr.set_mmap2(1);
        attr.set_comm(1);
        attr.set_task(1);
        attr.set_sample_id_all(1);
        attr.set_use_clockid(1);
        attr.clockid = libc::CLOCK_MONOTONIC_RAW; // I6: same clock as everything else
        attr.set_watermark(1);
        attr.__bindgen_anon_2.wakeup_watermark = (data_size / 2) as u32;

        // SAFETY: attr fully initialised; kernel copies it.
        let raw = unsafe { sys::perf_event_open(&mut attr, pid, -1, -1, PERF_FLAG_FD_CLOEXEC as u64) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: raw is a fresh valid fd.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        let map_len = page + data_size;
        // SAFETY: mapping a perf fd; checked against MAP_FAILED below.
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE, // write needed for data_tail
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh mmap of page + data_size bytes (power-of-two data).
        let ring = unsafe { Ring::from_raw_parts(base.cast(), page, data_size, true) };
        Ok(Sampler { fd, ring })
    }

    /// For the epoll loop (spec §9.2 B-O1).
    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Decode and deliver every pending event. Decode failures are counted,
    /// not fatal: returns (events, undecodable).
    pub fn drain(&mut self, mut f: impl FnMut(Event)) -> Result<(usize, usize), RingError> {
        let mut bad = 0;
        let n = self.ring.drain(|rec| match decode(rec) {
            Ok(ev) => f(ev),
            Err(_) => bad += 1,
        })?;
        Ok((n, bad))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap-around: a 24-byte LOST record straddling the end of the data area.
    #[test]
    fn drains_record_that_wraps() {
        let page = 4096usize;
        let data = 4096usize;
        let mut mem = vec![0u64; (page + data) / 8];
        let base = mem.as_mut_ptr() as *mut u8;

        let start: u64 = data as u64 - 12; // 12 bytes before the end
        let mut rec = Vec::new();
        rec.extend_from_slice(&2u32.to_ne_bytes()); // LOST
        rec.extend_from_slice(&0u16.to_ne_bytes());
        rec.extend_from_slice(&24u16.to_ne_bytes());
        rec.extend_from_slice(&0u64.to_ne_bytes());
        rec.extend_from_slice(&7u64.to_ne_bytes()); // lost = 7
        for (i, b) in rec.iter().enumerate() {
            let off = ((start + i as u64) % data as u64) as usize;
            // SAFETY: off < data, inside our allocation.
            unsafe { *base.add(page + off) = *b };
        }
        // SAFETY: mem is 8-aligned, page+data bytes, data is a power of two.
        let mut ring = unsafe { Ring::from_raw_parts(base, page, data, false) };
        let meta = base as *mut perf_event_mmap_page;
        unsafe {
            (*meta).data_tail = start;
            (*meta).data_head = start + 24;
        }
        let mut got = Vec::new();
        let n = ring.drain(|r| got.push(r.to_vec())).unwrap();
        assert_eq!(n, 1);
        assert_eq!(got[0], rec);
        assert_eq!(decode(&got[0]).unwrap(), Event::Lost { count: 7 });
        assert_eq!(unsafe { (*meta).data_tail }, start + 24);
    }
}
