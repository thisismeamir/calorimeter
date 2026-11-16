//! Typed errors for YAML loading, include resolution, and layered `--set` overrides (spec §5.1).

use std::path::PathBuf;
use thiserror::Error;

/// Errors produced during YAML loading, `include:` expansion, or layered resolution (spec §5.1).
#[derive(Debug, Error)]
pub enum LoadError {
    /// I/O error reading a YAML file from disk.
    #[error("failed to read `{path}`: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying OS I/O error.
        #[source]
        source: std::io::Error,
    },

    /// YAML syntax or merge-key error with optional line/column location.
    #[error("YAML parse error in `{path}`{loc}: {message}")]
    YamlSyntax {
        /// File path where the error occurred.
        path: PathBuf,
        /// Formatted `:<line>:<col>` if available from the parser.
        loc: String,
        /// Line number (1-based) if reported by the parser.
        line: Option<usize>,
        /// Column number (1-based) if reported by the parser.
        column: Option<usize>,
        /// Parser error message.
        message: String,
    },

    /// Cyclic `include:` chain detected (spec §5.1).
    #[error("include cycle detected: {chain}")]
    IncludeCycle {
        /// Formatted `a.yaml -> b.yaml -> a.yaml` cycle chain.
        chain: String,
    },

    /// `include:` key was not a string or sequence of strings.
    #[error("invalid `include:` in `{path}`: expected a list of file paths")]
    InvalidIncludeFormat {
        /// File containing the malformed `include:` key.
        path: PathBuf,
    },

    /// Malformed CLI `--set key=value` argument (spec §5.1).
    #[error("invalid `--set` argument `{raw}`: {reason}")]
    InvalidCliSet {
        /// Raw `--set` string passed on the CLI.
        raw: String,
        /// Reason the `--set` expression was rejected.
        reason: String,
    },

    /// Deserialization into target typed configuration struct failed.
    #[error("configuration schema error: {0}")]
    Deserialization(String),
}

impl LoadError {
    /// Constructs a `LoadError::YamlSyntax` extracting line/column metadata from `calo_schema::yaml::Error`.
    pub fn from_yaml_err(path: impl Into<PathBuf>, err: calo_schema::yaml::Error) -> Self {
        let path = path.into();
        let (line, column, loc) = if let Some(loc) = err.location() {
            (
                Some(loc.line()),
                Some(loc.column()),
                format!(":{}:{}", loc.line(), loc.column()),
            )
        } else {
            (None, None, String::new())
        };
        Self::YamlSyntax {
            path,
            loc,
            line,
            column,
            message: err.to_string(),
        }
    }
}
