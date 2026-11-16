//! Machine discovery output schema (`calo.discovery/1`, spec §6.3, §10).

use crate::common::{Confidence, TiersActive};
use crate::error::SchemaError;
use crate::SchemaDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Cache hierarchy sizes observed on a CPU node (spec §6.3, §7, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservedCache {
    /// L1 data cache size in KiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l1d_kb: Option<u64>,
    /// L2 cache size in KiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l2_kb: Option<u64>,
    /// L3 cache size in MiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l3_mb: Option<u64>,
}

/// Observed CPU topology and ISA features (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservedCpu {
    /// Processor model name string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Microarchitecture codename (e.g. `"zen4"`, `"sapphirerapids"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microarch: Option<String>,
    /// Number of physical sockets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sockets: Option<u32>,
    /// Total physical cores across sockets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
    /// Total logical hardware threads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    /// Simultaneous multithreading factor or state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smt: Option<bool>,
    /// Supported instruction set extensions (e.g. `["avx2", "avx512"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub isa: Vec<String>,
    /// Cache sizes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<ObservedCache>,
}

/// Observed system memory and NUMA layout (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservedMemory {
    /// Total physical RAM in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    /// Number of NUMA nodes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numa_nodes: Option<u32>,
    /// NUMA distance matrix between nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numa_distance: Vec<Vec<u32>>,
    /// Transparent hugepages policy string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thp: Option<String>,
    /// Number of allocated static hugepages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hugepages: Option<u64>,
    /// Number of populated memory channels (best effort).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
}

/// Observed OS and power policy state (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservedPolicy {
    /// CPU frequency scaling governor (e.g. `"performance"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub governor: Option<String>,
    /// Turbo boost state (`"on"`, `"off"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turbo: Option<String>,
    /// SMT control state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smt: Option<String>,
    /// Kernel `/proc/sys/kernel/perf_event_paranoid` value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perf_event_paranoid: Option<i32>,
    /// Kernel isolcpus / nohz_full isolation string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    /// Cgroup allowed CPUs string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroup_cpus: Option<String>,
    /// Cgroup memory limit in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroup_mem_bytes: Option<u64>,
}

/// Observed GPU clock frequencies (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservedGpuClocks {
    /// Streaming multiprocessor / compute unit clock in MHz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sm_mhz: Option<u32>,
    /// Device memory clock in MHz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_mhz: Option<u32>,
}

/// Observed individual GPU device descriptor (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedGpu {
    /// Device ordinal index on the node.
    pub index: u32,
    /// GPU vendor (`"nvidia"`, `"amd"`, `"intel"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Device model name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Device architecture target (e.g. `"sm_80"`, `"gfx90a"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Number of SMs / CUs / Xe-cores.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sms: Option<u32>,
    /// Total device memory in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_bytes: Option<u64>,
    /// Kernel driver version string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Compute runtime version string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Whether ECC memory is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecc: Option<bool>,
    /// Observed GPU clocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clocks: Option<ObservedGpuClocks>,
    /// Interconnect links to peer devices or host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<String>,
    /// Nearest host NUMA node id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numa: Option<i32>,
    /// Whether hardware profiling counters are accessible by the current user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiling_permitted: Option<bool>,
}

/// Observed network interface descriptor (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedNic {
    /// Interface or HCA name (e.g. `"mlx5_0"`).
    pub name: String,
    /// Interconnect type (`"infiniband"`, `"slingshot"`, `"ethernet"`).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub nic_type: Option<String>,
    /// Link rate in Gbps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_gbps: Option<f64>,
    /// Nearest NUMA node id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numa: Option<i32>,
    /// Port state string (e.g. `"ACTIVE"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_state: Option<String>,
}

/// Observed MPI installation metadata (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservedMpi {
    /// MPI implementation name (e.g. `"openmpi"`, `"mpich"`, `"cray-mpich"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    /// MPI library version string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Discovered launcher binary or command template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    /// Provided MPI thread level in run context or supported levels in discovery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_level_provided: Option<String>,
    /// Whether GPU-aware MPI support was detected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_aware: Option<bool>,
}

/// Observed hardware and system state shared between `calo.discovery/1` and `calo.record/2` (spec §6.3, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ObservedHardware {
    /// CPU topology and ISA details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<ObservedCpu>,
    /// Memory and NUMA topology details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<ObservedMemory>,
    /// OS kernel version (`uname -r`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// OS distribution description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distribution: Option<String>,
    /// Hostname of the observed node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Power, frequency, and isolation policy state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<ObservedPolicy>,
    /// Hardware performance counter aliases verified via `perf_event_open`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counters_available: Vec<String>,
    /// Observed GPU devices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpus: Vec<ObservedGpu>,
    /// Observed high-speed and ethernet NICs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nic: Vec<ObservedNic>,
    /// Observed MPI runtime properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<ObservedMpi>,
}

/// Provenance entry for a single discovered field (spec §6.3 B-D3, §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldProvenance {
    /// Source interface or command (e.g. `"/proc/cpuinfo"`, `"nvml"`).
    pub source: String,
    /// Confidence level (`measured`, `queried`, `inferred`, `unavailable`).
    pub confidence: Confidence,
}

/// Unresolved discovery field with suggested remediation (spec §6.3 B-D4, §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedField {
    /// Dotted field path that could not be resolved.
    pub field: String,
    /// Suggested fix (e.g. `"sysctl kernel.perf_event_paranoid=1"`).
    pub fix: String,
}

/// Discovered toolchain entry (spec §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DiscoveredToolchain {
    /// Discovered compilers (e.g. `["gcc 13.2.0", "clang 17.0.6"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compilers: Vec<String>,
    /// CMake version if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmake: Option<String>,
    /// CUDA toolkit version if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cuda: Option<String>,
    /// ROCm toolkit version if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rocm: Option<String>,
    /// Environment modules providing the toolchain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

/// Candidate workdir mount discovered from `/proc/mounts` and `df` (spec §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredMount {
    /// Mount path.
    pub path: String,
    /// Filesystem type (e.g. `"lustre"`, `"gpfs"`, `"ext4"`).
    pub fstype: String,
    /// Available space in bytes.
    pub free_bytes: u64,
}

/// Output of `probe --discover` (`calo.discovery/1`, spec §6.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Discovery {
    /// Schema header (`"calo.discovery/1"`).
    pub schema: String,
    /// Probe version string.
    pub probe_version: String,
    /// ISO 8601 timestamp when discovery ran.
    pub at: String,
    /// Canonical SHA-256 fingerprint of stable hardware fields (spec §6.3).
    pub fingerprint: String,
    /// Highest supported capability tiers on this node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiers_supported: Option<TiersActive>,
    /// Observed hardware, policy, counters, GPUs, NICs, and MPI.
    pub observed: ObservedHardware,
    /// Discovered compilers, toolkits, and modules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<DiscoveredToolchain>,
    /// Candidate storage mounts for `workdir`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub storage: Vec<DiscoveredMount>,
    /// Per-field source and confidence provenance map.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, FieldProvenance>,
    /// Fields that could not be resolved along with suggested fixes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<UnresolvedField>,
}

impl SchemaDoc for Discovery {
    const SCHEMA: &'static str = "calo.discovery/1";

    fn schema_str(&self) -> &str {
        &self.schema
    }

    fn check(&self) -> Result<(), SchemaError> {
        if self.schema != Self::SCHEMA {
            return Err(SchemaError::VersionMismatch {
                expected: Self::SCHEMA.to_string(),
                found: self.schema.clone(),
            });
        }
        if self.fingerprint.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "fingerprint".to_string(),
            });
        }
        Ok(())
    }
}
