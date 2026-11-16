//! Machine inventory schema (`calo.inventory/1`, spec §7).

use crate::discovery::{FieldProvenance, UnresolvedField};
use crate::error::SchemaError;
use crate::SchemaDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Connection method for reaching a machine (spec §6.1, §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectType {
    /// Local host execution.
    Local,
    /// Remote execution over SSH.
    Ssh,
    /// Slurm cluster login node.
    Slurm,
}

/// Connection configuration under `machines.<name>.connect` (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineConnect {
    /// Connection adapter type (`local`, `ssh`, `slurm`).
    #[serde(rename = "type")]
    pub connect_type: ConnectType,
    /// Remote hostname or login node address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// SSH username.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Allowed Slurm partitions (spec §6.2 R-SPC-1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partitions: Vec<String>,
    /// Scheduler account string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// CPU description in an inventory node or node class (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InventoryCpu {
    /// CPU model string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Microarchitecture name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microarch: Option<String>,
    /// Socket count per node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sockets: Option<u32>,
    /// Physical core count per node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
    /// Hardware thread count per node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    /// Whether SMT is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smt: Option<bool>,
    /// Supported ISA flags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub isa: Vec<String>,
}

/// Cache sizes in an inventory node (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InventoryCache {
    /// L2 cache in KiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l2_kb: Option<u64>,
    /// L3 cache in MiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l3_mb: Option<u64>,
}

/// Memory configuration in an inventory node (spec §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InventoryMemory {
    /// Total memory in GiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gb: Option<f64>,
    /// Memory channel count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    /// NUMA node count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numa_nodes: Option<u32>,
}

/// GPU interconnect link type in inventory (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuLinkType {
    /// NVIDIA NVLink interconnect.
    Nvlink,
    /// PCIe interconnect.
    Pcie,
    /// AMD xGMI Infinity Fabric interconnect.
    Xgmi,
    /// No multi-GPU interconnect.
    None,
}

/// GPU descriptor in an inventory node (spec §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InventoryGpu {
    /// GPU vendor (`"nvidia"`, `"amd"`, `"intel"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// GPU model name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// GPU architecture code (e.g. `"sm_80"`, `"gfx90a"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Number of GPUs per node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_node: Option<u32>,
    /// Memory per GPU in GiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_gb: Option<f64>,
    /// Interconnect link technology.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<GpuLinkType>,
}

/// Node hardware summary in `inventory.yaml` (spec §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InventoryNode {
    /// CPU specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<InventoryCpu>,
    /// Cache specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<InventoryCache>,
    /// System memory specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<InventoryMemory>,
    /// GPU specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<InventoryGpu>,
}

/// Homogeneous node class within a cluster machine (spec §6.5, §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeClass {
    /// Number of nodes in this class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Partition name associated with this class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<String>,
    /// Scheduler node features/constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Hardware specification for nodes in this class.
    pub node: InventoryNode,
}

/// Interconnect fabric type (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkType {
    /// InfiniBand fabric.
    Infiniband,
    /// HPE Cray Slingshot fabric.
    Slingshot,
    /// Ethernet fabric.
    Ethernet,
}

/// Machine network description (spec §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryNetwork {
    /// Network fabric technology.
    #[serde(rename = "type")]
    pub net_type: NetworkType,
    /// Unidirectional injection bandwidth in Gbps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bw_gbps: Option<f64>,
    /// Network topology description (e.g. `"fat-tree"`, `"dragonfly"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topology: Option<String>,
}

/// Calibrated CPU reference capabilities (spec §6.5, §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReferenceCpu {
    /// Sustained CPU memory bandwidth in GB/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_bw_gbs: Option<f64>,
    /// Peak CPU throughput in GFLOP/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_gflops: Option<f64>,
}

/// Calibrated GPU reference capabilities (spec §6.5, §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReferenceGpu {
    /// Sustained device memory bandwidth in GB/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_bw_gbs: Option<f64>,
    /// Peak FP64 throughput in GFLOP/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_gflops_fp64: Option<f64>,
    /// Peak FP32 throughput in GFLOP/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_gflops_fp32: Option<f64>,
}

/// Calibrated MPI reference latencies and bandwidth (spec §6.5, §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReferenceMpi {
    /// Point-to-point latency in microseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p2p_latency_us: Option<f64>,
    /// Point-to-point bandwidth in GB/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p2p_bw_gbs: Option<f64>,
    /// Allreduce collective latency in microseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allreduce_latency_us: Option<f64>,
}

/// Provenance metadata for calibrated reference values (spec §6.4, §6.5, §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceMeasured {
    /// Version of `probe` used for calibration.
    pub probe: String,
    /// Machine fingerprint at the time of calibration.
    pub fingerprint: String,
    /// ISO 8601 timestamp when calibration ran.
    pub at: String,
    /// Set to `true` when the machine's fingerprint has changed since calibration (spec §6.4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
}

/// Calibrated roofline and interconnect reference data (spec §6.5, §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InventoryReference {
    /// CPU memory bandwidth and compute peak.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<ReferenceCpu>,
    /// GPU memory bandwidth and compute peaks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<ReferenceGpu>,
    /// MPI latency and bandwidth bounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<ReferenceMpi>,
    /// Host-to-device transfer bandwidth in GB/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_device_bw_gbs: Option<f64>,
    /// Calibration provenance and staleness state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured: Option<ReferenceMeasured>,
}

/// CPU binding mode for launcher (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuBindMode {
    /// Bind ranks to physical cores.
    Core,
    /// Bind ranks to NUMA domains.
    Numa,
    /// No launcher CPU binding.
    None,
}

/// GPU binding mode for launcher (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuBindMode {
    /// Map GPU ordinal by `local_rank`.
    LocalRank,
    /// Explicit rank-to-GPU mapping.
    Explicit,
    /// No GPU binding.
    None,
}

/// Launcher CPU and GPU binding configuration (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LauncherBind {
    /// CPU binding strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuBindMode>,
    /// GPU binding strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuBindMode>,
}

/// MPI launcher configuration in `inventory.yaml` (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InventoryLauncher {
    /// MPI launch command template (e.g. `"srun -n {ranks} --ntasks-per-node {ranks_per_node}"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<String>,
    /// CPU and GPU rank binding defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind: Option<LauncherBind>,
    /// Whether the MPI stack supports direct GPU buffer pointers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_aware_mpi: Option<bool>,
}

/// Power measurement backend source (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerSource {
    /// Intel/AMD RAPL energy counters.
    Rapl,
    /// NVIDIA Management Library.
    Nvml,
    /// AMD ROCm SMI library.
    RocmSmi,
    /// No power instrumentation.
    None,
}

/// Machine-specific `probe` overrides in `inventory.yaml` (spec §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InventoryProbe {
    /// Machine-specific counter alias to raw vendor event overrides (spec §7 R-INV-4).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counters: BTreeMap<String, String>,
    /// Power sensor source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerSource>,
    /// Maximum allowed sampling frequency in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_sample_hz: Option<u32>,
}

/// CPU isolation list in `policy` (spec §3 I2, §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyIsolate {
    /// CPU list reserved or isolated (e.g. `"0-3"`).
    pub cpu_list: String,
}

/// Requested GPU clock policy (spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuClockPolicy {
    /// Clocks locked to deterministic base/boost frequency.
    Locked,
    /// Default dynamic GPU clocks.
    Default,
}

/// Requested machine execution policy (spec §7 R-INV-3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InventoryPolicy {
    /// Launcher/OS CPU isolation configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolate: Option<PolicyIsolate>,
    /// Requested cpufreq governor (e.g. `"performance"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub governor: Option<String>,
    /// Requested turbo state (e.g. `"off"`, `"on"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turbo: Option<String>,
    /// Requested GPU clock mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_clocks: Option<GpuClockPolicy>,
}

/// Discovery provenance metadata recorded in `inventory.yaml` (spec §6.3, §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryDiscoveredMeta {
    /// ISO 8601 timestamp when discovery ran.
    pub at: String,
    /// `calo` version that wrote the discovery data.
    pub calo: String,
    /// Provenance per discovered field path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, FieldProvenance>,
    /// Unresolved fields and suggested fixes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<UnresolvedField>,
}

/// Machine entry in `inventory.yaml` (spec §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineEntry {
    /// How `calo` connects to this machine.
    pub connect: MachineConnect,
    /// Derived SHA-256 hardware fingerprint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// User-assigned machine tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Named homogeneous node classes (may be omitted on single-node machines).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_classes: BTreeMap<String, NodeClass>,
    /// Direct node specification for single-node machines (spec §7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<InventoryNode>,
    /// Interconnect fabric details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<InventoryNetwork>,
    /// Calibrated reference values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<InventoryReference>,
    /// Remote or local scratch working directory path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Environment modules to load by default on this machine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// Default environment variables for this machine.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// MPI launcher settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launcher: Option<InventoryLauncher>,
    /// Probe counter/power/sampling settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<InventoryProbe>,
    /// Requested system policy state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<InventoryPolicy>,
    /// Discovery provenance block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered: Option<InventoryDiscoveredMeta>,
}

/// Top-level `inventory.yaml` document (`calo.inventory/1`, spec §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    /// Schema header (`"calo.inventory/1"`).
    pub schema: String,
    /// Map of machine names to their configurations.
    pub machines: BTreeMap<String, MachineEntry>,
}

impl SchemaDoc for Inventory {
    const SCHEMA: &'static str = "calo.inventory/1";

    fn schema_str(&self) -> &str {
        &self.schema
    }
}
