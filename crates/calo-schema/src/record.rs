//! Run record schema (`calo.record/2`, spec §10).

use crate::common::{
    CallgraphMethod, CpuTier, GpuTier, Mode, MpiTier, ScalarValue, Status, TiersActive, Warning,
};
use crate::discovery::ObservedHardware;
use crate::error::SchemaError;
use crate::runspec::GpuTraceSource;
use crate::table::Sidecar;
use crate::SchemaDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Record identity block (`identity:` in `calo.record/2`, spec §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordIdentity {
    /// Deterministic run identifier.
    pub run_id: String,
    /// Benchmark name.
    pub benchmark: String,
    /// Optional benchmark group name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Benchmark execution mode (`measure`, `profile`, `kernel`).
    pub mode: Mode,
    /// Pinned scalar parameter values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ScalarValue>,
    /// Repetition index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// Whether this run was a warmup repetition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warmup: Option<bool>,
    /// MPI rank index (absent in aggregate `run.yaml`, spec §10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    /// Total MPI ranks in the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ranks: Option<u32>,
    /// Node hostname where this rank executed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Node-local MPI rank index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_rank: Option<u32>,
    /// Hex-encoded suite hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suite_hash: Option<String>,
    /// Linked run IDs (e.g. linked `measure` and `profile` runs, spec §8.5 R-BEN-4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_runs: Vec<String>,
    /// Run completion status (`ok`, `failed`, `timeout`, `invalid`).
    pub status: Status,
}

/// Rank resource placement recorded in `environment.placement` (spec §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecordPlacement {
    /// Allowed/bound Linux CPU list string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_list: Option<String>,
    /// Bound NUMA node indices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numa: Vec<u32>,
    /// Number of threads per rank (`OMP_NUM_THREADS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    /// Bound GPU device ordinals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpus: Vec<u32>,
    /// Bound NIC name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nic: Option<String>,
}

/// Device ownership entry in `environment.devices` (spec §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceOwners {
    /// Device index on the node.
    pub index: u32,
    /// Ranks bound to this device.
    pub owners: Vec<u32>,
}

/// Individual applied patch hash entry in `environment.patchset` (spec §8.2 R-PAT-3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedPatch {
    /// Patch filename.
    pub file: String,
    /// Hex-encoded SHA-256 hash of the patch file.
    pub sha256: String,
}

/// Patchset provenance in `environment.patchset` (spec §8.2 R-PAT-3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordPatchset {
    /// Patch set name.
    pub name: String,
    /// Ordered list of applied patches and their SHA-256 digests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<AppliedPatch>,
    /// Resulting git tree hash after applying patches.
    pub tree_hash: String,
}

/// Build provenance in `environment.build` (spec §8.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordBuild {
    /// Build variant name.
    pub name: String,
    /// Hex-encoded build cache key.
    pub cache_key: String,
    /// Compiler flags string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    /// Accelerator backend (`cuda`, `hip`, `sycl`, `none`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Target GPU architectures (e.g. `["sm_80"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_arch: Vec<String>,
    /// Compiler toolchain identity string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
}

/// Rendered template file entry in `environment.rendered_files` (spec §8.4 R-STP-3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedFile {
    /// Relative path to rendered file.
    pub path: String,
    /// Hex-encoded SHA-256 digest of rendered content.
    pub sha256: String,
}

/// MPI library identity in `environment.mpi_lib` (spec §8.3 R-BLD-5, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordMpiLib {
    /// MPI library name.
    pub name: String,
    /// MPI library version string.
    pub version: String,
}

/// Environment block in `calo.record/2` (`environment:`, spec §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordEnvironment {
    /// Machine name from inventory.
    pub machine: String,
    /// Node class name if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_class: Option<String>,
    /// Observed node hardware fingerprint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Declared hardware snapshot from `inventory.yaml`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub declared: BTreeMap<String, crate::yaml::Value>,
    /// Hardware and policy observed at runtime by `probe` (spec §7 R-INV-2, §10).
    pub observed: ObservedHardware,
    /// Rank CPU, NUMA, thread, GPU, and NIC placement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<RecordPlacement>,
    /// Node GPU device ownership map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<DeviceOwners>,
    /// Source repository commit SHAs (`<source>: sha`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub revisions: BTreeMap<String, String>,
    /// Applied patch set details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patchset: Option<RecordPatchset>,
    /// Build configuration and cache key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<RecordBuild>,
    /// SHA-256 hashes of built binary artifacts (`<path>: sha256`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifact_hashes: BTreeMap<String, String>,
    /// Rendered input files and their SHA-256 digests.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rendered_files: BTreeMap<String, RenderedFile>,
    /// Linked MPI library name and version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi_lib: Option<RecordMpiLib>,
    /// Loaded environment modules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

/// Energy consumption measurements in Joules (`energy_j`, spec §0.3, §10).
/// Unmeasured domains MUST be `None` (omitted); a measured 0.0 is `Some(0.0)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EnergyJoules {
    /// CPU package energy in Joules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pkg: Option<f64>,
    /// DRAM energy in Joules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dram: Option<f64>,
    /// GPU energy in Joules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<f64>,
}

impl EnergyJoules {
    /// Returns true if all energy sub-fields are `None`.
    pub fn is_empty(&self) -> bool {
        self.pkg.is_none() && self.dram.is_none() && self.gpu.is_none()
    }
}

/// Hardware performance counter measurement (`scalars.counters.<alias>`, spec §9.3 B3, §10 R-REC-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counter {
    /// Raw counter value.
    pub value: u64,
    /// Time the counter was enabled in nanoseconds.
    pub time_enabled_ns: u64,
    /// Time the counter was actively running on hardware in nanoseconds.
    pub time_running_ns: u64,
    /// Whether the value has been scaled for multiplexing.
    pub scaled: bool,
}

/// End-of-run scalar measurements (`observations.scalars`, T0, spec §9.3 B2, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecordScalars {
    /// Elapsed wall-clock time in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_s: Option<f64>,
    /// User CPU time in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_cpu_s: Option<f64>,
    /// System CPU time in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sys_cpu_s: Option<f64>,
    /// Peak resident set size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    /// Minor (soft) page faults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minor_faults: Option<u64>,
    /// Major (hard) page faults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub major_faults: Option<u64>,
    /// Voluntary context switches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voluntary_ctx: Option<u64>,
    /// Involuntary context switches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub involuntary_ctx: Option<u64>,
    /// Block input operations in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_in_bytes: Option<u64>,
    /// Block output operations in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_out_bytes: Option<u64>,
    /// Energy measurements in Joules (`pkg`, `dram`, `gpu`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy_j: Option<EnergyJoules>,
    /// Hardware performance counters keyed by alias.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counters: BTreeMap<String, Counter>,
}

/// Timestamp span for a workload phase (`observations.phases.<name>`, spec §9.4, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseSpan {
    /// Monotonic start timestamp in nanoseconds.
    pub start_ns: i64,
    /// Monotonic end timestamp in nanoseconds.
    pub end_ns: i64,
}

/// Recursive call-tree node for instrumented code regions (T2, spec §9.4, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionNode {
    /// Region name.
    pub name: String,
    /// Number of entries into this region.
    pub calls: u64,
    /// Inclusive elapsed time in seconds.
    pub incl_s: f64,
    /// Exclusive elapsed time in seconds.
    pub excl_s: f64,
    /// Nested child regions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<RegionNode>,
}

/// Per-thread region tree (`observations.regions.threads[i]`, spec §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadRegions {
    /// OS thread ID (`tid`).
    pub tid: i64,
    /// Top-level region nodes for this thread.
    pub tree: Vec<RegionNode>,
}

/// Region observations block (`observations.regions`, T2, spec §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Regions {
    /// Per-thread region call trees.
    pub threads: Vec<ThreadRegions>,
}

/// Workload-reported metric via `CALO_CHANNEL` (`observations.reported.<key>`, spec §9.4, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportedMetric {
    /// Numeric metric value.
    pub value: f64,
    /// Optional unit string reported by the workload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Loaded executable/shared library module for symbolization (`observations.profile.modules[i]`, spec §9.3 B10, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileModule {
    /// Filesystem path to the mapped binary/library.
    pub path: String,
    /// ELF build-id hex string.
    pub build_id: String,
    /// Base virtual memory load address.
    pub base: u64,
    /// Mapped byte size.
    pub size: u64,
}

/// CPU stack sampling observations block (`observations.profile`, T1, spec §9.3, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordProfile {
    /// Stack unwinding method (`fp` or `dwarf`).
    pub method: CallgraphMethod,
    /// Sampling frequency in Hz.
    pub freq_hz: u32,
    /// Total captured stack samples.
    pub samples: u64,
    /// Total lost samples reported by the kernel ring buffer.
    pub lost: u64,
    /// Mapped binary modules and build-ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<ProfileModule>,
    /// Sidecar reference to raw stack samples table.
    pub data: Sidecar,
}

/// Aggregated per-(function, communicator) MPI statistics (M1, spec §9.5 B17, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MpiFunctionSummary {
    /// MPI function name (e.g. `"MPI_Allreduce"`).
    pub name: String,
    /// Logical communicator identifier or label.
    pub comm: String,
    /// Invocation count.
    pub calls: u64,
    /// Total cumulative time spent in seconds.
    pub total_s: f64,
    /// Minimum single-call duration in seconds.
    pub min_s: f64,
    /// Maximum single-call duration in seconds.
    pub max_s: f64,
    /// Total bytes sent.
    pub bytes_sent: u64,
    /// Total bytes received.
    pub bytes_recv: u64,
    /// Power-of-two message size histogram counts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub size_hist: Vec<u64>,
}

/// Cross-node clock synchronization parameters (`observations.mpi.clock_sync`, spec §9.7 B33, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MpiClockSync {
    /// Estimated clock offset to rank 0 at `MPI_Init` in nanoseconds.
    pub offset_ns: i64,
    /// Estimated linear clock drift in parts per million (ppm).
    pub drift_ppm: f64,
    /// Minimum round-trip time bound in nanoseconds.
    pub rtt_bound_ns: i64,
    /// Synchronization method name (e.g. `"ping_pong_min_rtt"`).
    pub method: String,
}

/// MPI observations block (`observations.mpi`, M1/M2, spec §9.5, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordMpi {
    /// Provided MPI thread support level (e.g. `"MPI_THREAD_MULTIPLE"`).
    pub thread_level: String,
    /// Self-measured PMPI shim overhead per call in nanoseconds (spec §9.5 B20).
    pub shim_overhead_ns_per_call: f64,
    /// Per-(function, communicator) aggregated statistics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<MpiFunctionSummary>,
    /// Sparse point-to-point peer communication matrix sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<Sidecar>,
    /// Collective entry/exit timestamp sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collectives: Option<Sidecar>,
    /// Optional M2 event trace sidecar (requires M2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<Sidecar>,
    /// Cross-node clock synchronization metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_sync: Option<MpiClockSync>,
}

/// Per-GPU device observations (`observations.gpu.devices[i]`, G0–G2, spec §9.6, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordGpuDevice {
    /// Device index on the node.
    pub index: u32,
    /// Ranks owning/bound to this device.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<u32>,
    /// G0 device state time-series sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_series: Option<Sidecar>,
    /// G1 aggregated kernel table sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernels: Option<Sidecar>,
    /// G1 memory copy/memset operations sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memcpy: Option<Sidecar>,
    /// G1 kernel and transfer activity timeline sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<Sidecar>,
    /// G2 hardware counter replay sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counters: Option<Sidecar>,
    /// G1 host/device clock correlation samples sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_correlation: Option<Sidecar>,
}

/// GPU observations block (`observations.gpu`, G0–G2, spec §9.6, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordGpu {
    /// Per-device sidecar references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<RecordGpuDevice>,
    /// Activity tracing source (`in_process` or `external`, G1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<GpuTraceSource>,
    /// Measured GPU activity tracing overhead in seconds (spec §9.6 B29).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracing_overhead_s: Option<f64>,
}

/// Observations container in `calo.record/2` (spec §10).
/// Each block is `Option` and present iff its tier is active in `provenance.tiers_active` (R-REC-8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecordObservations {
    /// T0 scalar resource and counter measurements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scalars: Option<RecordScalars>,
    /// T2 / channel workload phase timestamps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phases: Option<BTreeMap<String, PhaseSpan>>,
    /// T2 hierarchical region call trees per thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regions: Option<Regions>,
    /// T2 workload-reported metrics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported: Option<BTreeMap<String, ReportedMetric>>,
    /// T2 workload-reported string tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<BTreeMap<String, String>>,
    /// T0 host time-series sidecars keyed by series name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<BTreeMap<String, Sidecar>>,
    /// T1 sampled CPU stack profile metadata and sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<RecordProfile>,
    /// M1/M2 MPI shim summaries, collectives, and traces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<RecordMpi>,
    /// G0/G1/G2 GPU state series, kernel timelines, and counter sidecars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<RecordGpu>,
}

/// Shim version strings in `provenance.shim_versions` (spec §10, §18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShimVersions {
    /// MPI PMPI shim version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<String>,
    /// GPU tracing shim version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

/// Workload process exit status in `provenance.exit` (spec §9.3 B6, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitInfo {
    /// Process exit code (if exited normally or via signal convention).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// Terminating signal name if killed by signal (e.g. `"SIGSEGV"`, `"SIGTERM"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
}

/// Overflow counters for ring buffers and sidecar writer queues (`provenance.trace_overflow`, spec §9.2 B-O2, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TraceOverflow {
    /// Dropped MPI event trace records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<u64>,
    /// Dropped GPU activity timeline records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<u64>,
    /// Dropped chunks in the background sidecar writer queue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer_queue: Option<u64>,
}

/// Provenance block in `calo.record/2` (`provenance:`, spec §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordProvenance {
    /// Version of `probe` that produced this record.
    pub probe_version: String,
    /// Versions of loaded MPI and GPU shims.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shim_versions: Option<ShimVersions>,
    /// Hardware counter aliases requested for the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counters: Vec<String>,
    /// Monotonic clock source name (e.g. `"CLOCK_MONOTONIC_RAW"`, spec §3 I6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock: Option<String>,
    /// Callgraph method used (`fp` or `dwarf`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callgraph: Option<CallgraphMethod>,
    /// Host wall-clock start timestamp (ISO 8601 with offset).
    pub start: String,
    /// Host wall-clock end timestamp (ISO 8601 with offset).
    pub end: String,
    /// Monotonic start anchor in nanoseconds (`CLOCK_MONOTONIC_RAW`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ns: Option<i64>,
    /// Workload process exit status.
    pub exit: ExitInfo,
    /// Fraction of lost CPU profile samples (`lost / (samples + lost)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_loss: Option<f64>,
    /// Ring buffer and writer queue overflow counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_overflow: Option<TraceOverflow>,
    /// Authoritative active capability tiers for this run (spec §10 R-REC-8).
    pub tiers_active: TiersActive,
    /// Structured warnings emitted during the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    /// Structured non-fatal shim/probe errors emitted during the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Warning>,
}

/// Statistical summary across ranks in aggregate `run.yaml` (spec §10 R-REC-7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankAggregateStats {
    /// Minimum value across ranks.
    pub min: f64,
    /// Arithmetic mean across ranks.
    pub mean: f64,
    /// Maximum value across ranks.
    pub max: f64,
    /// Load imbalance ratio (`max / mean`).
    pub imbalance: f64,
}

/// Optional run-level aggregation block present in `run.yaml` (spec §10 R-REC-7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RunLevelAggregate {
    /// Relative paths to all constituent `rank<k>.yaml` records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rank_records: Vec<String>,
    /// Aggregated scalar statistics across ranks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scalars: BTreeMap<String, RankAggregateStats>,
    /// Aggregated region inclusive time statistics across ranks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub regions: BTreeMap<String, RankAggregateStats>,
    /// Aggregated symbol class statistics across ranks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub classes: BTreeMap<String, RankAggregateStats>,
}

/// Run record document (`calo.record/2`, spec §10).
/// Field declaration order matches normative emission order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Schema header (`"calo.record/2"`).
    pub schema: String,
    /// Run and rank identity block.
    pub identity: RecordIdentity,
    /// Hardware, placement, source, patch, and build environment block.
    pub environment: RecordEnvironment,
    /// Tier-gated observation blocks.
    #[serde(default)]
    pub observations: RecordObservations,
    /// Probe, clock, exit, tier, and diagnostic provenance block.
    pub provenance: RecordProvenance,
    /// Optional run-level multi-rank summary (present in `run.yaml`, spec §10 R-REC-7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<RunLevelAggregate>,
}

impl Record {
    /// Verifies that all required fields per spec §10 R-REC-1 are non-empty.
    /// Required: `schema`, `identity.{run_id,benchmark,mode,status}`,
    /// `environment.{machine,observed}`, `provenance.{probe_version,tiers_active,start,end,exit}`.
    pub fn required_fields_present(&self) -> Result<(), SchemaError> {
        if self.schema.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "schema".to_string(),
            });
        }
        if self.identity.run_id.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "identity.run_id".to_string(),
            });
        }
        if self.identity.benchmark.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "identity.benchmark".to_string(),
            });
        }
        if self.environment.machine.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "environment.machine".to_string(),
            });
        }
        if self.provenance.probe_version.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "provenance.probe_version".to_string(),
            });
        }
        if self.provenance.start.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "provenance.start".to_string(),
            });
        }
        if self.provenance.end.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "provenance.end".to_string(),
            });
        }
        if self.provenance.exit.code.is_none() && self.provenance.exit.signal.is_none() {
            return Err(SchemaError::MissingRequiredField {
                field: "provenance.exit".to_string(),
            });
        }
        Ok(())
    }

    /// Enforces spec §10 R-REC-8: `provenance.tiers_active` is authoritative.
    /// Any observation block whose required capability tier is absent from `provenance.tiers_active`
    /// is rejected with `SchemaError::InactiveTierObservation`.
    pub fn tier_authoritative_check(&self) -> Result<(), SchemaError> {
        let tiers = &self.provenance.tiers_active;
        let obs = &self.observations;

        // CPU T0 blocks: scalars, series
        if obs.scalars.is_some() && !tiers.satisfies( crate::common::Tier::T0 ) {
            return Err(SchemaError::InactiveTierObservation {
                block: "observations.scalars",
                required_tier: "T0",
            });
        }
        if obs.series.is_some() && !tiers.satisfies(crate::common::Tier::T0) {
            return Err(SchemaError::InactiveTierObservation {
                block: "observations.series",
                required_tier: "T0",
            });
        }

        // CPU T1 block: profile
        if obs.profile.is_some() && !tiers.satisfies(crate::common::Tier::T1) {
            return Err(SchemaError::InactiveTierObservation {
                block: "observations.profile",
                required_tier: "T1",
            });
        }

        // CPU T2 blocks: phases, regions, reported, tags
        if obs.phases.is_some() && !tiers.satisfies(crate::common::Tier::T2) {
            return Err(SchemaError::InactiveTierObservation {
                block: "observations.phases",
                required_tier: "T2",
            });
        }
        if obs.regions.is_some() && !tiers.satisfies(crate::common::Tier::T2) {
            return Err(SchemaError::InactiveTierObservation {
                block: "observations.regions",
                required_tier: "T2",
            });
        }
        if obs.reported.is_some() && !tiers.satisfies(crate::common::Tier::T2) {
            return Err(SchemaError::InactiveTierObservation {
                block: "observations.reported",
                required_tier: "T2",
            });
        }
        if obs.tags.is_some() && !tiers.satisfies(crate::common::Tier::T2) {
            return Err(SchemaError::InactiveTierObservation {
                block: "observations.tags",
                required_tier: "T2",
            });
        }

        // MPI M1/M2 blocks
        if let Some(mpi) = &obs.mpi {
            if !tiers.satisfies(crate::common::Tier::M1) {
                return Err(SchemaError::InactiveTierObservation {
                    block: "observations.mpi",
                    required_tier: "M1",
                });
            }
            if mpi.trace.is_some() && !tiers.satisfies(crate::common::Tier::M2) {
                return Err(SchemaError::InactiveTierObservation {
                    block: "observations.mpi.trace",
                    required_tier: "M2",
                });
            }
        }

        // GPU G0/G1/G2 blocks
        if let Some(gpu) = &obs.gpu {
            if !tiers.satisfies(crate::common::Tier::G0) {
                return Err(SchemaError::InactiveTierObservation {
                    block: "observations.gpu",
                    required_tier: "G0",
                });
            }
            let has_g1_data = gpu.source.is_some()
                || gpu.tracing_overhead_s.is_some()
                || gpu.devices.iter().any(|d| {
                    d.kernels.is_some()
                        || d.memcpy.is_some()
                        || d.timeline.is_some()
                        || d.clock_correlation.is_some()
                });
            if has_g1_data && !tiers.satisfies(crate::common::Tier::G1) {
                return Err(SchemaError::InactiveTierObservation {
                    block: "observations.gpu (G1 activity)",
                    required_tier: "G1",
                });
            }
            let has_g2_data = gpu.devices.iter().any(|d| d.counters.is_some());
            if has_g2_data && !tiers.satisfies(crate::common::Tier::G2) {
                return Err(SchemaError::InactiveTierObservation {
                    block: "observations.gpu.devices.counters",
                    required_tier: "G2",
                });
            }
        }

        // Verify CpuTier/MpiTier/GpuTier enum usage directly
        let _ = (CpuTier::T0, MpiTier::M0, GpuTier::G0);

        Ok(())
    }
}

impl SchemaDoc for Record {
    const SCHEMA: &'static str = "calo.record/2";

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
        self.required_fields_present()?;
        self.tier_authoritative_check()?;
        Ok(())
    }
}
