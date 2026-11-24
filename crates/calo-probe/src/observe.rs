//! Single-threaded level-triggered `epoll` observe loop (Spec §9.1 step 5, §9.2 B-O1).

#![forbid(unsafe_code)]

use std::{io, os::fd::RawFd};

use crate::{
    channel::{ChannelAccumulator, ChannelEvent, ChannelFifo},
    clock::now_ns,
    gpu::GpuStateCollector,
    profile::ProfileCollector,
    series::HostSampler,
    spawn::Child,
    sys::poll::{child_exited_nonblocking, Epoll, SignalGuard, TimerFd},
    table_writer::WriterHandle,
};

const TOK_PIDFD: u64 = 1;
const TOK_CHANNEL: u64 = 2;
const TOK_SERIES_TIMER: u64 = 3;
const TOK_SIGNAL: u64 = 4;
const TOK_TIMEOUT: u64 = 5;
const TOK_GPU_TIMER: u64 = 6;
const TOK_PROFILE_BASE: u64 = 100;
const GRACE_NS: u64 = 5_000_000_000;

/// Outcome of the observe loop when the child exits.
#[derive(Debug, Clone, Copy)]
pub struct ObserveOutcome {
    pub exit_ns: i64,
    pub timed_out: bool,
    pub signalled_by_parent: Option<i32>,
}

/// Runs the single-threaded level-triggered `epoll` loop until the child exits.
#[allow(clippy::too_many_arguments)]
pub fn run_observe_loop(
    child: &Child,
    channel: &mut ChannelFifo,
    acc: &mut ChannelAccumulator,
    sampler: &mut HostSampler,
    writer: &WriterHandle,
    sig_guard: &SignalGuard,
    sample_hz: u32,
    timeout_s: Option<u64>,
    mut profile: Option<&mut ProfileCollector>,
    mut gpu: Option<&mut GpuStateCollector>,
    gpu_sample_hz: u32,
) -> io::Result<ObserveOutcome> {
    let epoll = Epoll::new()?;
    let read_mask = libc::EPOLLIN as u32;

    epoll.add(child.pidfd(), TOK_PIDFD, read_mask)?;
    epoll.add(channel.read_fd(), TOK_CHANNEL, read_mask)?;
    epoll.add(sig_guard.as_raw_fd(), TOK_SIGNAL, read_mask)?;

    let series_timer = TimerFd::new_monotonic()?;
    let period_ns = 1_000_000_000u64 / u64::from(sample_hz.max(1));
    series_timer.arm_periodic_ns(period_ns)?;
    epoll.add(series_timer.as_raw_fd(), TOK_SERIES_TIMER, read_mask)?;

    let timeout_timer = if let Some(secs) = timeout_s {
        let t = TimerFd::new_monotonic()?;
        t.arm_oneshot_ns(secs.saturating_mul(1_000_000_000).max(1))?;
        epoll.add(t.as_raw_fd(), TOK_TIMEOUT, read_mask)?;
        Some(t)
    } else {
        None
    };

    let gpu_timer = if gpu.is_some() {
        let t = TimerFd::new_monotonic()?;
        let gpu_period_ns = 1_000_000_000u64 / u64::from(gpu_sample_hz.max(1));
        t.arm_periodic_ns(gpu_period_ns)?;
        epoll.add(t.as_raw_fd(), TOK_GPU_TIMER, read_mask)?;
        Some(t)
    } else {
        None
    };

    let profile_fds: Vec<RawFd> = if let Some(ref prof) = profile {
        let fds = prof.raw_fds();
        for (idx, &fd) in fds.iter().enumerate() {
            epoll.add(fd, TOK_PROFILE_BASE + idx as u64, read_mask)?;
        }
        fds
    } else {
        Vec::new()
    };

    let mut events = [libc::epoll_event { events: 0, u64: 0 }; 16];
    let mut channel_buf: Vec<ChannelEvent> = Vec::with_capacity(256);
    let mut timed_out = false;
    let mut grace_armed = false;
    let mut signalled_by_parent: Option<i32> = None;

    loop {
        let n = epoll.wait(&mut events, -1)?;
        for ev in &events[..n] {
            let tok = ev.u64;
            match tok {
                TOK_PIDFD => {
                    // Capture exit timestamp FIRST before any draining or reaping.
                    let exit_ns = now_ns();
                    if child.is_pidfd_fallback() {
                        // SIGCHLD self-pipe indicates some child exited; verify ours exited.
                        let mut scratch = [0u8; 64];
                        let _ =
                            crate::sys::channel::read_nonblocking(child.pidfd(), &mut scratch);
                        if !child_exited_nonblocking(child.pid()) {
                            continue;
                        }
                    }
                    return Ok(ObserveOutcome {
                        exit_ns,
                        timed_out,
                        signalled_by_parent,
                    });
                }
                TOK_SERIES_TIMER => {
                    let _ = series_timer.drain();
                    if let Some(row) = sampler.sample() {
                        writer.send_host_series(row);
                    }
                }
                TOK_CHANNEL => {
                    let t = now_ns();
                    channel_buf.clear();
                    let _ = channel.drain_ready(t, &mut channel_buf);
                    for cev in channel_buf.drain(..) {
                        acc.apply(cev);
                    }
                }
                TOK_SIGNAL => {
                    while let Ok(Some(sig)) = sig_guard.read_signal() {
                        signalled_by_parent = Some(sig);
                        let _ = child.kill_group(sig);
                    }
                }
                TOK_TIMEOUT => {
                    if let Some(ref t) = timeout_timer {
                        let _ = t.drain();
                        if !grace_armed {
                            timed_out = true;
                            grace_armed = true;
                            let _ = child.kill_group(libc::SIGTERM);
                            let _ = t.arm_oneshot_ns(GRACE_NS);
                        } else {
                            let _ = child.kill_group(libc::SIGKILL);
                        }
                    }
                }
                TOK_GPU_TIMER => {
                    if let Some(ref t) = gpu_timer {
                        let _ = t.drain();
                    }
                    if let Some(ref mut g) = gpu {
                        g.sample_all(now_ns());
                    }
                }
                t if t >= TOK_PROFILE_BASE => {
                    if let Some(ref mut prof) = profile {
                        let idx = (t - TOK_PROFILE_BASE) as usize;
                        if let Some(&fd) = profile_fds.get(idx) {
                            prof.drain_fd(Some(fd));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
