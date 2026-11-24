//! Pure parsers and readers for `/proc/<pid>/stat`, `/proc/<pid>/status`,
//! cgroup v2 snapshots/deltas, RAPL energy counters, and `peak_rss_bytes` (Spec §9.3 B2/B4, §10).

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Parsed fields from `/proc/<pid>/stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcStat {
    pub pid: i32,
    pub comm: String,
    pub state: char,
    pub ppid: i32,
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    pub cutime_ticks: u64,
    pub cstime_ticks: u64,
    pub num_threads: u64,
}

/// Parsed fields from `/proc/<pid>/status` (sizes normalized to bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcStatus {
    pub vm_rss_bytes: Option<u64>,
    pub vm_hwm_bytes: Option<u64>,
    pub threads: Option<u64>,
}

/// Point-in-time cgroup v2 resource snapshot.
///
/// Note: cgroup v2 statistics are **cgroup-wide** and shared with any other processes
/// running in the same cgroup. `memory_peak` (`memory.peak`) is cumulative over the
/// cgroup's lifetime (informational only) and must **never** feed `peak_rss_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CgroupSnapshot {
    pub cpu_usage_us: Option<u64>,
    pub cpu_user_us: Option<u64>,
    pub cpu_system_us: Option<u64>,
    pub nr_throttled: Option<u64>,
    pub throttled_us: Option<u64>,
    pub io_rbytes: Option<u64>,
    pub io_wbytes: Option<u64>,
    /// Cumulative cgroup `memory.peak` in bytes (informational only; never use for `peak_rss_bytes`).
    pub memory_peak: Option<u64>,
}

/// Difference between two [`CgroupSnapshot`]s (`later - self`).
///
/// Any field where a counter reset occurred (`later < self`) or either endpoint was
/// absent yields `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CgroupDelta {
    pub cpu_usage_us: Option<u64>,
    pub cpu_user_us: Option<u64>,
    pub cpu_system_us: Option<u64>,
    pub nr_throttled: Option<u64>,
    pub throttled_us: Option<u64>,
    pub io_rbytes: Option<u64>,
    pub io_wbytes: Option<u64>,
}

impl CgroupSnapshot {
    /// Reads a cgroup v2 snapshot for `pid` from `/proc/<pid>/cgroup` and `/sys/fs/cgroup`,
    /// returning `None` silently if cgroup v2 is not active or unreadable.
    pub fn read(pid: libc::pid_t) -> Option<Self> {
        Self::read_with_roots(pid, Path::new("/proc"), Path::new("/sys/fs/cgroup"))
    }

    /// Reads a cgroup v2 snapshot using injectable `proc_root` and `cgroup_root` paths.
    pub fn read_with_roots(pid: libc::pid_t, proc_root: &Path, cgroup_root: &Path) -> Option<Self> {
        let cg_file = fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).ok()?;
        let rel_path = parse_proc_cgroup_v2_path(&cg_file)?;
        let rel_trimmed = rel_path.trim_start_matches('/');
        let cg_dir = if rel_trimmed.is_empty() {
            cgroup_root.to_path_buf()
        } else {
            cgroup_root.join(rel_trimmed)
        };
        if !cg_dir.exists() {
            return None;
        }

        let mut snap = Self::default();
        if let Ok(mem_peak) = fs::read_to_string(cg_dir.join("memory.peak")) {
            snap.memory_peak = mem_peak.trim().parse::<u64>().ok();
        }
        if let Ok(cpu_stat) = fs::read_to_string(cg_dir.join("cpu.stat")) {
            parse_cgroup_cpu_stat(&cpu_stat, &mut snap);
        }
        if let Ok(io_stat) = fs::read_to_string(cg_dir.join("io.stat")) {
            parse_cgroup_io_stat(&io_stat, &mut snap);
        }

        if snap == Self::default() {
            None
        } else {
            Some(snap)
        }
    }

    /// Computes monotonic counter deltas between `self` (earlier) and `later`.
    /// Any counter that decreased (reset) or was missing yields `None`.
    pub fn delta(&self, later: &Self) -> CgroupDelta {
        CgroupDelta {
            cpu_usage_us: sub_opt(later.cpu_usage_us, self.cpu_usage_us),
            cpu_user_us: sub_opt(later.cpu_user_us, self.cpu_user_us),
            cpu_system_us: sub_opt(later.cpu_system_us, self.cpu_system_us),
            nr_throttled: sub_opt(later.nr_throttled, self.nr_throttled),
            throttled_us: sub_opt(later.throttled_us, self.throttled_us),
            io_rbytes: sub_opt(later.io_rbytes, self.io_rbytes),
            io_wbytes: sub_opt(later.io_wbytes, self.io_wbytes),
        }
    }
}

fn sub_opt(later: Option<u64>, earlier: Option<u64>) -> Option<u64> {
    match (later, earlier) {
        (Some(b), Some(a)) => b.checked_sub(a),
        _ => None,
    }
}

/// RAPL energy totals in Joules over a run (Spec §10 `energy_j`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RaplEnergyJ {
    pub pkg: Option<f64>,
    pub dram: Option<f64>,
}

/// Converts `/proc/<pid>/stat` clock ticks into nanoseconds using `clk_tck` (`_SC_CLK_TCK`).
pub fn ticks_to_ns(ticks: u64, clk_tck: u64) -> u64 {
    let hz = clk_tck.max(1) as u128;
    ((ticks as u128) * 1_000_000_000 / hz) as u64
}

/// Combines `rusage.maxrss` (bytes) and the sampled `series.host` peak RSS (bytes),
/// returning the larger value (Spec §10 `peak_rss_bytes`).
pub fn merge_peak_rss(rusage_maxrss_bytes: u64, series_peak_rss_bytes: Option<u64>) -> u64 {
    rusage_maxrss_bytes.max(series_peak_rss_bytes.unwrap_or(0))
}

/// Tolerant pure parser for `/proc/<pid>/stat`.
///
/// Splits at the **last** `)` so `comm` values containing spaces or parentheses parse cleanly.
pub fn parse_proc_stat(content: &str) -> Option<ProcStat> {
    let lparen = content.find('(')?;
    let rparen = content.rfind(')')?;
    if rparen <= lparen {
        return None;
    }

    let pid = content[..lparen].trim().parse::<i32>().ok()?;
    let comm = content[lparen + 1..rparen].to_string();
    let rest = content[rparen + 1..].trim_start();
    let fields: Vec<&str> = rest.split_whitespace().collect();

    // Fields after `)`:
    // 0: state, 1: ppid, ..., 11: utime, 12: stime, 13: cutime, 14: cstime, 17: num_threads
    let state = fields.first()?.chars().next()?;
    let ppid = fields.get(1)?.parse::<i32>().ok()?;
    let utime_ticks = fields.get(11)?.parse::<u64>().ok()?;
    let stime_ticks = fields.get(12)?.parse::<u64>().ok()?;
    let cutime_ticks = fields
        .get(13)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let cstime_ticks = fields
        .get(14)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let num_threads = fields
        .get(17)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);

    Some(ProcStat {
        pid,
        comm,
        state,
        ppid,
        utime_ticks,
        stime_ticks,
        cutime_ticks,
        cstime_ticks,
        num_threads,
    })
}

/// Reads and parses `/proc/<pid>/stat` under `proc_root`.
pub fn read_proc_stat_in(proc_root: &Path, pid: libc::pid_t) -> Option<ProcStat> {
    let content = fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    parse_proc_stat(&content)
}

/// Reads and parses `/proc/<pid>/stat`.
pub fn read_proc_stat(pid: libc::pid_t) -> Option<ProcStat> {
    read_proc_stat_in(Path::new("/proc"), pid)
}

/// Tolerant pure parser for `/proc/<pid>/status`.
pub fn parse_proc_status(content: &str) -> ProcStatus {
    let mut out = ProcStatus::default();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            out.vm_rss_bytes = parse_kib_field(rest);
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            out.vm_hwm_bytes = parse_kib_field(rest);
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            out.threads = rest.trim().parse::<u64>().ok();
        }
    }
    out
}

fn parse_kib_field(raw: &str) -> Option<u64> {
    let num = raw.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(num.saturating_mul(1024))
}

/// Reads and parses `/proc/<pid>/status` under `proc_root`.
pub fn read_proc_status_in(proc_root: &Path, pid: libc::pid_t) -> Option<ProcStatus> {
    let content = fs::read_to_string(proc_root.join(pid.to_string()).join("status")).ok()?;
    Some(parse_proc_status(&content))
}

/// Reads and parses `/proc/<pid>/status`.
pub fn read_proc_status(pid: libc::pid_t) -> Option<ProcStatus> {
    read_proc_status_in(Path::new("/proc"), pid)
}

/// Parses `/proc/<pid>/cgroup` and returns the cgroup v2 relative path (`0::<path>`), if present.
pub fn parse_proc_cgroup_v2_path(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("0::") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Pure parser for cgroup v2 `cpu.stat` (`usage_usec`, `user_usec`, `system_usec`, etc.).
pub fn parse_cgroup_cpu_stat(content: &str, snap: &mut CgroupSnapshot) {
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(val_s)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(val) = val_s.parse::<u64>() else {
            continue;
        };
        match key {
            "usage_usec" => snap.cpu_usage_us = Some(val),
            "user_usec" => snap.cpu_user_us = Some(val),
            "system_usec" => snap.cpu_system_us = Some(val),
            "nr_throttled" => snap.nr_throttled = Some(val),
            "throttled_usec" => snap.throttled_us = Some(val),
            _ => {}
        }
    }
}

/// Pure parser for cgroup v2 `io.stat` (sums `rbytes=` and `wbytes=` across all block devices).
pub fn parse_cgroup_io_stat(content: &str, snap: &mut CgroupSnapshot) {
    let mut total_r: u64 = 0;
    let mut total_w: u64 = 0;
    let mut any = false;
    for line in content.lines() {
        for token in line.split_whitespace() {
            if let Some(val_s) = token.strip_prefix("rbytes=") {
                if let Ok(v) = val_s.parse::<u64>() {
                    total_r = total_r.saturating_add(v);
                    any = true;
                }
            } else if let Some(val_s) = token.strip_prefix("wbytes=") {
                if let Ok(v) = val_s.parse::<u64>() {
                    total_w = total_w.saturating_add(v);
                    any = true;
                }
            }
        }
    }
    if any {
        snap.io_rbytes = Some(total_r);
        snap.io_wbytes = Some(total_w);
    }
}

/// Computes wrap-aware energy delta in microjoules between two RAPL `energy_uj` readings.
pub fn rapl_delta_uj(prev: u64, curr: u64, max_range: Option<u64>) -> Option<u64> {
    if curr >= prev {
        Some(curr - prev)
    } else if let Some(max) = max_range {
        if max >= prev {
            Some((max - prev).saturating_add(curr))
        } else {
            None
        }
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RaplDomainKind {
    Pkg,
    Dram,
}

#[derive(Debug, Clone)]
pub(crate) struct RaplDomain {
    pub kind: RaplDomainKind,
    pub energy_path: PathBuf,
    pub max_range_uj: Option<u64>,
    pub prev_uj: u64,
    pub total_uj: u64,
}

pub(crate) fn discover_rapl_domains_in(
    powercap_root: &Path,
) -> (BTreeMap<PathBuf, RaplDomain>, Vec<String>) {
    let mut domains = BTreeMap::new();
    let mut notes = Vec::new();
    let entries = match fs::read_dir(powercap_root) {
        Ok(e) => e,
        Err(err) => {
            let errno = err.raw_os_error().unwrap_or(0);
            notes.push(format!(
                "RAPL powercap directory '{}' unreadable (errno {errno}): {err}",
                powercap_root.display()
            ));
            return (domains, notes);
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("intel-rapl:") {
            continue;
        }
        let dir = entry.path();
        let Ok(domain_name) = fs::read_to_string(dir.join("name")) else {
            continue;
        };
        let domain_lower = domain_name.trim().to_ascii_lowercase();
        let kind = if domain_lower.starts_with("package") {
            RaplDomainKind::Pkg
        } else if domain_lower == "dram" || domain_lower.starts_with("dram") {
            RaplDomainKind::Dram
        } else {
            continue;
        };

        let energy_path = dir.join("energy_uj");
        let raw_energy = match fs::read_to_string(&energy_path) {
            Ok(s) => s,
            Err(err) => {
                let errno = err.raw_os_error().unwrap_or(0);
                notes.push(format!(
                    "RAPL counter '{}' unreadable (errno {errno}): {err}",
                    energy_path.display()
                ));
                continue;
            }
        };
        let Ok(first_uj) = raw_energy.trim().parse::<u64>() else {
            continue;
        };
        let max_range_uj = fs::read_to_string(dir.join("max_energy_range_uj"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());

        domains.insert(
            dir,
            RaplDomain {
                kind,
                energy_path,
                max_range_uj,
                prev_uj: first_uj,
                total_uj: 0,
            },
        );
    }
    (domains, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat_with_spaces_parens_and_children_times() {
        let fixture = "4210 (my (weird) proc 2) R 1 4210 4210 0 -1 4194304 100 0 2 0 125 35 40 15 20 0 4 0 12345 1048576 256";
        let st = parse_proc_stat(fixture).expect("parse proc stat");
        assert_eq!(st.pid, 4210);
        assert_eq!(st.comm, "my (weird) proc 2");
        assert_eq!(st.state, 'R');
        assert_eq!(st.ppid, 1);
        assert_eq!(st.utime_ticks, 125);
        assert_eq!(st.stime_ticks, 35);
        assert_eq!(st.cutime_ticks, 40);
        assert_eq!(st.cstime_ticks, 15);
        assert_eq!(st.num_threads, 4);
    }

    #[test]
    fn parses_proc_status_fixture() {
        let fixture = "\
Name:\tsh
State:\tS (sleeping)
VmHWM:\t   4096 kB
VmRSS:\t   2048 kB
Threads:\t3
";
        let st = parse_proc_status(fixture);
        assert_eq!(st.vm_hwm_bytes, Some(4096 * 1024));
        assert_eq!(st.vm_rss_bytes, Some(2048 * 1024));
        assert_eq!(st.threads, Some(3));
    }

    #[test]
    fn rapl_wraparound_arithmetic() {
        assert_eq!(rapl_delta_uj(100, 250, Some(1000)), Some(150));
        assert_eq!(rapl_delta_uj(900, 150, Some(1000)), Some(250));
        assert_eq!(rapl_delta_uj(900, 150, None), None);
    }

    #[test]
    fn converts_clock_ticks_to_ns() {
        assert_eq!(ticks_to_ns(150, 100), 1_500_000_000);
        assert_eq!(ticks_to_ns(25, 250), 100_000_000);
    }

    #[test]
    fn cgroup_snapshot_delta_handles_normal_and_reset() {
        assert_eq!(
            parse_proc_cgroup_v2_path("0::/user.slice/calo.scope\n"),
            Some("/user.slice/calo.scope".to_string())
        );

        let mut early = CgroupSnapshot {
            memory_peak: Some(8_388_608),
            ..Default::default()
        };
        parse_cgroup_cpu_stat(
            "usage_usec 12000\nuser_usec 9000\nsystem_usec 3000\nnr_throttled 2\nthrottled_usec 500\n",
            &mut early,
        );
        parse_cgroup_io_stat("8:0 rbytes=1024 wbytes=2048 rios=1 wios=2\n", &mut early);

        let mut later = CgroupSnapshot {
            memory_peak: Some(16_777_216),
            ..Default::default()
        };
        parse_cgroup_cpu_stat(
            // Simulate a counter reset on `throttled_usec` (200 < 500).
            "usage_usec 20000\nuser_usec 15000\nsystem_usec 5000\nnr_throttled 3\nthrottled_usec 200\n",
            &mut later,
        );
        parse_cgroup_io_stat("8:0 rbytes=4096 wbytes=1024\n", &mut later);

        let delta = early.delta(&later);
        assert_eq!(delta.cpu_usage_us, Some(8_000));
        assert_eq!(delta.cpu_user_us, Some(6_000));
        assert_eq!(delta.cpu_system_us, Some(2_000));
        assert_eq!(delta.nr_throttled, Some(1));
        assert_eq!(delta.throttled_us, None, "counter reset must yield None");
        assert_eq!(delta.io_rbytes, Some(3072));
        assert_eq!(delta.io_wbytes, None, "counter reset must yield None");
    }
}
