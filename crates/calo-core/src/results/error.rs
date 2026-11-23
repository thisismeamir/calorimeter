//! Typed errors for results directory creation, manifest persistence, and locking (spec §4, §15).

use calo_schema::SchemaError;
use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by `calo-core::results` (spec §4, §15).
#[derive(Debug, Error)]
pub enum ResultsError {
    /// Underlying filesystem I/O error.
    #[error("I/O error at `{path}`: {source}")]
    Io {
        /// Path involved in the failed I/O operation.
        path: PathBuf,
        /// Underlying OS I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Schema serialization or validation error.
    #[error("schema error: {0}")]
    Schema(#[from] SchemaError),

    /// Timed out waiting for the `manifest.yaml` lock file.
    #[error("timed out waiting for manifest lock `{path}`")]
    LockTimeout {
        /// Lock file path.
        path: PathBuf,
    },
}
