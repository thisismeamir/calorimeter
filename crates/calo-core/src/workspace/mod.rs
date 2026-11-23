//! Source resolution, clean git worktrees, patch application, and cached builds (spec §8.1–§8.3, §16 R-SAF-2).

pub mod build;
pub mod error;
pub mod patches;
pub mod sources;
pub mod worktree;

pub use build::{
    compute_cache_key, execute_build, hash_build_artifacts, BuildOutcome, BuildTargetContext,
};
pub use error::WorkspaceError;
pub use patches::{check_patchsets, LintIssue};
pub use sources::resolve_sources;
pub use worktree::Worktree;
