//! Passive, read-only hardware discovery (`calo.discovery/1`, Spec §6.3 B-D1–B-D4).

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use calo_schema::{
    common::{Confidence, CpuTier, TiersActive},
    discovery::{
        Discovery, FieldProvenance, ObservedCache, ObservedCpu, ObservedHardware, ObservedMemory,
        ObservedPolicy, UnresolvedField,
    },
    SchemaDoc,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    clock::realtime_rfc3339,
    counters::{CounterAlias, CounterSet},
};

/// Options for [`discover_local`] / `Probe::discover`.
#[derive(Debug, Clone, Default)]
pub struct DiscoverOpts {
    /// Reserved for future deep/calibration hooks (always passive in P8).
    pub deep: bool,
}

/// Runs passive, read-only local discovery and returns a validated `calo.discovery/1` document.
pub fn discover_local(_opts: &DiscoverOpts) -> Discovery {
    let mut fields = BTreeMap::new();
    let mut unresolved = Vec::new();

    let cpu = read_cpu(&mut fields, &mut unresolved);
    let memory = read_memory(&mut fields, &mut unresolved);
    let policy = read_policy(&mut fields, &mut unresolved);

    let kernel = read_trimmed("/proc/sys/kernel/osrelease");
    if kernel.is_some() {
        record_field(&mut fields, "kernel", "/proc/sys/kernel/osrelease", Confidence::Queried);
    }
    let hostname = read_trimmed("/proc/sys/kernel/hostname");
    if hostname.is_some() {
        record_field(&mut fields, "hostname", "/proc/sys/kernel/hostname", Confidence::Queried);
    }
    let distribution = read_os_release();
    if distribution.is_some() {
        record_field(&mut fields, "distribution", "/etc/os-release", Confidence::Queried);
    }

    // Test-open built-in counter aliases on self (pid = 0) without root.
    let counter_set = CounterSet::open(0, CounterAlias::all());
    let counters_available: Vec<String> = counter_set
        .open_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    if counters_available.is_empty() {
        unresolved.push(UnresolvedField {
            field: "observed.counters_available".to_string(),
            fix: "sysctl kernel.perf_event_paranoid=1 (or grant CAP_PERFMON)".to_string(),
        });
    } else {
        record_field(
            &mut fields,
            "counters_available",
            "perf_event_open",
            Confidence::Measured,
        );
    }

    let paranoid = policy.as_ref().and_then(|p| p.perf_event_paranoid);
    let cpu_tier = if paranoid.is_some_and(|p| p <= 2) && !counters_available.is_empty() {
        CpuTier::T2
    } else {
        CpuTier::T0
    };

    let observed = ObservedHardware {
        cpu,
        memory,
        kernel,
        distribution,
        hostname,
        policy,
        counters_available,
        gpus: Vec::new(),
        nic: Vec::new(),
        mpi: None,
    };

    let fingerprint = compute_fingerprint(&observed);

    Discovery {
        schema: Discovery::SCHEMA.to_string(),
        probe_version: env!("CARGO_PKG_VERSION").to_string(),
        at: realtime_rfc3339(),
        fingerprint,
        tiers_supported: Some(TiersActive {
            cpu: Some(cpu_tier),
            mpi: None,
            gpu: None,
        }),
        observed,
        toolchain: None,
        storage: Vec::new(),
        fields,
        unresolved,
    }
}

fn record_field(
    fields: &mut BTreeMap<String, FieldProvenance>,
    key: &str,
    source: &str,
    confidence: Confidence,
) {
    fields.insert(
        key.to_string(),
        FieldProvenance {
            source: source.to_string(),
            confidence,
        },
    );
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_os_release() -> Option<String> {
    let raw = fs::read_to_string("/etc/os-release").ok()?;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}

fn read_cpu(
    fields: &mut BTreeMap<String, FieldProvenance>,
    unresolved: &mut Vec<UnresolvedField>,
) -> Option<ObservedCpu> {
    let Ok(raw) = fs::read_to_string("/proc/cpuinfo") else {
        unresolved.push(UnresolvedField {
            field: "observed.cpu".to_string(),
            fix: "ensure /proc/cpuinfo is readable".to_string(),
        });
        return None;
    };

    let mut model: Option<String> = None;
    let mut threads = 0u32;
    let mut sockets = BTreeSet::new();
    let mut phys_cores = BTreeSet::new();
    let mut cur_phys: Option<u32> = None;
    let mut cur_core: Option<u32> = None;
    let mut isa_set = BTreeSet::new();

    const INTERESTING_ISA: &[&str] = &[
        "sse4_2", "avx", "avx2", "fma", "avx512f", "avx512dq", "avx512bw", "avx512vl",
        "amx_tile", "sve", "sve2", "asimd", "neon",
    ];

    for line in raw.lines() {
        if line.trim().is_empty() {
            if let (Some(p), Some(c)) = (cur_phys.take(), cur_core.take()) {
                phys_cores.insert((p, c));
            }
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        match key {
            "processor" => threads += 1,
            "model name" | "Model Name" if model.is_none() => model = Some(val.to_string()),
            "physical id" => {
                if let Ok(p) = val.parse::<u32>() {
                    sockets.insert(p);
                    cur_phys = Some(p);
                }
            }
            "core id" => {
                if let Ok(c) = val.parse::<u32>() {
                    cur_core = Some(c);
                }
            }
            "flags" | "Features" if isa_set.is_empty() => {
                for tok in val.split_whitespace() {
                    if INTERESTING_ISA.contains(&tok) {
                        isa_set.insert(tok.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if let (Some(p), Some(c)) = (cur_phys, cur_core) {
        phys_cores.insert((p, c));
    }

    let cores = if !phys_cores.is_empty() {
        Some(phys_cores.len() as u32)
    } else if threads > 0 {
        Some(threads)
    } else {
        None
    };
    let smt = match (cores, threads) {
        (Some(c), t) if t > 0 => Some(t > c),
        _ => None,
    };
    let cache = read_cpu_cache();

    record_field(fields, "cpu", "/proc/cpuinfo", Confidence::Queried);
    if cache.is_some() {
        record_field(
            fields,
            "cpu.cache",
            "/sys/devices/system/cpu/cpu0/cache",
            Confidence::Queried,
        );
    }

    Some(ObservedCpu {
        model,
        microarch: None,
        sockets: (!sockets.is_empty()).then_some(sockets.len() as u32),
        cores,
        threads: (threads > 0).then_some(threads),
        smt,
        isa: isa_set.into_iter().collect(),
        cache,
    })
}

fn read_cpu_cache() -> Option<ObservedCache> {
    let base = Path::new("/sys/devices/system/cpu/cpu0/cache");
    let Ok(entries) = fs::read_dir(base) else {
        return None;
    };
    let mut cache = ObservedCache::default();
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(level) = read_trimmed(&p.join("level").to_string_lossy()) else {
            continue;
        };
        let Some(ty) = read_trimmed(&p.join("type").to_string_lossy()) else {
            continue;
        };
        let Some(size_s) = read_trimmed(&p.join("size").to_string_lossy()) else {
            continue;
        };
        let Some(kb) = parse_size_kb(&size_s) else {
            continue;
        };
        match (level.as_str(), ty.as_str()) {
            ("1", "Data") => cache.l1d_kb = Some(kb),
            ("2", _) => cache.l2_kb = Some(kb),
            ("3", _) => cache.l3_mb = Some(kb / 1024),
            _ => {}
        }
    }
    if cache.l1d_kb.is_some() || cache.l2_kb.is_some() || cache.l3_mb.is_some() {
        Some(cache)
    } else {
        None
    }
}

fn parse_size_kb(raw: &str) -> Option<u64> {
    if let Some(n) = raw.strip_suffix('K') {
        n.parse::<u64>().ok()
    } else if let Some(n) = raw.strip_suffix('M') {
        n.parse::<u64>().ok().map(|v| v * 1024)
    } else {
        raw.parse::<u64>().ok().map(|v| v / 1024)
    }
}

fn read_memory(
    fields: &mut BTreeMap<String, FieldProvenance>,
    unresolved: &mut Vec<UnresolvedField>,
) -> Option<ObservedMemory> {
    let mut total_bytes = None;
    let mut hugepages = None;
    if let Ok(raw) = fs::read_to_string("/proc/meminfo") {
        for line in raw.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                if let Some(kb) = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    total_bytes = Some(kb * 1024);
                }
            } else if let Some(rest) = line.strip_prefix("HugePages_Total:") {
                hugepages = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<u64>().ok());
            }
        }
        record_field(fields, "memory", "/proc/meminfo", Confidence::Queried);
    } else {
        unresolved.push(UnresolvedField {
            field: "observed.memory".to_string(),
            fix: "ensure /proc/meminfo is readable".to_string(),
        });
    }

    let mut node_ids = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/devices/system/node") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id_s) = name.strip_prefix("node") {
                if let Ok(id) = id_s.parse::<u32>() {
                    node_ids.push(id);
                }
            }
        }
    }
    node_ids.sort_unstable();
    let numa_nodes = (!node_ids.is_empty()).then_some(node_ids.len() as u32);
    let mut numa_distance = Vec::new();
    for id in &node_ids {
        let path = format!("/sys/devices/system/node/node{id}/distance");
        if let Some(raw) = read_trimmed(&path) {
            let row: Vec<u32> = raw
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .collect();
            if !row.is_empty() {
                numa_distance.push(row);
            }
        }
    }

    let thp = read_trimmed("/sys/kernel/mm/transparent_hugepage/enabled").and_then(|s| {
        s.split_whitespace()
            .find(|tok| tok.starts_with('[') && tok.ends_with(']'))
            .map(|tok| tok.trim_matches(|c| c == '[' || c == ']').to_string())
    });

    if total_bytes.is_none() && numa_nodes.is_none() {
        return None;
    }

    Some(ObservedMemory {
        total_bytes,
        numa_nodes,
        numa_distance,
        thp,
        hugepages,
        channels: None,
    })
}

fn read_policy(
    fields: &mut BTreeMap<String, FieldProvenance>,
    _unresolved: &mut [UnresolvedField],
) -> Option<ObservedPolicy> {
    let governor = read_trimmed("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");
    let turbo = read_trimmed("/sys/devices/system/cpu/intel_pstate/no_turbo")
        .map(|v| if v == "0" { "on" } else { "off" }.to_string())
        .or_else(|| {
            read_trimmed("/sys/devices/system/cpu/cpufreq/boost")
                .map(|v| if v == "1" { "on" } else { "off" }.to_string())
        });
    let smt = read_trimmed("/sys/devices/system/cpu/smt/control");
    let perf_event_paranoid =
        read_trimmed("/proc/sys/kernel/perf_event_paranoid").and_then(|s| s.parse::<i32>().ok());

    if governor.is_none() && turbo.is_none() && smt.is_none() && perf_event_paranoid.is_none() {
        return None;
    }
    record_field(fields, "policy", "/sys + /proc/sys/kernel", Confidence::Queried);
    Some(ObservedPolicy {
        governor,
        turbo,
        smt,
        perf_event_paranoid,
        isolation: None,
        cgroup_cpus: None,
        cgroup_mem_bytes: None,
    })
}

/// Canonical SHA-256 fingerprint matching `calo_core::discover::fingerprint` (Spec §6.3).
fn compute_fingerprint(obs: &ObservedHardware) -> String {
    #[derive(Serialize)]
    struct StableCpu<'a> {
        model: &'a Option<String>,
        microarch: &'a Option<String>,
        sockets: &'a Option<u32>,
        cores: &'a Option<u32>,
        threads: &'a Option<u32>,
        smt: &'a Option<bool>,
        isa: &'a [String],
        cache: &'a Option<ObservedCache>,
    }
    #[derive(Serialize)]
    struct StableMemory<'a> {
        total_bytes: &'a Option<u64>,
        numa_nodes: &'a Option<u32>,
        numa_distance: &'a [Vec<u32>],
    }
    #[derive(Serialize)]
    struct StableGpu<'a> {
        index: u32,
        vendor: &'a Option<String>,
        model: &'a Option<String>,
        arch: &'a Option<String>,
        sms: &'a Option<u32>,
        mem_bytes: &'a Option<u64>,
        driver: &'a Option<String>,
    }
    #[derive(Serialize)]
    struct StableNic<'a> {
        name: &'a str,
        nic_type: &'a Option<String>,
        rate_gbps: &'a Option<f64>,
    }
    #[derive(Serialize)]
    struct StableMpi<'a> {
        library: &'a Option<String>,
        version: &'a Option<String>,
    }
    #[derive(Serialize)]
    struct StableFingerprintInput<'a> {
        cpu: Option<StableCpu<'a>>,
        memory: Option<StableMemory<'a>>,
        gpus: Vec<StableGpu<'a>>,
        nic: Vec<StableNic<'a>>,
        kernel: &'a Option<String>,
        mpi: Option<StableMpi<'a>>,
    }

    let payload = StableFingerprintInput {
        cpu: obs.cpu.as_ref().map(|c| StableCpu {
            model: &c.model,
            microarch: &c.microarch,
            sockets: &c.sockets,
            cores: &c.cores,
            threads: &c.threads,
            smt: &c.smt,
            isa: &c.isa,
            cache: &c.cache,
        }),
        memory: obs.memory.as_ref().map(|m| StableMemory {
            total_bytes: &m.total_bytes,
            numa_nodes: &m.numa_nodes,
            numa_distance: &m.numa_distance,
        }),
        gpus: obs
            .gpus
            .iter()
            .map(|g| StableGpu {
                index: g.index,
                vendor: &g.vendor,
                model: &g.model,
                arch: &g.arch,
                sms: &g.sms,
                mem_bytes: &g.mem_bytes,
                driver: &g.driver,
            })
            .collect(),
        nic: obs
            .nic
            .iter()
            .map(|n| StableNic {
                name: &n.name,
                nic_type: &n.nic_type,
                rate_gbps: &n.rate_gbps,
            })
            .collect(),
        kernel: &obs.kernel,
        mpi: obs.mpi.as_ref().map(|m| StableMpi {
            library: &m.library,
            version: &m.version,
        }),
    };

    let canonical_yaml = calo_schema::yaml::to_string(&payload).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical_yaml.as_bytes());
    format!("{:x}", hasher.finalize())
}
