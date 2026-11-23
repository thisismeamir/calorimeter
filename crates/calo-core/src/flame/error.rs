//! Typed errors for folded-stack parsing and flamegraph rendering (spec §13).

use thiserror::Error;

/// Errors produced by `calo-core::flame` (spec §13).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FlameError {
    /// Malformed folded stack line (expected `<f1>;<f2> <weight>`).
    #[error("invalid folded stack at line {line}: {reason}")]
    InvalidFoldedLine {
        /// 1-based line number.
        line: usize,
        /// Explanation.
        reason: String,
    },

    /// Invalid highlight regex in `FlameOpts`.
    #[error("invalid highlight regex `{pattern}`: {message}")]
    InvalidHighlightRegex {
        /// Regex pattern string.
        pattern: String,
        /// Compiler error message.
        message: String,
    },
}
