//! Typed errors for service registration, dependency resolution, and execution (spec §12.2, §12.4).

use crate::expr::ExprError;
use thiserror::Error;

/// Errors returned by `calo-core::services` (spec §12.2, §12.4).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// Service declares a `python:` entrypoint, which is not supported in-process (spec §12.4).
    #[error("external Python service `{service}` (`{entrypoint}`) is not supported in this build (python_unavailable)")]
    PythonUnsupported {
        /// Service name.
        service: String,
        /// Declared `pkg.module:function` entrypoint.
        entrypoint: String,
    },

    /// Service declares a `python:` entrypoint, which is not available in-process (spec §12.4).
    #[error("external Python service `{service}` is not available because (`{reason}`)")]
    PythonUnavailable {
        /// Service name.
        service: String,
        // Failure Reason
        reason: String,
    },

    /// Unknown service name requested from the registry (spec §12.2 R-SVC-2).
    #[error("unknown service `{name}`")]
    UnknownService {
        /// Requested service name.
        name: String,
    },

    /// Dependency cycle detected in the service graph (spec §12.2 R-SVC-2).
    #[error("service dependency cycle detected: {cycle}")]
    DependencyCycle {
        /// Formatted cycle path.
        cycle: String,
    },

    /// Required input column was missing from the input `Frame`.
    #[error("missing required input column `{column}` for service `{service}`")]
    MissingColumn {
        /// Service name.
        service: String,
        /// Missing column name.
        column: String,
    },

    /// Column lengths did not match when constructing or mutating a `Frame`.
    #[error("column length mismatch for `{column}`: expected {expected} rows, got {actual}")]
    RowCountMismatch {
        /// Column name.
        column: String,
        /// Expected row count.
        expected: usize,
        /// Actual row count.
        actual: usize,
    },

    /// Invalid or incompatible column type in a `Frame`.
    #[error("column `{column}` type error: expected {expected}, found {actual}")]
    ColumnType {
        /// Column name.
        column: String,
        /// Expected type description.
        expected: &'static str,
        /// Actual type description.
        actual: &'static str,
    },

    /// Invalid configuration or empty data passed to a statistical or scaling service.
    #[error("invalid input for service `{service}`: {reason}")]
    InvalidInput {
        /// Service name.
        service: String,
        /// Explanation.
        reason: String,
    },

    /// Error parsing or evaluating an expression-defined service (spec §12.2 R-SVC-5).
    #[error("expression error in service `{service}`: {source}")]
    Expression {
        /// Service name.
        service: String,
        /// Underlying expression error.
        #[source]
        source: ExprError,
    },

    /// Service is registered as a stub and requires engineer-authored symbolization or clock alignment (Task 16).
    #[error(
        "service `{service}` is not yet implemented (requires symbolization or clock alignment)"
    )]
    NotYetImplemented {
        /// Service name.
        service: String,
    },
}
