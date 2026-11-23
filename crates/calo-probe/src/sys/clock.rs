//! Monotonic and wall-clock time primitives via `libc::clock_gettime` (Spec §3 I6).

use std::mem::MaybeUninit;

/// Returns the current `CLOCK_MONOTONIC_RAW` timestamp in nanoseconds (Spec §3 I6).
pub fn monotonic_raw_ns() -> i64 {
    let mut ts = MaybeUninit::<libc::timespec>::zeroed();
    // SAFETY: `ts.as_mut_ptr()` points to a valid, aligned `libc::timespec` stack slot,
    // and `CLOCK_MONOTONIC_RAW` is always supported on Linux >= 2.6.28.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, ts.as_mut_ptr()) };
    if rc != 0 {
        return 0;
    }
    // SAFETY: `clock_gettime` returned 0, so `ts` was initialized by the kernel.
    let ts = unsafe { ts.assume_init() };
    (ts.tv_sec)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec)
}

/// Returns the current `CLOCK_REALTIME` timestamp as `(seconds, nanoseconds)` since Unix epoch.
pub fn realtime_timespec() -> (i64, u32) {
    let mut ts = MaybeUninit::<libc::timespec>::zeroed();
    // SAFETY: `ts.as_mut_ptr()` points to a valid, aligned `libc::timespec` stack slot,
    // and `CLOCK_REALTIME` is always supported on POSIX/Linux kernels.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, ts.as_mut_ptr()) };
    if rc != 0 {
        return (0, 0);
    }
    // SAFETY: `clock_gettime` returned 0, so `ts` was initialized by the kernel.
    let ts = unsafe { ts.assume_init() };
    (ts.tv_sec, ts.tv_nsec.max(0) as u32)
}

/// Returns the kernel user-space clock ticks per second (`_SC_CLK_TCK`, typically 100 on Linux).
pub fn clock_ticks_per_sec() -> u64 {
    // SAFETY: `sysconf(_SC_CLK_TCK)` is thread-safe and has no pointer preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz > 0 {
        hz as u64
    } else {
        100
    }
}
