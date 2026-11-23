//! Typed errors for Parquet export and schema validation (spec §0.2, §10 R-REC-2).

use crate::table::TableError;
use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by `calo-core::export` (spec §0.2, §10 R-REC-2).
#[derive(Debug, Error)]
pub enum ExportError {
    /// Underlying I/O error.
    #[error("I/O error at `{path}`: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// OS I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Error reading `calo.table/1` sidecar.
    #[error("table reader error: {0}")]
    Table(#[from] TableError),

    /// Schema validation error at a specific JSON-pointer path.
    #[error("schema validation error at `{path}`: {message}")]
    SchemaValidation {
        /// JSON-pointer path in the YAML document.
        path: String,
        /// Validation failure description.
        message: String,
    },

    /// Parquet encoding/writer error.
    #[error("parquet export error: {0}")]
    Parquet(String),
}
