//! Canonical SHA-256 hardware fingerprinting over stable `calo.discovery/1` fields (spec §6.3).

use calo_schema::discovery::{Discovery, ObservedCache};
use serde::Serialize;
use sha2::{Digest, Sha256};

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

/// Computes the canonical SHA-256 hardware fingerprint over the stable fields of `disc` (spec §6.3).
///
/// Volatile state (governor, clocks, free space, hostname, policy) is strictly excluded.
pub fn fingerprint(disc: &Discovery) -> String {
    let obs = &disc.observed;
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

/// Difference report produced by `calo verify` when comparing an inventory machine to a fresh `Discovery` (spec §7 R-INV-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintDiff {
    /// Expected fingerprint stored in `inventory.yaml`.
    pub expected: Option<String>,
    /// Newly computed fingerprint from `Discovery`.
    pub observed: String,
    /// `true` if `expected == Some(observed)`.
    pub matches: bool,
}

/// Compares `expected_fp` from `inventory.yaml` against `fingerprint(discovered)` (spec §7 R-INV-5, §14).
pub fn verify_fingerprint_diff(
    expected_fp: Option<&str>,
    discovered: &Discovery,
) -> FingerprintDiff {
    let observed = fingerprint(discovered);
    let matches = expected_fp == Some(observed.as_str());
    FingerprintDiff {
        expected: expected_fp.map(str::to_string),
        observed,
        matches,
    }
}
