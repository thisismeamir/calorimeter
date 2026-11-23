//! Gated child process spawning, pre-exec setup, exec handshake, and `wait4` rusage (Spec §9.1).

#![forbid(unsafe_code)]

use std::{ffi::CString, io, os::fd::RawFd};

use crate::sys::spawn::SysChild;

/// NUMA memory policy applied in the child prior to `execvpe` via `SYS_set_mempolicy` (Spec §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NumaPolicy {
    /// Do not modify the inherited NUMA memory policy.
    #[default]
    None,
    /// Bind memory allocations to the specified NUMA node IDs (`MPOL_BIND`).
    Bind(Vec<u32>),
    /// Interleave memory allocations across online NUMA nodes (`MPOL_INTERLEAVE`).
    Interleave,
}

/// Standard stream redirection applied in the child prior to `execvpe` (Spec §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Redirect {
    /// Inherit the parent's file descriptor.
    #[default]
    Inherit,
    /// Redirect the stream to the given file path (opened `O_WRONLY | O_CREAT | O_TRUNC`).
    File(CString),
    /// Redirect the stream to `/dev/null`.
    Null,
}

/// Pre-allocated configuration for a gated child process (Spec §9.1 step 2).
#[derive(Debug, Clone)]
pub struct ChildSetup {
    /// Argument vector (`argv[0]` is the executable name or path).
    ///
    /// Note: `execvpe` resolves a bare `argv[0]` using the **parent's** `PATH`,
    /// not `ChildSetup.env`.
    pub argv: Vec<CString>,
    /// Environment entries in `KEY=VALUE` format passed to `execvpe`.
    pub env: Vec<CString>,
    /// Optional working directory applied via `chdir`.
    pub cwd: Option<CString>,
    /// Optional CPU affinity list applied via `sched_setaffinity`.
    pub cpu_list: Option<Vec<usize>>,
    /// NUMA memory policy applied via `SYS_set_mempolicy`.
    pub numa: NumaPolicy,
    /// Resource limits `(resource, soft, hard)` applied via `setrlimit`.
    pub rlimits: Vec<(i32, u64, u64)>,
    /// Standard output redirection.
    pub stdout: Redirect,
    /// Standard error redirection.
    pub stderr: Redirect,
}

impl Default for ChildSetup {
    fn default() -> Self {
        Self {
            argv: Vec::new(),
            env: crate::sys::spawn::current_env_cstrings(),
            cwd: None,
            cpu_list: None,
            numa: NumaPolicy::None,
            rlimits: Vec::new(),
            stdout: Redirect::Inherit,
            stderr: Redirect::Inherit,
        }
    }
}

/// Error returned by [`Child::go`] when releasing the child into `execvpe` (Spec §9.1 step 4).
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    /// The child's `execvpe` (or pre-exec setup) failed with the reported OS `errno`.
    #[error("child exec failed with errno {0}")]
    Exec(i32),
    /// An I/O error occurred on the handshake pipes in the parent.
    #[error("handshake pipe I/O error: {0}")]
    Io(#[from] io::Error),
}

impl ExecError {
    /// Returns the child's raw OS `errno` if this is [`ExecError::Exec`].
    pub fn errno(&self) -> Option<i32> {
        match self {
            Self::Exec(e) => Some(*e),
            Self::Io(_) => None,
        }
    }
}

/// Decoded exit status returned by [`Child::wait4`] (Spec §9.1 step 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    /// Normal exit code (`WEXITSTATUS`) if the child exited normally.
    pub code: Option<i32>,
    /// Terminating signal number (`WTERMSIG`) if the child was killed by a signal.
    pub signal: Option<i32>,
}

/// Resource usage counters returned by [`Child::wait4`] with normalized units (Spec §9.1, §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rusage {
    /// User CPU time in nanoseconds (`ru_utime`).
    pub utime_ns: u64,
    /// System CPU time in nanoseconds (`ru_stime`).
    pub stime_ns: u64,
    /// Maximum resident set size in bytes (`ru_maxrss * 1024`).
    pub maxrss: u64,
    /// Minor page faults (`ru_minflt`).
    pub minflt: u64,
    /// Major page faults (`ru_majflt`).
    pub majflt: u64,
    /// Voluntary context switches (`ru_nvcsw`).
    pub nvcsw: u64,
    /// Involuntary context switches (`ru_nivcsw`).
    pub nivcsw: u64,
    /// Block input operations in bytes (`ru_inblock * 512`).
    pub inblock_bytes: u64,
    /// Block output operations in bytes (`ru_oublock * 512`).
    pub oublock_bytes: u64,
}

/// Handle to a spawned child process gated on the handshake pipe (Spec §9.1 steps 2–6).
pub struct Child {
    inner: SysChild,
}

/// Spawns a child process that applies pre-exec setup (`setpgid`, `PR_SET_PDEATHSIG`,
/// `sched_setaffinity`, `set_mempolicy`, `setrlimit`, `chdir`, stream redirects, and
/// resetting `SIGPIPE`/`SIGINT`/`SIGTERM`/`SIGCHLD`/`SIGHUP` + signal mask) and blocks
/// on the go pipe before calling `execvpe` (Spec §9.1 step 2).
///
/// # Platform notes
/// - `PR_SET_PDEATHSIG` is bound to the **spawning OS thread**, not the parent process:
///   if `spawn_gated` is called from a short-lived worker thread that exits, the child
///   receives `SIGKILL` even if the parent process remains alive.
/// - `execvpe` resolves `argv[0]` using the **parent's** `PATH`, not `ChildSetup.env`.
pub fn spawn_gated(setup: &ChildSetup) -> io::Result<Child> {
    SysChild::spawn_gated(setup).map(|inner| Child { inner })
}

impl Child {
    /// Returns the child's OS process ID (`pid_t`).
    pub fn pid(&self) -> libc::pid_t {
        self.inner.pid()
    }

    /// Releases the child to call `execvpe` and waits for `execvpe` to either succeed
    /// (`O_CLOEXEC` closes the exec-error pipe) or report its `errno` (Spec §9.1 step 4).
    ///
    /// Note: `execvpe` resolves `argv[0]` using the **parent's** `PATH`, not `ChildSetup.env`.
    pub fn go(&mut self) -> Result<(), ExecError> {
        self.inner.go()
    }

    /// Returns a pollable file descriptor (`pidfd` on Linux >= 5.3, or a `SIGCHLD` self-pipe
    /// read descriptor on older kernels) for `epoll` registration (Spec §9.2 B-O1).
    ///
    /// Note: On kernels `< 5.3` where `pidfd_open` returns `ENOSYS`, the `SIGCHLD` self-pipe
    /// fallback is **untested** and a readable fd only indicates that **some** child process
    /// exited, not necessarily this specific `Child`.
    pub fn pidfd(&self) -> RawFd {
        self.inner.pidfd()
    }

    /// Returns `true` if `SYS_pidfd_open` returned `ENOSYS` and the `SIGCHLD` self-pipe
    /// fallback is active.
    pub fn is_pidfd_fallback(&self) -> bool {
        self.inner.is_pidfd_fallback()
    }

    /// Sends signal `sig` to the child's entire process group (`kill(-pid, sig)`).
    pub fn kill_group(&self, sig: i32) -> io::Result<()> {
        self.inner.kill_group(sig)
    }

    /// Waits for the child process to terminate and collects its [`ExitStatus`] and [`Rusage`]
    /// via `wait4` (Spec §9.1 step 6).
    pub fn wait4(&mut self) -> io::Result<(ExitStatus, Rusage)> {
        self.inner.wait4()
    }
}
