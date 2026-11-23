//! Low-level FIFO creation (`mkfifo` mode 0600), non-blocking open, and `F_SETPIPE_SZ`
//! (Spec §9.4 B13).

use std::{
    ffi::CString,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
};

const PIPE_TARGET_BYTES: libc::c_int = 1024 * 1024;

/// Creates `<outputs_dir>/channel.fifo` (mode 0600), opens a non-blocking read end and a
/// dummy non-blocking write end, and best-effort raises the pipe buffer to 1 MiB.
pub fn open_channel_fifo(fifo_path: &Path) -> io::Result<(OwnedFd, OwnedFd)> {
    let c_path = CString::new(fifo_path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    // Remove stale FIFO if present, then mkfifo(0600).
    // SAFETY: `c_path.as_ptr()` is a valid NUL-terminated path.
    unsafe {
        let _ = libc::unlink(c_path.as_ptr());
        if libc::mkfifo(c_path.as_ptr(), 0o600) < 0 {
            return Err(io::Error::last_os_error());
        }
    }

    // SAFETY: `c_path.as_ptr()` is a valid NUL-terminated path; `O_RDONLY | O_NONBLOCK | O_CLOEXEC`
    // never blocks on open even before any writer opens the FIFO.
    let read_raw = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if read_raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `read_raw` is a fresh open descriptor returned by `open`.
    let read_fd = unsafe { OwnedFd::from_raw_fd(read_raw) };

    // SAFETY: Opening `O_WRONLY | O_NONBLOCK` succeeds immediately because `read_fd` is already open.
    let write_raw = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_WRONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if write_raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `write_raw` is a fresh open descriptor returned by `open`.
    let write_fd = unsafe { OwnedFd::from_raw_fd(write_raw) };

    // Best-effort increase pipe buffer from 64 KiB to 1 MiB to reduce writer stalls.
    // SAFETY: `read_fd` is an open pipe/FIFO descriptor and `F_SETPIPE_SZ` takes a scalar `c_int`.
    unsafe {
        let _ = libc::fcntl(read_fd.as_raw_fd(), libc::F_SETPIPE_SZ, PIPE_TARGET_BYTES);
    }

    Ok((read_fd, write_fd))
}

/// Non-blocking read from `fd` into `buf`, returning `Ok(0)` on `EAGAIN`/`EWOULDBLOCK` or EOF.
pub fn read_nonblocking(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        // SAFETY: `fd` is an open descriptor and `buf` is valid for `buf.len()` writable bytes.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if err.kind() == io::ErrorKind::WouldBlock {
            return Ok(0);
        }
        return Err(err);
    }
}
