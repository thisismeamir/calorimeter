// I8: the only unsafe in this unit lives here.
use perf_event_open_sys as sys;
use std::{
    io,
    os::fd::{FromRawFd, OwnedFd},
};
use sys::bindings::*;

/// One hardware counter, opened on a not-yet-exec'd child (spec §9.1 step 3).
pub struct Counter {
    fd: OwnedFd,
}

/// Raw read: value plus enabled/running times, so multiplexing
/// scaling can be redone later (R-REC-5).
#[derive(Debug, Clone, Copy)]
pub struct RawCount {
    pub value: u64,
    pub time_enabled_ns: u64,
    pub time_running_ns: u64,
}

impl Counter {
/// Open an event with explicit `perf_event_attr.type_`, `config`, and `exclude_kernel`
    /// for `pid`, any CPU. Starts disabled and enables on exec.
    #[allow(clippy::field_reassign_with_default)]
    pub fn open_with_type(
        type_: u32,
        config: u64,
        pid: libc::pid_t,
        exclude_kernel: bool,
    ) -> io::Result<Self> {
        let mut attr = perf_event_attr::default();
        attr.type_ = type_;
        attr.size = std::mem::size_of::<perf_event_attr>() as u32;
        attr.config = config;
        attr.read_format = (PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING) as u64;
        attr.set_disabled(1);
        attr.set_enable_on_exec(1);
        attr.set_inherit(1);        // cover child threads
        attr.set_exclude_kernel(u64::from(exclude_kernel));
        attr.set_exclude_hv(1);

        // SAFETY: attr is fully initialised and outlives the call; the
        // kernel copies it. Returns a new fd or -1 with errno set.
        let fd = unsafe { sys::perf_event_open(&mut attr, pid, -1, -1, PERF_FLAG_FD_CLOEXEC as u64) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is a valid, owned descriptor we just created.
        Ok(Self { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
    }

    /// Open `config` (e.g. PERF_COUNT_HW_CPU_CYCLES) for `pid`, any CPU.
    /// Starts disabled and enables on exec, so pre-exec setup is not counted.
    pub fn open_hw(config: u64, pid: libc::pid_t) -> io::Result<Self> {
        Self::open_with_type(PERF_TYPE_HARDWARE, config, pid, false)
    }

    pub fn read(&self) -> io::Result<RawCount> {
        let mut buf = [0u64; 3]; // value, time_enabled, time_running
                                 // SAFETY: buf is valid for 24 bytes; fd is open.
        let n = unsafe {
            libc::read(
                std::os::fd::AsRawFd::as_raw_fd(&self.fd),
                buf.as_mut_ptr().cast(),
                24,
            )
        };

        if n < 0 {
            return Err(io::Error::last_os_error());
        }

        if n != 24 {
            return Err(io::Error::last_os_error());
        }
        Ok(RawCount {
            value: buf[0],
            time_enabled_ns: buf[1],
            time_running_ns: buf[2],
        })
    }
}
