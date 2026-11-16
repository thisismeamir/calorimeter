//! Typed errors for report table generation and regression gate evaluation (spec §13).

use crate::expr::ExprError;
use thiserror::Error;

/// Errors produced during report pivot table construction or gate evaluation (spec §13).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReportError {
    /// No matching records or metric values were found for a gate.
    #[error("no matching records with metric `{metric}` found for {context}")]
    EmptyMetricSet {
        /// Metric name.
        metric: String,
        /// Context (`"observed"` or `"baseline"`).
        context: &'static str,
    },

    /// Baseline value was zero when computing a relative percentage threshold (`> +5%`).
    #[error("baseline value for metric `{metric}` is zero; cannot compute relative percentage change")]
    ZeroBaseline {
        /// Metric name.
        metric: String,
    },

    /// Invalid `fail_if` threshold or expression syntax.
    #[error("invalid gate `fail_if` specification `{raw}`: {reason}")]
    InvalidFailIf {
        /// Raw `fail_if` string.
        raw: String,
        /// Explanation of failure.
        reason: String,
    },

    /// Expression evaluation error in a gate `fail_if` expression.
    #[error("expression error in gate `fail_if`: {0}")]
    Expression(#[from] ExprError),
}
