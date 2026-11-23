//! Host time-series sampler (`series.host`) reading `/proc` and RAPL powercap (Spec §9.3 B2/B4, §10).
//!
//! Uses only `/proc` and `/sys/class/powercap` — never `perf_event_open` counter reads.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    clock::now_ns,
    scalars::{
        discover_rapl_domains_in, rapl_delta_uj, read_proc_stat_in, read_proc_status_in,
        ticks_to_ns, RaplDomain, RaplDomainKind, RaplEnergyJ,
    },
    sys::clock::clock_ticks_per_sec,
};

const MAX_TREE_PIDS: usize = 4096;

/// Single row of the `series.host` table (Spec §10, `calo-schema::tables::series.host`).
///
/// Power columns are `f32::NAN` when unmeasured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostSeriesRow {
    pub t_ns: i64,
    pub rss_bytes: i64,
    pub utime_ns: i64,
    pub stime_ns: i64,
    pub power_pkg_w: f32,
    pub power_dram_w: f32,
}

/// Periodic host-level sampler for a child process tree (Spec §9.3 B2/B4).
pub struct HostSampler {
    root_pid: libc::pid_t,
    proc_root: PathBuf,
    powercap_root: PathBuf,
    clk_tck: u64,
    peak_rss_bytes: Option<u64>,
    prev_t_ns: Option<i64>,
    rapl_domains: BTreeMap<PathBuf, RaplDomain>,
    notes: Vec<String>,
    noted_children_absent: bool,
    noted_tree_truncated: bool,
    last_rss_bytes: Option<u64>,
}

impl HostSampler {
    /// Creates a new [`HostSampler`] targeting `root_pid` using `/proc` and `/sys/class/powercap`.
    pub fn new(root_pid: libc::pid_t) -> Self {
        Self::new_with_roots(
            root_pid,
            PathBuf::from("/proc"),
            PathBuf::from("/sys/class/powercap"),
        )
    }

    /// Creates a new [`HostSampler`] with injectable `proc_root` and `powercap_root` for testing.
    pub fn new_with_roots(
        root_pid: libc::pid_t,
        proc_root: PathBuf,
        powercap_root: PathBuf,
    ) -> Self {
        let (rapl_domains, notes) = discover_rapl_domains_in(&powercap_root);
        Self {
            root_pid,
            proc_root,
            powercap_root,
            clk_tck: clock_ticks_per_sec(),
            peak_rss_bytes: None,
            prev_t_ns: None,
            rapl_domains,
            notes,
            noted_children_absent: false,
            noted_tree_truncated: false,
            last_rss_bytes: None
        }
    }

    /// Re-reads RAPL baselines and resets the power interval timestamp.
    /// Call immediately after `Child::go()`.
    pub fn rebase(&mut self) {
        let (domains, notes) = discover_rapl_domains_in(&self.powercap_root);
        self.rapl_domains = domains;
        self.prev_t_ns = None;
        for note in notes {
            if !self.notes.contains(&note) {
                self.notes.push(note);
            }
        }
    }

    /// Returns diagnostic notes collected during sampling (e.g. RAPL unreadable with errno,
    /// `/proc/<pid>/task/*/children` absent fallback, or process-tree truncation).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Collects one `series.host` row at the current `CLOCK_MONOTONIC_RAW` instant.
    ///
    /// Returns `None` when `/proc` is unreadable for the whole process tree (e.g. after exit).
    pub fn sample(&mut self) -> Option<HostSeriesRow> {
        let t_ns = now_ns();
        let tree = collect_process_tree_in(&self.proc_root, self.root_pid);
        if tree.children_absent && !self.noted_children_absent {
            self.noted_children_absent = true;
            self.notes.push(format!(
                "/proc/{}/task/*/children absent; fell back to root PID only",
                self.root_pid
            ));
        }
        if tree.truncated && !self.noted_tree_truncated {
            self.noted_tree_truncated = true;
            self.notes.push(format!(
                "process tree walk truncated at {MAX_TREE_PIDS} PIDs"
            ));
        }

        let mut total_rss: u64 = 0;
        let mut any_rss = false;
        let mut total_utime_ticks: u64 = 0;
        let mut total_stime_ticks: u64 = 0;
        let mut any_stat = false;

        for pid in tree.pids {
            if let Some(status) = read_proc_status_in(&self.proc_root, pid) {
                if let Some(rss) = status.vm_rss_bytes {
                    total_rss = total_rss.saturating_add(rss);
                    any_rss = true;
                }
            }
            if let Some(stat) = read_proc_stat_in(&self.proc_root, pid) {
                total_utime_ticks = total_utime_ticks
                    .saturating_add(stat.utime_ticks)
                    .saturating_add(stat.cutime_ticks);
                total_stime_ticks = total_stime_ticks
                    .saturating_add(stat.stime_ticks)
                    .saturating_add(stat.cstime_ticks);
                any_stat = true;
            }
        }

        if !any_rss && !any_stat {
            return None;
        }

        let rss_bytes = if any_rss {
            self.last_rss_bytes = Some(total_rss);
            self.peak_rss_bytes = Some(self.peak_rss_bytes.unwrap_or(0).max(total_rss));
            total_rss.min(i64::MAX as u64) as i64
        } else if let Some(last_rss) = self.last_rss_bytes {
            if !self.notes.iter().any(|n| n == "rss_carried_forward") {
                self.notes.push("rss_carried_forward".to_string());
            }
            last_rss.min(i64::MAX as u64) as i64
        } else {
            0
        };

        let (utime_ns, stime_ns) = if any_stat {
            (
                ticks_to_ns(total_utime_ticks, self.clk_tck).min(i64::MAX as u64) as i64,
                ticks_to_ns(total_stime_ticks, self.clk_tck).min(i64::MAX as u64) as i64,
            )
        } else {
            (0, 0)
        };

        let dt_ns = self
            .prev_t_ns
            .map(|prev| t_ns.saturating_sub(prev))
            .filter(|&dt| dt > 0);
        self.prev_t_ns = Some(t_ns);

        let (power_pkg_w, power_dram_w) = self.sample_rapl_power(dt_ns);

        Some(HostSeriesRow {
            t_ns,
            rss_bytes,
            utime_ns,
            stime_ns,
            power_pkg_w,
            power_dram_w,
        })
    }

    fn sample_rapl_power(&mut self, dt_ns: Option<i64>) -> (f32, f32) {
        if self.rapl_domains.is_empty() {
            return (f32::NAN, f32::NAN);
        }

        let mut pkg_uj: u64 = 0;
        let mut any_pkg = false;
        let mut dram_uj: u64 = 0;
        let mut any_dram = false;

        for dom in self.rapl_domains.values_mut() {
            let Ok(raw) = fs::read_to_string(&dom.energy_path) else {
                continue;
            };
            let Ok(curr) = raw.trim().parse::<u64>() else {
                continue;
            };
            if let Some(delta) = rapl_delta_uj(dom.prev_uj, curr, dom.max_range_uj) {
                dom.total_uj = dom.total_uj.saturating_add(delta);
                match dom.kind {
                    RaplDomainKind::Pkg => {
                        pkg_uj = pkg_uj.saturating_add(delta);
                        any_pkg = true;
                    }
                    RaplDomainKind::Dram => {
                        dram_uj = dram_uj.saturating_add(delta);
                        any_dram = true;
                    }
                }
            }
            dom.prev_uj = curr;
        }

        let Some(dt) = dt_ns else {
            return (f32::NAN, f32::NAN);
        };
        let dt_f = dt as f64;
        let pkg_w = if any_pkg {
            ((pkg_uj as f64 * 1_000.0) / dt_f) as f32
        } else {
            f32::NAN
        };
        let dram_w = if any_dram {
            ((dram_uj as f64 * 1_000.0) / dt_f) as f32
        } else {
            f32::NAN
        };
        (pkg_w, dram_w)
    }

    /// Returns the peak RSS (bytes) observed across all sampled ticks.
    pub fn peak_rss_bytes(&self) -> Option<u64> {
        self.peak_rss_bytes
    }

    /// Performs a final RAPL read and returns total wrap-aware energy in Joules over the run.
    pub fn finish_energy_j(&mut self) -> RaplEnergyJ {
        let _ = self.sample_rapl_power(None);
        let mut pkg_uj: u64 = 0;
        let mut any_pkg = false;
        let mut dram_uj: u64 = 0;
        let mut any_dram = false;

        for dom in self.rapl_domains.values() {
            match dom.kind {
                RaplDomainKind::Pkg => {
                    pkg_uj = pkg_uj.saturating_add(dom.total_uj);
                    any_pkg = true;
                }
                RaplDomainKind::Dram => {
                    dram_uj = dram_uj.saturating_add(dom.total_uj);
                    any_dram = true;
                }
            }
        }

        RaplEnergyJ {
            pkg: any_pkg.then_some(pkg_uj as f64 * 1e-6),
            dram: any_dram.then_some(dram_uj as f64 * 1e-6),
        }
    }
}

struct TreeWalk {
    pids: Vec<libc::pid_t>,
    children_absent: bool,
    truncated: bool,
}

fn collect_process_tree_in(proc_root: &Path, root_pid: libc::pid_t) -> TreeWalk {
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    visited.insert(root_pid);
    queue.push_back(root_pid);

    let mut children_absent = false;
    let mut truncated = false;

    while let Some(pid) = queue.pop_front() {
        let pid_dir = proc_root.join(pid.to_string());
        if !pid_dir.exists() {
            continue;
        }
        let task_dir = pid_dir.join("task");
        let Ok(tids) = fs::read_dir(&task_dir) else {
            if pid == root_pid {
                children_absent = true;
            }
            continue;
        };
        let mut saw_children_file = false;
        for tid_entry in tids.flatten() {
            let children_path = tid_entry.path().join("children");
            let Ok(raw) = fs::read_to_string(children_path) else {
                continue;
            };
            saw_children_file = true;
            for tok in raw.split_whitespace() {
                if let Ok(child_pid) = tok.parse::<libc::pid_t>() {
                    if visited.len() >= MAX_TREE_PIDS {
                        truncated = true;
                        break;
                    }
                    if visited.insert(child_pid) {
                        queue.push_back(child_pid);
                    }
                }
            }
            if truncated {
                break;
            }
        }
        if pid == root_pid && !saw_children_file {
            children_absent = true;
        }
        if truncated {
            break;
        }
    }

    TreeWalk {
        pids: visited.into_iter().collect(),
        children_absent,
        truncated,
    }
}

/// Walks `/proc/<pid>/task/*/children` breadth-first starting from `root_pid`.
pub fn collect_process_tree(root_pid: libc::pid_t) -> Vec<libc::pid_t> {
    collect_process_tree_in(Path::new("/proc"), root_pid).pids
}
