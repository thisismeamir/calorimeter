//! Typed errors for SVG plot rendering (spec §13).

use thiserror::Error;

/// Errors produced by `calo-core::plot` (spec §13).
#[derive(Debug, Error, Clone, PartialEq)]
pub enum PlotError {
    /// Log or log2 scale encountered a non-positive value (Task 19).
    #[error("non-positive value {value} in series `{series}` on {axis}-axis with log scale")]
    NonPositiveLogValue {
        /// Series name.
        series: String,
        /// Axis (`"x"` or `"y"`).
        axis: &'static str,
        /// Offending value.
        value: f64,
    },

    /// Required column was missing from the input `Frame`.
    #[error("missing required column `{column}` for plot `{plot_type}`")]
    MissingColumn {
        /// Plot type name.
        plot_type: String,
        /// Missing column name.
        column: String,
    },

    /// Plot type is deferred (`roofline`, `timeline`, `gpu_timeline`) and not yet implemented.
    #[error("plot type `{plot_type}` is not yet implemented")]
    NotYetImplemented {
        /// Plot type name.
        plot_type: String,
    },

    /// Unknown plot type name.
    #[error("unknown plot type `{plot_type}`")]
    UnknownPlotType {
        /// Plot type name.
        plot_type: String,
    },
}
