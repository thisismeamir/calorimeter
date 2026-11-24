//! Monotonic and wall-clock timestamps for `calo-probe`
#![forbid(unsafe_code)]

/// Returns the current `CLOCK_MONOTONIC_RAW` timestamp in nanoseconds (Spec §3 I6).
///
/// Every event, sample, series tick, and phase/region timestamp in `calo-probe`
/// uses this single clock helper.
#[inline]
pub fn now_ns() -> i64 {
    crate::sys::clock::monotonic_raw_ns()
}

/// Returns the current `CLOCK_REALTIME` wall-clock timestamp in nanoseconds since Unix epoch.
#[inline]
pub fn realtime_ns() -> i64 {
    let (secs, nsecs) = crate::sys::clock::realtime_timespec();
    secs.saturating_mul(1_000_000_000)
        .saturating_add(nsecs as i64)
}

/// Formats the current `CLOCK_REALTIME` instant as an ISO 8601 / RFC 3339 UTC timestamp
/// (`YYYY-MM-DDTHH:MM:SS.ffffffZ`) for `provenance.start` and `provenance.end` (Spec §10).
pub fn realtime_rfc3339() -> String {
    let (secs, nsecs) = crate::sys::clock::realtime_timespec();
    format_unix_utc(secs, nsecs)
}

/// Pure civil-from-days formatter (Howard Hinnant's algorithm) converting Unix epoch
/// `(seconds, nanoseconds)` into `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
pub fn format_unix_utc(unix_secs: i64, nsecs: u32) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400) as u32;

    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    let micros = nsecs / 1_000;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{micros:06}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_clock_is_non_decreasing() {
        let t0 = now_ns();
        let t1 = now_ns();
        assert!(t1 >= t0, "t1 ({t1}) < t0 ({t0})");
    }

    #[test]
    fn formats_known_epoch_timestamps() {
        assert_eq!(format_unix_utc(0, 0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(
            format_unix_utc(1_700_000_000, 123_456_000),
            "2023-11-14T22:13:20.123456Z"
        );
    }
}
