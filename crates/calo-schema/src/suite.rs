//! Benchmark suite schema (`calo.suite/1`, spec §5, §8, §12, §13).

use crate::common::{Mode, ScalarValue, Tier};
use crate::error::SchemaError;
use crate::runspec::{RunSpecGpu, RunSpecProfile};
use crate::SchemaDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Suite metadata header under `suite:` (spec §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SuiteMeta {
    /// Suite name.
    pub name: String,
    /// Suite version string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Human-readable suite description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Git source repository definition under `sources:` (spec §8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SourceDef {
    /// Git repository URL or path.
    pub git: String,
    /// Tag, branch, or commit SHA (resolved to commit SHA at suite start per R-SRC-1).
    pub rev: String,
    /// Whether to initialize and update git submodules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodules: Option<bool>,
}

/// Patch conflict resolution behavior (spec §8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatchConflictPolicy {
    /// Fail the build/run if a patch does not apply cleanly.
    Fail,
    /// Skip conflicting patch sets.
    Skip,
}

/// Patch application strategy (spec §8.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PatchApplyConfig {
    /// Application method (e.g. `"git-apply"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Path strip level (`-pN`, default 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strip: Option<u32>,
    /// Behavior when a patch fails to apply cleanly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_conflict: Option<PatchConflictPolicy>,
}

/// Patch sets configuration under `patches:` (spec §8.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct PatchesConfig {
    /// Directory containing `.patch` files and optional `patchset.yaml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Named ordered lists of patch filenames.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sets: BTreeMap<String, Vec<String>>,
    /// Patch application options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply: Option<PatchApplyConfig>,
}

/// Optional `patchset.yaml` per-patch metadata (spec §8.2 R-PAT-4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct PatchMetadata {
    /// Required source version constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// List of conflicting patch filenames.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts_with: Vec<String>,
    /// Human-readable description of the patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// GPU/accelerator compute backend for a build (spec §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuildBackend {
    /// NVIDIA CUDA backend.
    Cuda,
    /// AMD ROCm HIP backend.
    Hip,
    /// Intel/Khronos SYCL backend.
    Sycl,
    /// Host CPU only.
    None,
}

/// Rebuild cache policy (spec §8.3 R-BLD-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RebuildPolicy {
    /// Reuse existing build matching the cache key.
    Cache,
    /// Always rebuild from a clean tree.
    Fresh,
}

/// Debug info requirement for a build (spec §8.3 R-BLD-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugInfoPolicy {
    /// Debug symbols and frame pointers are mandatory (required for `mode: profile`).
    Required,
    /// Debug symbols are optional.
    Optional,
}

/// Build variant definition under `builds:` (spec §8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BuildDef {
    /// Opaque shell script to configure and compile the target.
    pub run: String,
    /// Compiler flags string recorded in the build cache key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    /// Accelerator backend (`cuda`, `hip`, `sycl`, `none`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BuildBackend>,
    /// Environment modules required for this build.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// Whether debug symbols are required (R-BLD-3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_info: Option<DebugInfoPolicy>,
    /// Whether GPU device lineinfo (`-lineinfo`) is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_lineinfo: Option<bool>,
    /// Build caching policy (`cache` or `fresh`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<RebuildPolicy>,
    /// Whether to measure build time itself as a benchmark (R-BLD-7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured: Option<bool>,
    /// Declared output artifact glob patterns for SHA-256 hashing (spec §8.3 R-BLD-2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

/// Value that may be either an integer literal or an interpolated `${{ expr }}` string (spec §8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum IntOrExpr {
    /// Literal unsigned integer.
    Int(u32),
    /// Expression string (e.g. `"${{ params.nodes }}"`).
    Expr(String),
}

/// Resource requirements for a step (spec §8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct StepResources {
    /// Number of nodes to allocate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<IntOrExpr>,
    /// Number of MPI ranks per node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ranks_per_node: Option<IntOrExpr>,
    /// Number of OpenMP/host threads per rank.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads_per_rank: Option<IntOrExpr>,
    /// Number of GPUs per rank.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpus_per_rank: Option<IntOrExpr>,
}

/// Rendered input file template specification in a step (spec §8.4 R-STP-3, R-STP-8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StepFileTemplate {
    /// Relative path to the template file.
    pub template: String,
    /// Variables substituted via `{{ var }}` in the template.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, ScalarValue>,
}

/// Channel expectation descriptor on a step (spec §8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct StepChannelConfig {
    /// Declared workload phases (e.g. `["init", "main"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<String>,
    /// Declared code regions (e.g. `["field_update", "fft"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<String>,
    /// Phases excluded from steady-state rate calculations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_from_rates: Vec<String>,
}

/// Execution step definition under `steps:` (spec §8.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StepDef {
    /// Shell command to execute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Whether this step is the measured workload step (R-STP-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured: Option<bool>,
    /// Step kind (e.g. `"native"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Reusable action fragment path (`./actions/<fragment>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uses: Option<String>,
    /// Node, rank, thread, and GPU resource requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<StepResources>,
    /// Input files rendered from templates before the step runs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, StepFileTemplate>,
    /// Expected `CALO_CHANNEL` phases and regions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<StepChannelConfig>,
    /// Step wall-clock timeout (`"HH:MM:SS"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
}

/// Matrix axis expansion mode (spec §8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpandMode {
    /// Cartesian product of all axis values.
    All,
    /// Single run with every axis at its default value.
    Defaults,
    /// Empty base set (only explicit `include` patterns are added).
    None,
}

/// Scaling study kind (spec §8.5 R-BEN-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScalingKind {
    /// Fixed total problem size across increasing resources: `T(base)*P(base) / (T*P)`.
    Strong,
    /// Fixed problem size per resource unit: `T(base) / T`.
    Weak,
}

/// Resource axis over which scaling efficiency is computed (spec §8.5 R-BEN-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScalingOver {
    /// Scale over node count.
    Nodes,
    /// Scale over total MPI rank count.
    Ranks,
    /// Scale over total GPU count.
    Gpus,
}

/// Scaling metadata on a benchmark (spec §8.5 R-BEN-5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScalingConfig {
    /// Strong or weak scaling formula.
    pub kind: ScalingKind,
    /// Resource quantity `P` in the scaling formula.
    pub over: ScalingOver,
    /// Parameter pattern selecting the reference baseline run.
    pub baseline: BTreeMap<String, ScalarValue>,
}

/// OpenMP instrumentation mode in `probe:` (spec §8.5, §9.5 B22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OmpMode {
    /// Stack frame classification only.
    Off,
    /// OMPT runtime interface barrier/idle instrumentation.
    Ompt,
}

/// MPI instrumentation shorthand on a benchmark `probe:` block (spec §8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkMpiMode {
    /// No MPI shim.
    Off,
    /// M1 summary aggregation.
    Summary,
    /// M2 bounded event trace.
    Events,
}

/// Probe configuration nested under a benchmark in `suite.yaml` (spec §8.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct BenchmarkProbeConfig {
    /// Counter aliases to collect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counters: Vec<String>,
    /// Time series sampling rate in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_hz: Option<u32>,
    /// CPU stack sampling options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<RunSpecProfile>,
    /// MPI shim mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpi: Option<BenchmarkMpiMode>,
    /// GPU state, tracing, and counter options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<RunSpecGpu>,
    /// OpenMP instrumentation mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omp: Option<OmpMode>,
}

/// Slurm job batching granularity (spec §8.5, §15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchMode {
    /// One scheduler job per individual run.
    Run,
    /// One scheduler job for the benchmark group.
    Group,
}

/// Target machine reference on a benchmark (either `"name"` / `"name/class"` or a selector map, spec §8.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum MachineTarget {
    /// Direct machine name or `machine/node_class` string.
    Name(String),
    /// Selector expression or key-value map.
    Selector(BTreeMap<String, ScalarValue>),
}

/// Benchmark definition in `suite.yaml` (spec §8.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BenchmarkDef {
    /// Unique benchmark name.
    pub name: String,
    /// Logical group name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Execution mode (`measure`, `profile`, `kernel`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// Target machine or node class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<MachineTarget>,
    /// Named build variant from `builds:`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Ordered step names from `steps:`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    /// Explicitly named measured steps when multiple steps are measured (spec §8.4 R-STP-2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measure: Vec<String>,
    /// Fixed scalar parameters for this benchmark.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ScalarValue>,
    /// Parameter sweep axes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, Vec<ScalarValue>>,
    /// Derived parameters evaluated via expression language in topological dependency order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub derive: BTreeMap<String, String>,
    /// Default values for axes when `expand: defaults` or partial `include` patterns are used.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub defaults: BTreeMap<String, ScalarValue>,
    /// Base matrix expansion strategy (`all`, `defaults`, `none`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expand: Option<ExpandMode>,
    /// Additional parameter patterns to include in the expanded matrix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<BTreeMap<String, ScalarValue>>,
    /// Parameter patterns to exclude from the expanded matrix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<BTreeMap<String, ScalarValue>>,
    /// Strong or weak scaling configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scaling: Option<ScalingConfig>,
    /// Number of measured repetitions per expanded point (R-BEN-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// Number of warmup runs per expanded point (R-BEN-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warmup: Option<u32>,
    /// Probe instrumentation configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<BenchmarkProbeConfig>,
    /// Source benchmark name to derive expanded runs from (spec §8.5 step 6, R-BEN-4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Filter expression applied when deriving from another benchmark or restricting axes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub where_expr: Option<String>,
    /// Alias for `where` key in YAML (`where` is a Rust keyword).
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter_where: Option<String>,
    /// Maximum concurrent runs allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    /// Scheduler batching mode (`run` or `group`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchMode>,
    /// Shorthand benchmark-scoped reports (spec §13 R-REP-2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<ReportDef>,
}

/// Confidence interval configuration for `stats.summary` (spec §12.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CiConfig {
    /// CI method (e.g. `"bootstrap"`).
    pub method: String,
    /// Confidence level (e.g. `0.95`).
    pub level: f64,
    /// Number of bootstrap resamples (e.g. `10000`).
    pub resamples: u32,
    /// Deterministic PRNG seed.
    pub seed: u64,
}

/// Outlier filter configuration for `stats.summary` (spec §12.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlierConfig {
    /// Outlier detection method (e.g. `"mad"`).
    pub method: String,
    /// Threshold multiplier (e.g. `3.5`).
    pub threshold: f64,
}

/// Service configuration or definition entry under `services:` (spec §12.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct ServiceConfig {
    /// Estimator for `stats.summary` (`"median"` or `"mean"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimator: Option<String>,
    /// Confidence interval settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiConfig>,
    /// Outlier filtering settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outliers: Option<OutlierConfig>,
    /// Expression string for expression-defined services (spec §12.2 R-SVC-5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    /// Display/output unit for expression-defined services.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Out-of-process Python entrypoint (`"pkg.module:function"`, spec §12.4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    /// Declared input column/metric names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// Declared output column/metric names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: Vec<String>,
    /// Required capability tiers (spec §12.2 R-SVC-2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Tier>,
}

/// Symbol folding rule in `symbols.fold` (spec §12.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SymbolFoldRule {
    /// Regex pattern to match against demangled symbol names.
    #[serde(rename = "match")]
    pub match_regex: String,
    /// Replacement string.
    #[serde(rename = "as")]
    pub replace_as: String,
}

/// Symbol classification rule in `symbols.classes` (spec §12.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SymbolClassRule {
    /// Regex pattern to match against symbol/kernel names.
    #[serde(rename = "match")]
    pub match_regex: String,
    /// Target classification label (e.g. `"mpi"`, `"fft"`, `"memory"`).
    pub class: String,
}

/// Symbol normalization and classification configuration (spec §8, §12.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct SymbolsConfig {
    /// Ordered regex replacement rules to collapse template/wrapper noise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fold: Vec<SymbolFoldRule>,
    /// Ordered regex classification rules (first match wins).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<SymbolClassRule>,
}

/// Single selector clause for filtering benchmarks/runs (spec §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct SelectorClause {
    /// Benchmark name or glob pattern.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<String>,
    /// Benchmark group name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Equality constraints on run parameters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ScalarValue>,
    /// Boolean filter expression.
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    pub where_expr: Option<String>,
    /// Execution mode filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// Required capability tiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tier: Vec<Tier>,
}

/// Report `select:` specification (`"all"`, a single selector, or a union list of selectors, spec §5.3, §13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ReportSelect {
    /// Keyword `"all"`.
    All(String),
    /// Single selector clause (keys intersect).
    Single(SelectorClause),
    /// List of selector clauses (union).
    List(Vec<SelectorClause>),
}

/// Plot axis scaling configuration (spec §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct PlotScale {
    /// X-axis scale (e.g. `"linear"`, `"log"`, `"log2"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// Y-axis scale (e.g. `"linear"`, `"log"`, `"log2"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
}

/// Plot specification inside a report (spec §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReportPlot {
    /// Built-in or external plot type (e.g. `"line"`, `"bar"`, `"roofline"`).
    #[serde(rename = "type")]
    pub plot_type: String,
    /// X-axis variable or parameter reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// Y-axis metric or service output reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    /// Grouping key for multiple lines/series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_by: Option<String>,
    /// Faceting key across subplots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facet_by: Option<String>,
    /// Axis scale transformations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<PlotScale>,
    /// Error bar source (e.g. `"ci95"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errorbars: Option<String>,
    /// Plot title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Pivot table specification inside a report (spec §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReportTable {
    /// Row grouping key (e.g. `"params.patch"`).
    pub rows: String,
    /// Column grouping key (e.g. `"params.N"`).
    pub cols: String,
    /// Cell metric/value name (e.g. `"speedup"`).
    pub value: String,
}

/// Regression gate specification inside a report (spec §13 R-REP-4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReportGate {
    /// Metric to evaluate (e.g. `"wall_s.median"`).
    pub metric: String,
    /// Selector identifying the baseline comparison benchmark/runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compare: Option<SelectorClause>,
    /// Failure threshold (`"> +5%"`, `"< -10%"`, or an expression).
    pub fail_if: String,
}

/// Report output format (spec §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReportOutputFormat {
    /// Markdown format.
    Md,
    /// Static HTML format.
    Html,
    /// PDF document format.
    Pdf,
    /// Comma-separated values format.
    #[serde(alias = "csv")]
    Csv,
    /// Scalable Vector Graphics format.
    Svg,
}

/// Report definition under `reports:` (spec §13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReportDef {
    /// Report name.
    pub name: String,
    /// Capability tiers required by this report (spec §2, §13).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Tier>,
    /// Run selector (`all`, single selector, or list of selectors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select: Option<ReportSelect>,
    /// Plots included in this report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plots: Vec<ReportPlot>,
    /// Summary/pivot tables included in this report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<ReportTable>,
    /// Automated regression gates evaluated by this report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<ReportGate>,
    /// Output formats to generate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<ReportOutputFormat>,
}

/// Run ordering policy in `options:` (spec §8, §15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunOrder {
    /// Interleave repetitions and scales across runs (default).
    Interleaved,
    /// Randomized execution order.
    Random,
    /// Sequential order as expanded.
    Sequential,
}

/// Failure handling policy in `options:` (spec §8.4 R-STP-7, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnFailurePolicy {
    /// Record failed run and continue suite execution (default).
    Record,
    /// Abort suite execution on first run failure.
    Stop,
}

/// Suite-wide options under `options:` (spec §7 R-INV-2, §8, §15, §16 R-SAF-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct SuiteOptions {
    /// Treat hardware mismatches between inventory and observed node as errors (R-INV-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_hardware: Option<bool>,
    /// Run scheduling order (`interleaved`, `random`, `sequential`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<RunOrder>,
    /// Behavior when a step fails (`record` or `stop`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<OnFailurePolicy>,
    /// Environment variable names whose values must be redacted from manifests and records (R-SAF-5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact: Vec<String>,
}

/// Top-level `suite.yaml` document (`calo.suite/1`, spec §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Suite {
    /// Schema header (`"calo.suite/1"`).
    pub schema: String,
    /// Optional list of relative YAML files to include (spec §5.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Suite identity metadata.
    pub suite: SuiteMeta,
    /// Read-only semantic context files attached verbatim to reports (spec §8, §13 R-REP-5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    /// Source repositories.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, SourceDef>,
    /// Patch sets and application configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patches: Option<PatchesConfig>,
    /// Named build configurations.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub builds: BTreeMap<String, BuildDef>,
    /// Named execution steps.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, StepDef>,
    /// Benchmark definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benchmarks: Vec<BenchmarkDef>,
    /// Service configurations and custom service definitions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub services: BTreeMap<String, ServiceConfig>,
    /// Symbol folding and classification rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbols: Option<SymbolsConfig>,
    /// Top-level report definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<ReportDef>,
    /// Suite execution options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<SuiteOptions>,
}

impl SchemaDoc for Suite {
    const SCHEMA: &'static str = "calo.suite/1";

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
        if self.suite.name.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "suite.name".to_string(),
            });
        }
        Ok(())
    }
}
