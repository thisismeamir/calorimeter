//! Shared enums and value types across Calorimeter schemas (spec §2, §6.3, §8, §9, §10, §17).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Unified capability tier identifier used in `requires` lists and selectors (spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub enum Tier {
    /// CPU Tier 0: Time, rusage, counters, series.
    T0,
    /// CPU Tier 1: Sampled stacks, hotspots, flame graphs.
    T1,
    /// CPU Tier 2: Region markers, exact region times, user metrics.
    T2,
    /// MPI Tier 0: Placement, per-rank records, MPI time via stack classification.
    M0,
    /// MPI Tier 1: PMPI shim per-call counts, time, bytes, wait, peer matrix.
    M1,
    /// MPI Tier 2: PMPI shim event trace with aligned clocks.
    M2,
    /// GPU Tier 0: Device inventory, utilization, memory, power, clocks, throttling.
    G0,
    /// GPU Tier 1: Kernel and memcpy timeline, launch latency, host correlation.
    G1,
    /// GPU Tier 2: Per-kernel counters via replay.
    G2,
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::T0 => "T0",
            Self::T1 => "T1",
            Self::T2 => "T2",
            Self::M0 => "M0",
            Self::M1 => "M1",
            Self::M2 => "M2",
            Self::G0 => "G0",
            Self::G1 => "G1",
            Self::G2 => "G2",
        };
        f.write_str(s)
    }
}

/// CPU capability tier (spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub enum CpuTier {
    /// Time, rusage, counters, series.
    T0,
    /// Sampled stacks, hotspots, flame graphs.
    T1,
    /// Region markers, exact region times, user metrics.
    T2,
}

/// MPI capability tier (spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub enum MpiTier {
    /// Placement, per-rank records, MPI time via stack classification.
    M0,
    /// PMPI shim per-call counts, time, bytes, wait, peer matrix.
    M1,
    /// Event trace with aligned clocks.
    M2,
}

/// GPU capability tier (spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub enum GpuTier {
    /// Device inventory, utilization, memory, power, clocks, throttling.
    G0,
    /// Vendor tracing runtime: kernel and memcpy timeline, launch latency, host correlation.
    G1,
    /// Per-kernel counters via replay.
    G2,
}

/// Active capability tiers per domain for a run or machine (spec §2, §9.0, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct TiersActive {
    /// Highest active CPU tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuTier>,
    /// Highest active MPI tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<MpiTier>,
    /// Highest active GPU tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuTier>,
}

impl TiersActive {
    /// Checks whether a unified `Tier` is satisfied by the active domain tiers (spec §2, §10 R-REC-8).
    pub fn satisfies(&self, tier: Tier) -> bool {
        match tier {
            Tier::T0 => self.cpu.is_some_and(|c| c >= CpuTier::T0),
            Tier::T1 => self.cpu.is_some_and(|c| c >= CpuTier::T1),
            Tier::T2 => self.cpu.is_some_and(|c| c >= CpuTier::T2),
            Tier::M0 => self.mpi.is_some_and(|m| m >= MpiTier::M0),
            Tier::M1 => self.mpi.is_some_and(|m| m >= MpiTier::M1),
            Tier::M2 => self.mpi.is_some_and(|m| m >= MpiTier::M2),
            Tier::G0 => self.gpu.is_some_and(|g| g >= GpuTier::G0),
            Tier::G1 => self.gpu.is_some_and(|g| g >= GpuTier::G1),
            Tier::G2 => self.gpu.is_some_and(|g| g >= GpuTier::G2),
        }
    }
}

/// Benchmark execution mode (spec §8.5 R-BEN-3, §9.9, §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// T0, G0, M0 only; source of headline metrics.
    Measure,
    /// CPU sampling, MPI shim, GPU activity tracing.
    Profile,
    /// GPU counter replay, short problems.
    Kernel,
}

/// Run completion status (spec §8.4 R-STP-7, §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Workload completed with zero exit code and valid measurements.
    Ok,
    /// Workload exited nonzero.
    Failed,
    /// Workload exceeded step timeout.
    Timeout,
    /// Run invalidated by strict policy or hardware mismatch.
    Invalid,
}

/// Provenance confidence level for discovered hardware fields (spec §6.3 B-D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Directly measured by microbenchmark or hardware probe.
    Measured,
    /// Queried from OS, driver, or sysfs interface.
    Queried,
    /// Inferred from model tables or heuristics.
    Inferred,
    /// Could not be determined.
    Unavailable,
}

/// Standardized warning and error codes (spec §17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    /// Observed hardware differs from declared inventory.
    HardwareMismatch,
    /// Actual governor, turbo, or clocks differ from requested policy.
    PolicyMismatch,
    /// A requested tier was unavailable.
    TierDegraded,
    /// Profile build has no symbols.
    NoSymbols,
    /// Sample loss exceeded `max_loss`.
    SampleLoss,
    /// A ring buffer or writer queue overflowed.
    TraceOverflow,
    /// Counter multiplexing occurred.
    MuxDetected,
    /// Multiple ranks share a GPU device without MPS.
    OversubscribedGpu,
    /// Other unrelated jobs were co-located on the node.
    ColocatedJobs,
    /// Multiple runs overlapped on the node set.
    Concurrency,
    /// A shim failed and disabled itself.
    ShimDisabled,
    /// Cross-node clock alignment was unavailable.
    ClockUnsynced,
    /// Artifact hash differed across rebuilds.
    BuildNondeterministic,
    /// An external Python service could not run.
    PythonUnavailable,
}

/// Structured warning or error entry in `provenance.warnings` and `provenance.errors` (spec §10, §17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Warning {
    /// Normative warning/error code.
    pub code: WarningCode,
    /// Human-readable diagnostic message.
    pub message: String,
}

/// Callgraph unwinding method for CPU profiling (spec §8.5, §9.3 B8, §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallgraphMethod {
    /// Frame pointer unwinding.
    Fp,
    /// DWARF stack copy unwinding.
    Dwarf,
}

/// Shim injection mechanism (spec §9.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShimInject {
    /// Dynamic injection via `LD_PRELOAD` / vendor injection env vars.
    Preload,
    /// Static link-time inclusion via build hook flags.
    Link,
    /// `Preload` unless the binary is detected as statically linked.
    Auto,
}

/// Region storage strategy in `probe` (spec §9.4, §9.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegionStorageMode {
    /// Per-event call trees.
    Events,
    /// In-probe count/sum/min/max aggregation.
    Aggregate,
}

/// Scalar parameter value used in `params`, `defaults`, and `axes` (spec §0.1, §8.5, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ScalarValue {
    /// Signed 64-bit integer.
    Int(i64),
    /// 64-bit floating-point number.
    Float(f64),
    /// Boolean scalar.
    Bool(bool),
    /// UTF-8 string scalar.
    Str(String),
}

impl Eq for ScalarValue {}

impl fmt::Display for ScalarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
        }
    }
}
