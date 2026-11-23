//! Execution adapter trait and job handle/state types (spec §7 R-INV-6, §15).

use crate::exec::error::ExecError;
use calo_schema::RunSpec;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// Opaque handle returned by `Adapter::submit` (spec §7 R-INV-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobHandle {
    /// Adapter-specific job identifier (e.g. PID or Slurm job ID).
    pub id: String,
    /// Path to the generated `calo.runspec/1` file.
    pub runspec_path: PathBuf,
    /// Directory where `probe` writes `rank<k>.yaml` and sidecars.
    pub outputs_dir: PathBuf,
}

/// Lifecycle state of a submitted job (spec §7 R-INV-6, §17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    /// Queued in scheduler, not yet executing.
    Pending,
    /// Actively executing.
    Running,
    /// Finished with exit code 0.
    Completed,
    /// Finished with nonzero exit code.
    Failed {
        /// Exit code if available.
        code: Option<i32>,
    },
    /// Exceeded step wall-clock timeout.
    TimedOut,
    /// Explicitly cancelled.
    Cancelled,
}

/// Staged execution bundle passed from `Adapter::stage` to `Adapter::submit` (spec §7 R-INV-6).
#[derive(Debug, Clone)]
pub struct StagedRun {
    /// Run identifier.
    pub run_id: String,
    /// Full `RunSpec` serialized to `runspec_path`.
    pub runspec: RunSpec,
    /// Path to `runspec.yaml`.
    pub runspec_path: PathBuf,
    /// Full argv vector (`probe --runspec ...` or `<launcher...> probe --rank-aware --runspec ...`).
    pub argv: Vec<String>,
    /// Working directory for execution.
    pub cwd: PathBuf,
    /// Additional launcher environment variables.
    pub env: BTreeMap<String, String>,
    /// Optional wall-clock timeout.
    pub timeout: Option<Duration>,
}

/// Execution backend adapter interface (`stage`, `submit`, `poll`, `fetch`, `cancel`, spec §7 R-INV-6).
pub trait Adapter: Send + Sync {
    /// Prepares files and writes `runspec.yaml` for a run.
    fn stage(&self, staged: StagedRun) -> Result<StagedRun, ExecError>;

    /// Submits the staged run for execution and returns a `JobHandle`.
    fn submit(&self, staged: &StagedRun) -> Result<JobHandle, ExecError>;

    /// Polls the current `JobState` of a submitted job.
    fn poll(&self, handle: &JobHandle) -> Result<JobState, ExecError>;

    /// Collects outputs from a completed job into `dest_dir`.
    fn fetch(&self, handle: &JobHandle, dest_dir: &std::path::Path) -> Result<(), ExecError>;

    /// Requests cancellation of a running or pending job.
    fn cancel(&self, handle: &JobHandle) -> Result<(), ExecError>;
}
