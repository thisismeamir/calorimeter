//! Safe wrappers over Linux `epoll`, `timerfd`, `signalfd`, and non-blocking `waitid`
//! for the single-threaded observe loop (Spec §9.2 B-O1).

use std::{
    io,
    mem::MaybeUninit,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    ptr,
};

/// Owned Linux `epoll` instance (`EPOLL_CLOEXEC`).
pub struct Epoll {
    fd: OwnedFd,
}

impl Epoll {
    pub fn new() -> io::Result<Self> {
        // SAFETY: `epoll_create1` takes scalar flags and returns a new fd or -1.
        let raw = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a valid owned descriptor returned by `epoll_create1`.
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(raw) },
        })
    }

    pub fn add(&self, fd: RawFd, token: u64, events: u32) -> io::Result<()> {
        let mut ev = libc::epoll_event { events, u64: token };
        // SAFETY: `self.fd` is an open epoll fd and `&mut ev` is a valid `epoll_event`.
        let rc = unsafe { libc::epoll_ctl(self.fd.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut ev) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn wait(&self, events: &mut [libc::epoll_event], timeout_ms: i32) -> io::Result<usize> {
        loop {
            // SAFETY: `events.as_mut_ptr()` is valid for `events.len()` writable `epoll_event`s.
            let n = unsafe {
                libc::epoll_wait(
                    self.fd.as_raw_fd(),
                    events.as_mut_ptr(),
                    events.len() as libc::c_int,
                    timeout_ms,
                )
            };
            if n >= 0 {
                return Ok(n as usize);
            }
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
    }
}

/// Owned Linux `timerfd` (`CLOCK_MONOTONIC | TFD_NONBLOCK | TFD_CLOEXEC`).
pub struct TimerFd {
    fd: OwnedFd,
}

impl TimerFd {
    pub fn new_monotonic() -> io::Result<Self> {
        // SAFETY: `timerfd_create` takes scalar clockid and flags, returning a new fd or -1.
        let raw = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a fresh owned timerfd descriptor.
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(raw) },
        })
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Arms a periodic timer with interval `period_ns`.
    pub fn arm_periodic_ns(&self, period_ns: u64) -> io::Result<()> {
        let ts = ns_to_timespec(period_ns.max(1));
        let spec = libc::itimerspec {
            it_interval: ts,
            it_value: ts,
        };
        // SAFETY: `self.fd` is a valid timerfd and `&spec` is a valid `itimerspec`.
        let rc = unsafe { libc::timerfd_settime(self.fd.as_raw_fd(), 0, &spec, ptr::null_mut()) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Arms a one-shot timer firing after `delay_ns`.
    pub fn arm_oneshot_ns(&self, delay_ns: u64) -> io::Result<()> {
        let spec = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: ns_to_timespec(delay_ns.max(1)),
        };
        // SAFETY: `self.fd` is a valid timerfd and `&spec` is a valid `itimerspec`.
        let rc = unsafe { libc::timerfd_settime(self.fd.as_raw_fd(), 0, &spec, ptr::null_mut()) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Drains the 8-byte expiration counter so level-triggered epoll rearms.
    pub fn drain(&self) -> io::Result<u64> {
        let mut count = 0u64;
        // SAFETY: `self.fd` is a valid timerfd and `count` is an 8-byte stack buffer.
        let n = unsafe { libc::read(self.fd.as_raw_fd(), ptr::addr_of_mut!(count).cast(), 8) };
        if n == 8 {
            Ok(count)
        } else if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                Ok(0)
            } else {
                Err(err)
            }
        } else {
            Ok(0)
        }
    }
}

fn ns_to_timespec(ns: u64) -> libc::timespec {
    libc::timespec {
        tv_sec: (ns / 1_000_000_000) as libc::time_t,
        tv_nsec: (ns % 1_000_000_000) as libc::c_long,
    }
}

/// Owned `signalfd` blocking `SIGTERM`, `SIGINT`, and `SIGHUP` on the current thread
/// and restoring the original thread signal mask on drop.
pub struct SignalGuard {
    fd: OwnedFd,
    prev_mask: libc::sigset_t,
}

impl SignalGuard {
    pub fn block_termination_signals() -> io::Result<Self> {
        let mut set = MaybeUninit::<libc::sigset_t>::zeroed();
        let mut prev = MaybeUninit::<libc::sigset_t>::zeroed();
        // SAFETY: `set` and `prev` are valid stack `sigset_t` slots; `pthread_sigmask` blocks
        // SIGTERM/SIGINT/SIGHUP on the calling thread and saves the prior mask.
        unsafe {
            libc::sigemptyset(set.as_mut_ptr());
            libc::sigaddset(set.as_mut_ptr(), libc::SIGTERM);
            libc::sigaddset(set.as_mut_ptr(), libc::SIGINT);
            libc::sigaddset(set.as_mut_ptr(), libc::SIGHUP);
            if libc::pthread_sigmask(libc::SIG_BLOCK, set.as_ptr(), prev.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // SAFETY: `set` was initialized above.
        let mask = unsafe { set.assume_init() };
        // SAFETY: `prev` was initialized by `pthread_sigmask`.
        let prev_mask = unsafe { prev.assume_init() };

        // SAFETY: `&mask` is a valid initialized `sigset_t`; flags request non-blocking cloexec fd.
        let raw = unsafe { libc::signalfd(-1, &mask, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC) };
        if raw < 0 {
            // SAFETY: restore previous thread mask before returning error.
            unsafe {
                let _ = libc::pthread_sigmask(libc::SIG_SETMASK, &prev_mask, ptr::null_mut());
            }
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a fresh owned signalfd descriptor.
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(raw) },
            prev_mask,
        })
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Reads one pending signal number if available.
    pub fn read_signal(&self) -> io::Result<Option<i32>> {
        let mut info = MaybeUninit::<libc::signalfd_siginfo>::zeroed();
        let sz = std::mem::size_of::<libc::signalfd_siginfo>();
        // SAFETY: `info` is a valid stack buffer of `sz` bytes and `self.fd` is an open signalfd.
        let n = unsafe { libc::read(self.fd.as_raw_fd(), info.as_mut_ptr().cast(), sz) };
        if n as usize == sz {
            // SAFETY: `read` filled the entire `signalfd_siginfo` struct.
            let info = unsafe { info.assume_init() };
            Ok(Some(info.ssi_signo as i32))
        } else if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(err)
            }
        } else {
            Ok(None)
        }
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        // SAFETY: `prev_mask` was captured by `pthread_sigmask` in `block_termination_signals`.
        unsafe {
            let _ = libc::pthread_sigmask(libc::SIG_SETMASK, &self.prev_mask, ptr::null_mut());
        }
    }
}

/// Non-destructively checks whether `pid` has exited (`waitid(P_PID, pid, WEXITED|WNOHANG|WNOWAIT)`),
/// used only when the `SIGCHLD` fallback self-pipe wakes up.
pub fn child_exited_nonblocking(pid: libc::pid_t) -> bool {
    let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: `info` is a valid zeroed stack `siginfo_t` pointer; `WNOWAIT` leaves child reapable.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc != 0 {
        return false;
    }
    // SAFETY: `waitid` returned 0, initializing `info`.
    let info = unsafe { info.assume_init() };
    unsafe { info.si_pid() == pid }
}
