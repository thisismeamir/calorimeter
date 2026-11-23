//! Typed errors for source pinning, worktree creation, patch application, and builds (spec §8.1–§8.3).

use crate::expr::ExprError;
use crate::proc::ProcError;
use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by `calo-core::workspace` (spec §8.1–§8.3).
#[derive(Debug, Error)]
pub enum WorkspaceError {
    /// Filesystem I/O failure.
    #[error("I/O error at `{path}`: {source}")]
    Io {
        /// File or directory path.
        path: PathBuf,
        /// Underlying OS I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Subprocess execution failure.
    #[error("process error: {0}")]
    Proc(#[from] ProcError),

    /// Failed to resolve a source `rev` to a commit SHA (spec §8.1 R-SRC-1).
    #[error("failed to resolve source `{source_name}` rev `{rev}`: {reason}")]
    SourceResolution {
        /// Source name in `suite.yaml`.
        source_name: String,
        /// Requested tag, branch, or SHA.
        rev: String,
        /// Detailed reason.
        reason: String,
    },

    /// Git command failed during clone, fetch, worktree, or tree-hash operations.
    #[error("git command failed during {stage}: {stderr}")]
    GitFailed {
        /// Operation stage.
        stage: &'static str,
        /// Captured stderr.
        stderr: String,
    },

    /// Patch file failed to apply cleanly under `on_conflict: fail` (spec §8.2).
    #[error("patch `{patch}` failed to apply: {stderr}")]
    PatchConflict {
        /// Patch filename.
        patch: String,
        /// Captured `git apply` stderr.
        stderr: String,
    },

    /// Build script exited with non-zero status (spec §8.3).
    #[error("build `{build}` failed with exit code {code:?}: {stderr}")]
    BuildFailed {
        /// Build name.
        build: String,
        /// Process exit code.
        code: Option<i32>,
        /// Captured stderr.
        stderr: String,
    },

    /// Expression interpolation error in `build.run`.
    #[error("expression error in build `{build}`: {source}")]
    Expression {
        /// Build name.
        build: String,
        /// Underlying expression error.
        #[source]
        source: ExprError,
    },
}
