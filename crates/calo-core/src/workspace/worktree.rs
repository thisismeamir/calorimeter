//! Clean git worktree management and ordered patch application (spec §8.2, §16 R-SAF-2).

use crate::proc::{CommandRunner, CommandSpec};
use crate::workspace::error::WorkspaceError;
use calo_schema::record::{AppliedPatch, RecordPatchset};
use calo_schema::suite::{PatchApplyConfig, PatchConflictPolicy, SourceDef};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// Managed clean git worktree created under `workdir` without touching the user's checkout (spec §16 R-SAF-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Path to the checked-out, patched worktree directory.
    pub path: PathBuf,
    /// Source repository name.
    pub source_name: String,
    /// Resolved commit SHA.
    pub base_sha: String,
    /// Patchset provenance (`name`, per-patch SHA-256, and resulting git `tree_hash`, spec §8.2 R-PAT-3).
    pub patchset: RecordPatchset,
}

impl Worktree {
    /// Creates a clean worktree for `(source_name, sha, patchset_name, machine_name)` under `workdir`,
    /// cloning/fetching into a local cache first and applying `patches` in order (spec §8.2, R-SAF-2).
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        source_name: &str,
        source_def: &SourceDef,
        sha: &str,
        patchset_name: &str,
        patch_files: &[PathBuf],
        machine_name: &str,
        workdir: &Path,
        apply_cfg: Option<&PatchApplyConfig>,
        runner: &dyn CommandRunner,
    ) -> Result<Self, WorkspaceError> {
        let cache_repo = workdir.join("cache").join(format!("{source_name}.git"));
        let wt_dir = workdir
            .join("worktrees")
            .join(format!("{source_name}-{patchset_name}-{machine_name}"));

        if let Some(parent) = cache_repo.parent() {
            fs::create_dir_all(parent).map_err(|source| WorkspaceError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if let Some(parent) = wt_dir.parent() {
            fs::create_dir_all(parent).map_err(|source| WorkspaceError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        // 1. Clone bare mirror or fetch into cache_repo
        if !cache_repo.exists() {
            let clone_out = runner.run(&CommandSpec::new(
                "git",
                vec![
                    "clone".to_string(),
                    "--bare".to_string(),
                    source_def.git.clone(),
                    cache_repo.display().to_string(),
                ],
            ))?;
            if !clone_out.success() {
                return Err(WorkspaceError::GitFailed {
                    stage: "git clone --bare",
                    stderr: clone_out.stderr,
                });
            }
        } else {
            let fetch_out = runner.run(&CommandSpec::new(
                "git",
                vec![
                    "-C".to_string(),
                    cache_repo.display().to_string(),
                    "fetch".to_string(),
                    "--all".to_string(),
                ],
            ))?;
            if !fetch_out.success() {
                return Err(WorkspaceError::GitFailed {
                    stage: "git fetch",
                    stderr: fetch_out.stderr,
                });
            }
        }

        // 2. Remove stale worktree if present, then `git worktree add --detach`
        if wt_dir.exists() {
            let _ = runner.run(&CommandSpec::new(
                "git",
                vec![
                    "-C".to_string(),
                    cache_repo.display().to_string(),
                    "worktree".to_string(),
                    "remove".to_string(),
                    "--force".to_string(),
                    wt_dir.display().to_string(),
                ],
            ));
            if wt_dir.exists() {
                fs::remove_dir_all(&wt_dir).map_err(|source| WorkspaceError::Io {
                    path: wt_dir.clone(),
                    source,
                })?;
            }
        }

        let wt_out = runner.run(&CommandSpec::new(
            "git",
            vec![
                "-C".to_string(),
                cache_repo.display().to_string(),
                "worktree".to_string(),
                "add".to_string(),
                "--detach".to_string(),
                wt_dir.display().to_string(),
                sha.to_string(),
            ],
        ))?;
        if !wt_out.success() {
            return Err(WorkspaceError::GitFailed {
                stage: "git worktree add",
                stderr: wt_out.stderr,
            });
        }

        // 3. Apply patches in order (never reverse-apply, spec §8.2 R-PAT-2)
        let strip = apply_cfg.and_then(|c| c.strip).unwrap_or(1);
        let on_conflict = apply_cfg
            .and_then(|c| c.on_conflict)
            .unwrap_or(PatchConflictPolicy::Fail);

        let mut applied = Vec::with_capacity(patch_files.len());
        for patch_path in patch_files {
            let patch_bytes = fs::read(patch_path).map_err(|source| WorkspaceError::Io {
                path: patch_path.clone(),
                source,
            })?;
            let patch_sha = sha256_hex(&patch_bytes);

            let apply_out = runner.run(
                &CommandSpec::new(
                    "git",
                    vec![
                        "apply".to_string(),
                        format!("-p{strip}"),
                        patch_path.display().to_string(),
                    ],
                )
                .with_cwd(&wt_dir),
            )?;

            if !apply_out.success() {
                match on_conflict {
                    PatchConflictPolicy::Fail => {
                        return Err(WorkspaceError::PatchConflict {
                            patch: patch_path.display().to_string(),
                            stderr: apply_out.stderr,
                        });
                    }
                    PatchConflictPolicy::Skip => continue,
                }
            }

            let file_label = patch_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("patch")
                .to_string();
            applied.push(AppliedPatch {
                file: file_label,
                sha256: patch_sha,
            });
        }

        // 4. Stage all changes and record resulting git tree hash (`git write-tree`, spec §8.2 R-PAT-3)
        let add_out = runner.run(
            &CommandSpec::new("git", vec!["add".to_string(), "-A".to_string()]).with_cwd(&wt_dir),
        )?;
        if !add_out.success() {
            return Err(WorkspaceError::GitFailed {
                stage: "git add -A",
                stderr: add_out.stderr,
            });
        }

        let tree_out = runner
            .run(&CommandSpec::new("git", vec!["write-tree".to_string()]).with_cwd(&wt_dir))?;
        if !tree_out.success() {
            return Err(WorkspaceError::GitFailed {
                stage: "git write-tree",
                stderr: tree_out.stderr,
            });
        }
        let tree_hash = tree_out.stdout.trim().to_string();

        Ok(Self {
            path: wt_dir,
            source_name: source_name.to_string(),
            base_sha: sha.to_string(),
            patchset: RecordPatchset {
                name: patchset_name.to_string(),
                patches: applied,
                tree_hash,
            },
        })
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
