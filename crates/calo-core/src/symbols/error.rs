//! Typed errors for symbol rule compilation and normalization (spec §12.1, §14).

use thiserror::Error;

/// Errors produced when compiling `symbols.fold` or `symbols.classes` regex rules (spec §12.1, §14).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SymbolError {
    /// A regex pattern in `symbols.fold` or `symbols.classes` failed to compile.
    #[error("invalid regex in symbols.{kind}[{index}] (`{pattern}`): {message}")]
    InvalidRegex {
        /// Rule section (`"fold"` or `"classes"`).
        kind: &'static str,
        /// 0-based rule index in the configuration list.
        index: usize,
        /// Raw regex pattern string.
        pattern: String,
        /// Regex compiler error message.
        message: String,
    },
}
