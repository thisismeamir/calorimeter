//! Typed errors for run execution, runspec building, and adapter operations (spec §8.4, §9, §15).

use crate::expr::ExprError;
use crate::proc::ProcError;
use crate::results::ResultsError;
use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by `calo-core::exec` (spec §8.4, §9, §15).
#[derive(Debug, Error)]
pub enum ExecError {
    /// Filesystem I/O failure.
    #[error("I/O error at `{path}`: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying OS I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Subprocess execution error.
    #[error("process error: {0}")]
    Proc(#[from] ProcError),

    /// Schema validation or YAML error.
    #[error("schema error: {0}")]
    Schema(#[from] calo_schema::SchemaError),

    /// Results directory or manifest persistence error.
    #[error("results error: {0}")]
    Results(#[from] ResultsError),

    /// Expression or template rendering error.
    #[error("expression/template error: {0}")]
    Expr(#[from] ExprError),

    /// Referenced benchmark, step, or machine was not found.
    #[error("missing {kind} `{name}`")]
    MissingTarget {
        /// Kind (`"benchmark"`, `"step"`, `"machine"`).
        kind: &'static str,
        /// Missing identifier.
        name: String,
    },

    /// `measure` mode benchmark attempted to enable sampling or shims (spec §8.5 R-BEN-3).
    #[error("benchmark `{benchmark}` is in `measure` mode, which forbids {feature} (spec R-BEN-3)")]
    InvalidModeConfig {
        /// Benchmark name.
        benchmark: String,
        /// Forbidden feature description.
        feature: &'static str,
    },

    /// `probe` binary could not be located via `--probe`, `$CALO_PROBE`, or executable sibling.
    #[error("could not locate `probe` binary (checked --probe, $CALO_PROBE, and calo sibling)")]
    ProbeNotFound,
}
