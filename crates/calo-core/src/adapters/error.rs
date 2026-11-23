//! Typed errors for Slurm/SSH adapters and partition discovery enforcement (spec §6.2 R-SPC-1).

use crate::exec::ExecError;
use crate::proc::ProcError;
use thiserror::Error;

/// Errors produced by `calo-core::adapters` (spec §6.2, §15).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdapterError {
    /// Attempted to submit a discovery job to a Slurm partition not explicitly named (spec §6.2 R-SPC-1).
    #[error("refusing to submit to unnamed partition `{partition}` (spec R-SPC-1 requires --partition or inventory partitions)")]
    UnnamedPartition {
        /// Partition name that was rejected.
        partition: String,
    },

    /// Failed to parse `sbatch --parsable` job ID output.
    #[error("failed to parse Slurm job ID from `sbatch` output: `{raw}`")]
    InvalidJobId {
        /// Raw stdout returned by `sbatch`.
        raw: String,
    },

    /// Subprocess execution failure.
    #[error("subprocess error: {0}")]
    Proc(String),

    /// Execution staging or I/O error.
    #[error("adapter error: {0}")]
    Exec(String),
}

impl From<ProcError> for AdapterError {
    fn from(e: ProcError) -> Self {
        Self::Proc(e.to_string())
    }
}

impl From<ExecError> for AdapterError {
    fn from(e: ExecError) -> Self {
        Self::Exec(e.to_string())
    }
}
