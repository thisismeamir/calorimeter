//! Generated probe execution specification (`calo.runspec/1`, spec §9.9).

use crate::common::{CallgraphMethod, Mode, RegionStorageMode, ShimInject};
use crate::error::SchemaError;
use crate::SchemaDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// NUMA placement policy for a launched rank (spec §9.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NumaPolicy {
    /// Bind memory allocations to specified NUMA nodes.
    Bind,
    /// Interleave memory allocations across NUMA nodes.
    Interleave,
    /// Do not set explicit NUMA policy.
    None,
}

/// CPU and NUMA affinity settings in `calo.runspec/1` (spec §9.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSpecAffinity {
    /// Linux CPU list string (e.g. `"4-67"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_list: Option<String>,
    /// NUMA binding policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numa: Option<NumaPolicy>,
}

/// Process resource limits in `calo.runspec/1` (spec §9.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct RunSpecRlimits {
    /// Core dump size limit in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<u64>,
}

/// CPU stack sampling configuration in `calo.runspec/1` and `calo.suite/1` (spec §8.5, §9.3, §9.9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSpecProfile {
    /// Sampling frequency in Hz (default 499).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freq_hz: Option<u32>,
    /// Callgraph unwinding mechanism (`fp` or `dwarf`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callgraph: Option<CallgraphMethod>,
    /// DWARF stack copy size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_bytes: Option<u32>,
    /// Counter aliases to attribute per sample.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribute: Vec<String>,
    /// Maximum tolerated sample loss fraction (default 0.01).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_loss: Option<f64>,
}

/// MPI tracing mode (spec §8.5, §9.5, §9.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MpiTraceMode {
    /// No PMPI shim activity.
    Off,
    /// Per-call aggregation, peer matrix, collective timestamps (M1).
    Summary,
    /// Bounded per-rank ring buffer event trace (M2).
    Events,
}

/// MPI configuration inside `calo.runspec/1` (spec §9.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSpecMpi {
    /// MPI instrumentation mode.
    pub mode: MpiTraceMode,
    /// Maximum number of events in the M2 trace ring buffer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_max_events: Option<u64>,
}

/// GPU activity trace mode (spec §8.5, §9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GpuTraceMode {
    /// No GPU activity tracing (G0 only).
    Off,
    /// Kernel and memcpy timeline tracing (G1).
    Activity,
}

/// GPU tracing source (spec §8.5, §9.6 B25, §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GpuTraceSource {
    /// Shim loaded directly into the workload process.
    InProcess,
    /// External vendor CLI profiler wrapped by `probe`.
    External,
}

/// GPU configuration in `calo.runspec/1` and `calo.suite/1` (spec §8.5, §9.6, §9.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct RunSpecGpu {
    /// Whether to sample G0 device state out-of-process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<bool>,
    /// G0 device state sampling frequency in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_hz: Option<u32>,
    /// G1 activity tracing mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<GpuTraceMode>,
    /// G1 activity tracing source (`in_process` or `external`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<GpuTraceSource>,
    /// G2 hardware performance counter names to collect via replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counters: Vec<String>,
    /// Regex filtering kernels for G2 counter replay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_filter: Option<String>,
    /// Maximum number of kernel launches to replay in G2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_limit: Option<u32>,
    /// Sampling rate for host stack capture at kernel launch (e.g. `"1/N"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_stacks: Option<String>,
    /// Maximum number of GPU timeline events buffered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_max_events: Option<u64>,
}

/// Rank identification settings in `calo.runspec/1` (spec §9.5 B14, §9.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSpecRank {
    /// Discover rank, size, and local_rank from launcher environment variables.
    pub from_env: bool,
}

/// Output directory specification in `calo.runspec/1` (spec §9.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSpecOutputs {
    /// Path to the directory where `probe` writes `rank<k>.yaml` and sidecars.
    pub dir: String,
}

/// Input specification consumed by `probe --runspec` (`calo.runspec/1`, spec §9.9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSpec {
    /// Schema header (`"calo.runspec/1"`).
    pub schema: String,
    /// Deterministic run identifier.
    pub run_id: String,
    /// Execution mode (`measure`, `profile`, `kernel`).
    pub mode: Mode,
    /// Workload argv vector.
    pub command: Vec<String>,
    /// Working directory for workload execution.
    pub cwd: String,
    /// Environment variables to set in child process.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// CPU and NUMA affinity settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affinity: Option<RunSpecAffinity>,
    /// Process resource limits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rlimits: Option<RunSpecRlimits>,
    /// Hardware counter aliases to open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counters: Vec<String>,
    /// Host time-series sampling rate in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_hz: Option<u32>,
    /// CPU stack profiling parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<RunSpecProfile>,
    /// MPI tracing and shim parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<RunSpecMpi>,
    /// GPU state, activity tracing, and counter parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<RunSpecGpu>,
    /// Shim injection strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shim_inject: Option<ShimInject>,
    /// Region storage strategy (`events` or `aggregate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regions: Option<RegionStorageMode>,
    /// Path to the `CALO_CHANNEL` FIFO.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Rank environment detection flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<RunSpecRank>,
    /// Output directory descriptor.
    pub outputs: RunSpecOutputs,
}

impl SchemaDoc for RunSpec {
    const SCHEMA: &'static str = "calo.runspec/1";

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
        if self.run_id.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "run_id".to_string(),
            });
        }
        if self.command.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "command".to_string(),
            });
        }
        Ok(())
    }
}
