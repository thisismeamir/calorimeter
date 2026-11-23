//! Low-level fork/exec handshake, process-group/pdeathsig setup, affinity/NUMA/rlimit
//! application, `pidfd_open` (with `SIGCHLD` self-pipe fallback), and `wait4` (Spec §9.1).

use std::{
    ffi::CString,
    fs, io,
    mem::MaybeUninit,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    ptr,
    sync::{
        atomic::{AtomicI32, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::spawn::{ChildSetup, ExecError, ExitStatus, NumaPolicy, Redirect, Rusage};

const MPOL_DEFAULT: libc::c_int = 0;
const MPOL_BIND: libc::c_int = 2;
const MPOL_INTERLEAVE: libc::c_int = 3;
const NODEMASK_LONGS: usize = 16;
const DEV_NULL: &[u8] = b"/dev/null\0";

/// Serializes pipe creation, `fork()`, and closing child pipe ends in the parent across threads.
static FORK_PIPE_LOCK: Mutex<()> = Mutex::new(());

/// Global write end of the fallback `SIGCHLD` pipe used only when `pidfd_open` returns `ENOSYS`.
static SIGCHLD_FALLBACK_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn sigchld_fallback_handler(_sig: libc::c_int) {
    let fd = SIGCHLD_FALLBACK_WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte: u8 = 1;
        // SAFETY: `libc::write` is async-signal-safe, and `byte` is a valid 1-byte stack buffer.
        unsafe {
            let _ = libc::write(fd, ptr::addr_of!(byte).cast(), 1);
        }
    }
}

#[derive(Clone, Copy)]
enum RawRedirect {
    Inherit,
    Null,
    File(*const libc::c_char),
}

/// All child setup data pre-allocated in the parent prior to `fork()` (Rule 3).
struct PreparedChild {
    prog: *const libc::c_char,
    argv_ptrs: Vec<*const libc::c_char>,
    envp_ptrs: Vec<*const libc::c_char>,
    cwd_ptr: Option<*const libc::c_char>,
    cpu_set: Option<libc::cpu_set_t>,
    numa_mode: libc::c_int,
    numa_nodemask: [libc::c_ulong; NODEMASK_LONGS],
    numa_maxnode: libc::c_ulong,
    rlimits: Vec<(libc::__rlimit_resource_t, libc::rlimit)>,
    stdout: RawRedirect,
    stderr: RawRedirect,
    sig_dfl: libc::sigaction,
    empty_sigset: libc::sigset_t,
}

impl PreparedChild {
    fn from_setup(setup: &ChildSetup) -> io::Result<Self> {
        if setup.argv.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ChildSetup.argv must contain at least argv[0]",
            ));
        }

        let prog = setup.argv[0].as_ptr();
        let mut argv_ptrs = Vec::with_capacity(setup.argv.len() + 1);
        for arg in &setup.argv {
            argv_ptrs.push(arg.as_ptr());
        }
        argv_ptrs.push(ptr::null());

        let mut envp_ptrs = Vec::with_capacity(setup.env.len() + 1);
        for kv in &setup.env {
            envp_ptrs.push(kv.as_ptr());
        }
        envp_ptrs.push(ptr::null());

        let cwd_ptr = setup.cwd.as_ref().map(|c| c.as_ptr());

        let cpu_set = if let Some(cpus) = &setup.cpu_list {
            if cpus.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ChildSetup.cpu_list must not be empty when Some",
                ));
            }
            // SAFETY: `cpu_set_t` is a plain bitmask POD type; `CPU_ZERO` initializes it.
            let mut set: libc::cpu_set_t = unsafe {
                let mut s = MaybeUninit::<libc::cpu_set_t>::zeroed();
                libc::CPU_ZERO(&mut *s.as_mut_ptr());
                s.assume_init()
            };
            for &cpu in cpus {
                if cpu >= libc::CPU_SETSIZE as usize {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("CPU index {cpu} exceeds CPU_SETSIZE"),
                    ));
                }
                // SAFETY: `cpu < CPU_SETSIZE` was verified above, and `set` is initialized.
                unsafe {
                    libc::CPU_SET(cpu, &mut set);
                }
            }
            Some(set)
        } else {
            None
        };

        let bits_per_long = (std::mem::size_of::<libc::c_ulong>() * 8) as u32;
        let max_supported_node = (NODEMASK_LONGS as u32) * bits_per_long;
        let mut numa_nodemask = [0 as libc::c_ulong; NODEMASK_LONGS];
        let (numa_mode, numa_maxnode) = match &setup.numa {
            NumaPolicy::None => (MPOL_DEFAULT, 0),
            NumaPolicy::Bind(nodes) => {
                if nodes.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "NumaPolicy::Bind requires at least one NUMA node",
                    ));
                }
                let mut max_node = 0u32;
                for &node in nodes {
                    if node >= max_supported_node {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("NUMA node {node} exceeds max supported node"),
                        ));
                    }
                    let word = (node / bits_per_long) as usize;
                    let bit = node % bits_per_long;
                    numa_nodemask[word] |= (1 as libc::c_ulong) << bit;
                    if node > max_node {
                        max_node = node;
                    }
                }
                (MPOL_BIND, (max_node as libc::c_ulong) + 2)
            }
            NumaPolicy::Interleave => {
                let online = read_online_numa_nodes();
                let mut max_node = 0u32;
                for node in online {
                    if node < max_supported_node {
                        let word = (node / bits_per_long) as usize;
                        let bit = node % bits_per_long;
                        numa_nodemask[word] |= (1 as libc::c_ulong) << bit;
                        if node > max_node {
                            max_node = node;
                        }
                    }
                }
                if numa_nodemask.iter().all(|&w| w == 0) {
                    numa_nodemask[0] = 1;
                }
                (MPOL_INTERLEAVE, (max_node as libc::c_ulong) + 2)
            }
        };

        let mut rlimits = Vec::with_capacity(setup.rlimits.len());
        for &(res, soft, hard) in &setup.rlimits {
            rlimits.push((
                res as libc::__rlimit_resource_t,
                libc::rlimit {
                    rlim_cur: soft as libc::rlim_t,
                    rlim_max: hard as libc::rlim_t,
                },
            ));
        }

        let stdout = match &setup.stdout {
            Redirect::Inherit => RawRedirect::Inherit,
            Redirect::Null => RawRedirect::Null,
            Redirect::File(path) => RawRedirect::File(path.as_ptr()),
        };
        let stderr = match &setup.stderr {
            Redirect::Inherit => RawRedirect::Inherit,
            Redirect::Null => RawRedirect::Null,
            Redirect::File(path) => RawRedirect::File(path.as_ptr()),
        };

        let mut empty_sigset = MaybeUninit::<libc::sigset_t>::zeroed();
        // SAFETY: `empty_sigset.as_mut_ptr()` points to a valid `libc::sigset_t` stack slot.
        if unsafe { libc::sigemptyset(empty_sigset.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `sigemptyset` succeeded and initialized `empty_sigset`.
        let empty_sigset = unsafe { empty_sigset.assume_init() };

        let mut sig_dfl = MaybeUninit::<libc::sigaction>::zeroed();
        let sa_ptr = sig_dfl.as_mut_ptr();
        // SAFETY: `sa_ptr` points to zeroed `MaybeUninit<libc::sigaction>`; `sigemptyset` initializes `sa_mask`.
        unsafe {
            (*sa_ptr).sa_sigaction = libc::SIG_DFL;
            if libc::sigemptyset(&mut (*sa_ptr).sa_mask) < 0 {
                return Err(io::Error::last_os_error());
            }
            (*sa_ptr).sa_flags = 0;
        }
        // SAFETY: All fields of `sig_dfl` have been initialized.
        let sig_dfl = unsafe { sig_dfl.assume_init() };

        Ok(Self {
            prog,
            argv_ptrs,
            envp_ptrs,
            cwd_ptr,
            cpu_set,
            numa_mode,
            numa_nodemask,
            numa_maxnode,
            rlimits,
            stdout,
            stderr,
            sig_dfl,
            empty_sigset,
        })
    }
}

fn read_online_numa_nodes() -> Vec<u32> {
    let Ok(raw) = fs::read_to_string("/sys/devices/system/node/online") else {
        return vec![0];
    };
    let mut out = Vec::new();
    for part in raw.trim().split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start_s, end_s)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start_s.parse::<u32>(), end_s.parse::<u32>()) {
                for n in s..=e {
                    out.push(n);
                }
            }
        } else if let Ok(n) = part.parse::<u32>() {
            out.push(n);
        }
    }
    if out.is_empty() {
        out.push(0);
    }
    out
}

fn create_cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1 as libc::c_int; 2];
    // SAFETY: `fds.as_mut_ptr()` points to a valid 2-element `c_int` array.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe2` returned 0, so both `fds[0]` and `fds[1]` are fresh open file descriptors.
    unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

fn open_pidfd_or_fallback(pid: libc::pid_t) -> io::Result<(OwnedFd, bool)> {
    // SAFETY: `SYS_pidfd_open` takes `(pid_t, unsigned int)` and has no memory safety preconditions.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) };
    if fd >= 0 {
        // SAFETY: `SYS_pidfd_open` returned a valid new file descriptor owned by this process.
        let owned = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };
        return Ok((owned, false));
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::ENOSYS) {
        return Err(err);
    }

    // Fallback for Linux < 5.3: self-pipe woken by SIGCHLD handler.
    let mut fds = [-1 as libc::c_int; 2];
    // SAFETY: `fds.as_mut_ptr()` points to a valid 2-element `c_int` array.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe2` succeeded, producing two valid descriptors.
    let (read_fd, write_fd) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let prev = SIGCHLD_FALLBACK_WRITE_FD.swap(write_fd.as_raw_fd(), Ordering::SeqCst);
    std::mem::forget(write_fd);
    if prev >= 0 {
        // SAFETY: `prev` was an owned pipe write descriptor stored by an earlier fallback setup.
        unsafe {
            libc::close(prev);
        }
    }

    // SAFETY: `sigaction` struct is zero-initialized and populated with a valid extern "C" fn pointer.
    unsafe {
        let mut sa = MaybeUninit::<libc::sigaction>::zeroed();
        let p = sa.as_mut_ptr();
        (*p).sa_sigaction = sigchld_fallback_handler as *const() as usize;
        libc::sigemptyset(&mut (*p).sa_mask);
        (*p).sa_flags = libc::SA_RESTART | libc::SA_NOCLDSTOP;
        if libc::sigaction(libc::SIGCHLD, p, ptr::null_mut()) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read_fd, true))
}

/// Async-signal-safe helper to read the last OS errno in the child.
///
/// # Safety
/// Must be called on the thread that just executed a failing `libc` call.
#[inline]
unsafe fn child_errno() -> i32 {
    *libc::__errno_location()
}

/// Async-signal-safe helper executed in the child when a pre-exec syscall fails.
/// Writes the 4-byte errno to `err_w`, waits for the parent to either send the go byte
/// or close `go_w` (avoiding `EPIPE` in the parent), and exits with code 126.
///
/// # Safety
/// Must only be called in the child process between `fork` and `execvpe` with valid `err_w` and `go_r`.
unsafe fn child_fail_pre_exec(err_w: RawFd, go_r: RawFd, errno: i32) -> ! {
    let bytes = errno.to_ne_bytes();
    let _ = libc::write(err_w, bytes.as_ptr().cast(), bytes.len());
    let _ = libc::close(err_w);
    let mut b = 0u8;
    loop {
        let n = libc::read(go_r, ptr::addr_of_mut!(b).cast(), 1);
        if n >= 0 || child_errno() != libc::EINTR {
            break;
        }
    }
    let _ = libc::close(go_r);
    libc::_exit(126)
}

/// Async-signal-safe redirection helper in the child.
///
/// # Safety
/// Must only be called in the child between `fork` and `execvpe`. Any `File(ptr)` pointer
/// must point to a valid NUL-terminated string pre-allocated before `fork`.
unsafe fn child_apply_redirect(redirect: RawRedirect, target_fd: libc::c_int) -> Result<(), i32> {
    let (path_ptr, flags, mode) = match redirect {
        RawRedirect::Inherit => return Ok(()),
        RawRedirect::Null => (
            DEV_NULL.as_ptr().cast::<libc::c_char>(),
            libc::O_WRONLY | libc::O_CLOEXEC,
            0,
        ),
        RawRedirect::File(ptr) => (
            ptr,
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
            0o644,
        ),
    };
    let fd = libc::open(path_ptr, flags, mode);
    if fd < 0 {
        return Err(child_errno());
    }
    if libc::dup2(fd, target_fd) < 0 {
        let err = child_errno();
        let _ = libc::close(fd);
        return Err(err);
    }
    if fd != target_fd {
        let _ = libc::close(fd);
    }
    Ok(())
}

/// Async-signal-safe child entrypoint between `fork()` and `execvpe()` (Rule 3).
///
/// # Safety
/// Must only be called in the newly forked child process (`fork() == 0`).
/// All pointers and signal structs inside `prep` must remain valid and immutable in the
/// child's address space.
unsafe fn child_entrypoint(
    prep: &PreparedChild,
    parent_pid: libc::pid_t,
    go_r: RawFd,
    go_w: RawFd,
    err_r: RawFd,
    err_w: RawFd,
) -> ! {
    // 1. Close parent pipe ends.
    let _ = libc::close(go_w);
    let _ = libc::close(err_r);

    // 2. Place child in its own process group so timeout kills the whole subtree.
    if libc::setpgid(0, 0) < 0 {
        child_fail_pre_exec(err_w, go_r, child_errno());
    }

    // 3. Ensure child dies with SIGKILL if parent dies, then re-check getppid() to close the fork race.
    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) < 0 {
        child_fail_pre_exec(err_w, go_r, child_errno());
    }
    if libc::getppid() != parent_pid {
        let _ = libc::close(go_r);
        let _ = libc::close(err_w);
        libc::_exit(128 + libc::SIGKILL);
    }

    // 4. Apply CPU affinity if requested.
    if let Some(ref set) = prep.cpu_set {
        let rc = libc::sched_setaffinity(
            0,
            std::mem::size_of::<libc::cpu_set_t>(),
            ptr::addr_of!(*set),
        );
        if rc < 0 {
            child_fail_pre_exec(err_w, go_r, child_errno());
        }
    }

    // 5. Apply NUMA policy via raw `SYS_set_mempolicy` syscall if requested.
    if prep.numa_mode != MPOL_DEFAULT {
        let rc = libc::syscall(
            libc::SYS_set_mempolicy,
            prep.numa_mode,
            prep.numa_nodemask.as_ptr(),
            prep.numa_maxnode,
        );
        if rc < 0 {
            child_fail_pre_exec(err_w, go_r, child_errno());
        }
    }

    // 6. Apply rlimits.
    for &(resource, ref lim) in &prep.rlimits {
        if libc::setrlimit(resource, ptr::addr_of!(*lim)) < 0 {
            child_fail_pre_exec(err_w, go_r, child_errno());
        }
    }

    // 7. Apply working directory.
    if let Some(cwd_ptr) = prep.cwd_ptr {
        if libc::chdir(cwd_ptr) < 0 {
            child_fail_pre_exec(err_w, go_r, child_errno());
        }
    }

    // 8. Apply stdout and stderr redirections.
    if let Err(errno) = child_apply_redirect(prep.stdout, libc::STDOUT_FILENO) {
        child_fail_pre_exec(err_w, go_r, errno);
    }
    if let Err(errno) = child_apply_redirect(prep.stderr, libc::STDERR_FILENO) {
        child_fail_pre_exec(err_w, go_r, errno);
    }

    // 9. Reset signal dispositions to SIG_DFL and clear the signal mask before blocking.
    const RESET_SIGNALS: [libc::c_int; 5] = [
        libc::SIGPIPE,
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGCHLD,
        libc::SIGHUP,
    ];
    for &sig in &RESET_SIGNALS {
        if libc::sigaction(sig, ptr::addr_of!(prep.sig_dfl), ptr::null_mut()) < 0 {
            child_fail_pre_exec(err_w, go_r, child_errno());
        }
    }
    if libc::sigprocmask(
        libc::SIG_SETMASK,
        ptr::addr_of!(prep.empty_sigset),
        ptr::null_mut(),
    ) < 0
    {
        child_fail_pre_exec(err_w, go_r, child_errno());
    }

    // 10. Block on the go pipe until the parent signals readiness or drops Child.
    let mut go_byte: u8 = 0;
    loop {
        let n = libc::read(go_r, ptr::addr_of_mut!(go_byte).cast(), 1);
        if n == 1 {
            break;
        }
        if n == 0 {
            // Parent closed go_w without writing a go byte (aborted / dropped before go).
            let _ = libc::close(go_r);
            let _ = libc::close(err_w);
            libc::_exit(126);
        }
        let err = child_errno();
        if err != libc::EINTR {
            let bytes = err.to_ne_bytes();
            let _ = libc::write(err_w, bytes.as_ptr().cast(), bytes.len());
            let _ = libc::close(go_r);
            let _ = libc::close(err_w);
            libc::_exit(126);
        }
    }
    let _ = libc::close(go_r);

    // 11. Execute the target image. On success, O_CLOEXEC closes `err_w`.
    let _ = libc::execvpe(prep.prog, prep.argv_ptrs.as_ptr(), prep.envp_ptrs.as_ptr());
    let errno = child_errno();
    let bytes = errno.to_ne_bytes();
    let _ = libc::write(err_w, bytes.as_ptr().cast(), bytes.len());
    let _ = libc::close(err_w);
    libc::_exit(127)
}

// This wait is for observability (e.g., ensuring `/proc/<pid>/stat` shows `'S'` when callers
// inspect the gated child), not for ordering: `read(go_r)` in the child already enforces the
// pre-exec synchronization barrier.
fn wait_until_child_parked(pid: libc::pid_t) -> io::Result<()> {
    let stat_path = format!("/proc/{pid}/stat");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match fs::read_to_string(&stat_path) {
            Ok(content) => {
                if let Some(idx) = content.rfind(')') {
                    let state = content[idx + 1..].trim_start().chars().next();
                    if matches!(state, Some('S' | 'Z' | 'X')) {
                        return Ok(());
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
        thread::sleep(Duration::from_micros(50));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("timed out waiting for child {pid} to park on go pipe"),
    ))
}

/// Internal OS handle backing [`crate::spawn::Child`].
pub struct SysChild {
    pid: libc::pid_t,
    go_w: Option<OwnedFd>,
    err_r: Option<OwnedFd>,
    pidfd: OwnedFd,
    pidfd_fallback: bool,
    reaped: bool,
}

impl SysChild {
    pub fn spawn_gated(setup: &ChildSetup) -> io::Result<Self> {
        let prep = PreparedChild::from_setup(setup)?;
        let fork_guard = FORK_PIPE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let (go_r, go_w) = create_cloexec_pipe()?;
        let (err_r, err_w) = create_cloexec_pipe()?;

        // SAFETY: `getpid` is always safe and has no preconditions.
        let parent_pid = unsafe { libc::getpid() };
        let go_r_raw = go_r.as_raw_fd();
        let go_w_raw = go_w.as_raw_fd();
        let err_r_raw = err_r.as_raw_fd();
        let err_w_raw = err_w.as_raw_fd();

        // SAFETY: All data required by the child has been pre-allocated in `prep`, and the child
        // executes only async-signal-safe functions before `execvpe` or `_exit`.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            // SAFETY: Executing in the freshly forked child process with pre-allocated `prep`.
            unsafe {
                child_entrypoint(&prep, parent_pid, go_r_raw, go_w_raw, err_r_raw, err_w_raw);
            }
        }

        // Drop child-side pipe ends in the parent immediately before releasing FORK_PIPE_LOCK.
        drop(go_r);
        drop(err_w);
        drop(fork_guard);

        let (pidfd, pidfd_fallback) = match open_pidfd_or_fallback(pid) {
            Ok(pair) => pair,
            Err(e) => {
                drop(go_w);
                drop(err_r);
                kill_and_reap_raw(pid);
                return Err(e);
            }
        };

        if let Err(e) = wait_until_child_parked(pid) {
            drop(go_w);
            drop(err_r);
            kill_and_reap_raw(pid);
            return Err(e);
        }

        Ok(Self {
            pid,
            go_w: Some(go_w),
            err_r: Some(err_r),
            pidfd,
            pidfd_fallback,
            reaped: false,
        })
    }

    pub fn pid(&self) -> libc::pid_t {
        self.pid
    }

    pub fn pidfd(&self) -> RawFd {
        self.pidfd.as_raw_fd()
    }

    pub fn is_pidfd_fallback(&self) -> bool {
        self.pidfd_fallback
    }

    pub fn kill_group(&self, sig: i32) -> io::Result<()> {
        // SAFETY: `self.pid > 0`, so `-self.pid` targets the child's process group.
        let rc = unsafe { libc::kill(-self.pid, sig) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn go(&mut self) -> Result<(), ExecError> {
        let Some(go_w) = self.go_w.take() else {
            return Err(ExecError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Child::go called more than once",
            )));
        };
        let Some(err_r) = self.err_r.take() else {
            return Err(ExecError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "exec-error pipe already consumed",
            )));
        };

        let byte: u8 = 1;
        loop {
            // SAFETY: `go_w` is a valid open pipe descriptor and `byte` is 1 byte on the stack.
            let n = unsafe { libc::write(go_w.as_raw_fd(), ptr::addr_of!(byte).cast(), 1) };
            if n == 1 {
                break;
            }
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if err.kind() == io::ErrorKind::BrokenPipe {
                    break;
                }
                return Err(ExecError::Io(err));
            }
        }
        // Close write end before reading exec-error pipe.
        drop(go_w);

        let mut buf = [0u8; 4];
        let mut read_total = 0usize;
        while read_total < buf.len() {
            // SAFETY: `err_r` is a valid open pipe descriptor and `buf[read_total..]` is within bounds.
            let n = unsafe {
                libc::read(
                    err_r.as_raw_fd(),
                    buf.as_mut_ptr().add(read_total).cast(),
                    buf.len() - read_total,
                )
            };
            if n == 0 {
                break;
            }
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(ExecError::Io(err));
            }
            read_total += n as usize;
        }
        drop(err_r);

        if read_total == 0 {
            Ok(())
        } else if read_total == 4 {
            let errno = i32::from_ne_bytes(buf);
            Err(ExecError::Exec(errno))
        } else {
            Err(ExecError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short read on exec-error pipe",
            )))
        }
    }

    pub fn wait4(&mut self) -> io::Result<(ExitStatus, Rusage)> {
        if self.reaped {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Child::wait4 called on an already-reaped child",
            ));
        }
        let mut status: libc::c_int = 0;
        let mut ru = MaybeUninit::<libc::rusage>::zeroed();
        loop {
            // SAFETY: `status` and `ru` are valid stack pointers for `libc::wait4`.
            let rc = unsafe { libc::wait4(self.pid, &mut status, 0, ru.as_mut_ptr()) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            break;
        }
        self.reaped = true;
        // SAFETY: `wait4` succeeded, so `ru` has been initialized by the kernel.
        let ru = unsafe { ru.assume_init() };

        let exit_status = if libc::WIFEXITED(status) {
            ExitStatus {
                code: Some(libc::WEXITSTATUS(status)),
                signal: None,
            }
        } else if libc::WIFSIGNALED(status) {
            ExitStatus {
                code: None,
                signal: Some(libc::WTERMSIG(status)),
            }
        } else {
            ExitStatus {
                code: None,
                signal: None,
            }
        };

        let utime_ns = timeval_to_ns(ru.ru_utime);
        let stime_ns = timeval_to_ns(ru.ru_stime);
        let rusage = Rusage {
            utime_ns,
            stime_ns,
            maxrss: (ru.ru_maxrss.max(0) as u64).saturating_mul(1024),
            minflt: ru.ru_minflt.max(0) as u64,
            majflt: ru.ru_majflt.max(0) as u64,
            nvcsw: ru.ru_nvcsw.max(0) as u64,
            nivcsw: ru.ru_nivcsw.max(0) as u64,
            inblock_bytes: (ru.ru_inblock.max(0) as u64).saturating_mul(512),
            oublock_bytes: (ru.ru_oublock.max(0) as u64).saturating_mul(512),
        };
        Ok((exit_status, rusage))
    }
}

impl Drop for SysChild {
    fn drop(&mut self) {
        // Close pipes first so an un-`go`ed child wakes from `read(go_r)`.
        self.go_w.take();
        self.err_r.take();
        if !self.reaped {
            kill_and_reap_raw(self.pid);
            self.reaped = true;
        } else {
            // Even after the direct child was reaped, send SIGKILL to its process group
            // to clean up any surviving background grandchildren.
            // SAFETY: `self.pid > 0`, so `-self.pid` targets the child's process group.
            unsafe {
                let _ = libc::kill(-self.pid, libc::SIGKILL);
            }
        }
    }
}

fn kill_and_reap_raw(pid: libc::pid_t) {
    // SAFETY: Sending SIGKILL to the child's process group (`-pid`) and pid (`pid`) is safe.
    unsafe {
        let _ = libc::kill(-pid, libc::SIGKILL);
        let _ = libc::kill(pid, libc::SIGKILL);
    }
    let mut status: libc::c_int = 0;
    loop {
        // SAFETY: `status` is a valid stack pointer and `ptr::null_mut()` is allowed for rusage.
        let rc = unsafe { libc::wait4(pid, &mut status, 0, ptr::null_mut()) };
        if rc >= 0 {
            break;
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
}

fn timeval_to_ns(tv: libc::timeval) -> u64 {
    let secs = tv.tv_sec.max(0) as u64;
    let usecs = tv.tv_usec.max(0) as u64;
    secs.saturating_mul(1_000_000_000)
        .saturating_add(usecs.saturating_mul(1_000))
}

/// Helper to construct a default `CString` environment slice from current environment variables.
pub fn current_env_cstrings() -> Vec<CString> {
    std::env::vars()
        .filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok())
        .collect()
}
