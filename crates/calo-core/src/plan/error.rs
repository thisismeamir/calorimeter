//! Typed errors for benchmark matrix expansion, selectors, and cost estimation (spec §5.3, §8.5).

use crate::expr::ExprError;
use thiserror::Error;

/// Errors produced during suite matrix expansion or selector evaluation (spec §5.3, §8.5).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// An axis in `axes:` had an empty list of values.
    #[error("benchmark `{benchmark}` axis `{axis}` has an empty value list")]
    EmptyAxis {
        /// Benchmark name.
        benchmark: String,
        /// Axis key name.
        axis: String,
    },

    /// Missing default value for an axis when `expand: defaults` is used.
    #[error("benchmark `{benchmark}` axis `{axis}` is missing a default value required by `expand: defaults`")]
    MissingDefault {
        /// Benchmark name.
        benchmark: String,
        /// Axis key name.
        axis: String,
    },

    /// Partial `include` pattern omitted a required axis when `expand: none` is active (spec §8.5 step 3).
    #[error("benchmark `{benchmark}` has `expand: none`, so `include` pattern must specify axis `{axis}`")]
    IncompleteIncludeForNone {
        /// Benchmark name.
        benchmark: String,
        /// Missing axis name.
        axis: String,
    },

    /// Dependency cycle detected among `derive` expressions (spec §8.5 step 5).
    #[error("derive dependency cycle in benchmark `{benchmark}` involving: {cycle}")]
    DeriveCycle {
        /// Benchmark name.
        benchmark: String,
        /// Comma-separated cycle keys.
        cycle: String,
    },

    /// Benchmark `from:` references an unknown or later-declared benchmark (spec §8.5 step 6).
    #[error("benchmark `{benchmark}` derives `from` unknown benchmark `{from}`")]
    UnknownFromBenchmark {
        /// Benchmark name.
        benchmark: String,
        /// Referenced `from` benchmark name.
        from: String,
    },

    /// Benchmark references an unknown machine or `machine/node_class` (spec §7, §8.5).
    #[error("benchmark `{benchmark}` references unknown machine `{machine}`")]
    UnknownMachine {
        /// Benchmark name.
        benchmark: String,
        /// Target machine string.
        machine: String,
    },

    /// Expression parse or evaluation error in `derive` or `where`.
    #[error("expression error in benchmark `{benchmark}` ({context}): {source}")]
    Expression {
        /// Benchmark name.
        benchmark: String,
        /// Context description (e.g. `"derive.N"` or `"where"`).
        context: String,
        /// Underlying expression error.
        #[source]
        source: ExprError,
    },

    /// Invalid timeout string format on a step (expected `HH:MM:SS`).
    #[error("invalid timeout `{timeout}` on step `{step}`: expected HH:MM:SS")]
    InvalidTimeout {
        /// Step name.
        step: String,
        /// Raw timeout string.
        timeout: String,
    },
}
