//! Build cache key computation, cached/fresh execution, and artifact SHA-256 hashing (spec §8.3).

use crate::expr::{interpolate, Env, Value};
use crate::plan::selector::glob_match;
use crate::proc::{CommandRunner, CommandSpec};
use crate::workspace::error::WorkspaceError;
use crate::workspace::worktree::sha256_hex;
use calo_schema::common::{ScalarValue, Warning, WarningCode};
use calo_schema::suite::{BuildDef, RebuildPolicy};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Inputs identifying a unique build cache key (spec §8.3 R-BLD-1, R-BLD-4, R-BLD-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTargetContext {
    /// Resolved source SHAs (`source_name -> sha`).
    pub source_shas: BTreeMap<String, String>,
    /// Resulting git tree hash of the applied patch set.
    pub patch_tree_hash: String,
    /// Machine CPU model string.
    pub cpu_model: String,
    /// Target GPU architecture string (e.g. `"sm_80"`, `"gfx90a"`, `"none"`).
    pub gpu_arch: String,
    /// Compiler toolchain identity string.
    pub toolchain: String,
    /// MPI library identity string.
    pub mpi_identity: String,
}

/// Outcome of executing (or reusing) a build (spec §8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutcome {
    /// Computed SHA-256 cache key.
    pub cache_key: String,
    /// Directory containing the built artifacts.
    pub artifact_dir: PathBuf,
    /// `true` if an existing cached build directory was reused.
    pub cache_hit: bool,
    /// SHA-256 hashes of declared artifact outputs (`relative_path -> sha256`, spec §8.3 R-BLD-2).
    pub artifact_hashes: BTreeMap<String, String>,
    /// Warnings emitted (e.g. `WarningCode::BuildNondeterministic`).
    pub warnings: Vec<Warning>,
}

/// Computes the deterministic SHA-256 `cache_key` per spec §8.3 R-BLD-1.
pub fn compute_cache_key(ctx: &BuildTargetContext, build: &BuildDef) -> String {
    let mut hasher = Sha256::new();
    for (src, sha) in &ctx.source_shas {
        hasher.update(b"src:");
        hasher.update(src.as_bytes());
        hasher.update(b"=");
        hasher.update(sha.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"patch_tree:");
    hasher.update(ctx.patch_tree_hash.as_bytes());
    hasher.update(b"\ncpu:");
    hasher.update(ctx.cpu_model.as_bytes());
    hasher.update(b"\ngpu_arch:");
    hasher.update(ctx.gpu_arch.as_bytes());
    hasher.update(b"\ntoolchain:");
    hasher.update(ctx.toolchain.as_bytes());
    hasher.update(b"\nmpi:");
    hasher.update(ctx.mpi_identity.as_bytes());
    hasher.update(b"\nflags:");
    hasher.update(build.flags.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\nrun:");
    hasher.update(build.run.as_bytes());
    hasher.update(b"\n");
    format!("{:x}", hasher.finalize())
}

struct BuildEnv<'a> {
    params: &'a BTreeMap<String, ScalarValue>,
}

impl Env for BuildEnv<'_> {
    fn get(&self, path: &str) -> Option<Value> {
        let key = path.strip_prefix("params.").unwrap_or(path);
        self.params.get(key).map(Value::from_scalar)
    }
}

/// Executes `build` inside `worktree_dir` (or reuses cached artifacts when `rebuild: cache`),
/// hashes declared `build.artifacts`, and flags `build_nondeterministic` if a forced rebuild
/// produces different artifact hashes (spec §8.3 R-BLD-1, R-BLD-2).
#[allow(clippy::too_many_arguments)]
pub fn execute_build(
    build_name: &str,
    build: &BuildDef,
    ctx: &BuildTargetContext,
    params: &BTreeMap<String, ScalarValue>,
    machine_modules: &[String],
    worktree_dir: &Path,
    builds_cache_root: &Path,
    runner: &dyn CommandRunner,
) -> Result<BuildOutcome, WorkspaceError> {
    let cache_key = compute_cache_key(ctx, build);
    let artifact_dir = builds_cache_root.join(format!("{build_name}-{}", &cache_key[..16]));
    let marker_path = artifact_dir.join(".calo_hashes.yaml");

    let policy = build.rebuild.unwrap_or(RebuildPolicy::Cache);

    if policy == RebuildPolicy::Cache && marker_path.exists() {
        if let Ok(raw) = fs::read_to_string(&marker_path) {
            if let Ok(cached_hashes) = calo_schema::yaml::from_str::<BTreeMap<String, String>>(&raw)
            {
                return Ok(BuildOutcome {
                    cache_key,
                    artifact_dir,
                    cache_hit: true,
                    artifact_hashes: cached_hashes,
                    warnings: Vec::new(),
                });
            }
        }
    }

    // Read previous hashes before forced rebuild to detect nondeterminism (R-BLD-2)
    let previous_hashes: Option<BTreeMap<String, String>> = if marker_path.exists() {
        fs::read_to_string(&marker_path)
            .ok()
            .and_then(|s| calo_schema::yaml::from_str(&s).ok())
    } else {
        None
    };

    fs::create_dir_all(&artifact_dir).map_err(|source| WorkspaceError::Io {
        path: artifact_dir.clone(),
        source,
    })?;

    // Prefix `module load ...` lines from build.modules (or machine_modules fallback)
    let modules = if !build.modules.is_empty() {
        &build.modules
    } else {
        machine_modules
    };
    let mut script = String::new();
    for m in modules {
        script.push_str(&format!("module load {m}\n"));
    }

    let env = BuildEnv { params };
    let rendered_run =
        interpolate(&build.run, &env).map_err(|source| WorkspaceError::Expression {
            build: build_name.to_string(),
            source,
        })?;
    script.push_str(&rendered_run);

    let out = runner.run(
        &CommandSpec::new("sh", vec!["-c".to_string(), script])
            .with_cwd(worktree_dir)
            .with_env("CALO_BUILD_DIR", artifact_dir.display().to_string()),
    )?;

    if !out.success() {
        return Err(WorkspaceError::BuildFailed {
            build: build_name.to_string(),
            code: out.exit_code,
            stderr: out.stderr,
        });
    }

    let artifact_hashes = hash_build_artifacts(worktree_dir, &build.artifacts)?;

    let mut warnings = Vec::new();
    if let Some(prev) = previous_hashes {
        if prev != artifact_hashes {
            warnings.push(Warning {
                code: WarningCode::BuildNondeterministic,
                message: format!("build `{build_name}` artifact hashes changed across rebuild"),
            });
        }
    }

    if let Ok(hashes_yaml) = calo_schema::yaml::to_string(&artifact_hashes) {
        let _ = fs::write(&marker_path, hashes_yaml);
    }

    Ok(BuildOutcome {
        cache_key,
        artifact_dir,
        cache_hit: false,
        artifact_hashes,
        warnings,
    })
}

/// Walks `base_dir` for all files matching any glob in `patterns` and returns `rel_path -> sha256` (spec §8.3).
pub fn hash_build_artifacts(
    base_dir: &Path,
    patterns: &[String],
) -> Result<BTreeMap<String, String>, WorkspaceError> {
    let mut hashes = BTreeMap::new();
    if patterns.is_empty() || !base_dir.exists() {
        return Ok(hashes);
    }

    let mut all_files = Vec::new();
    collect_files_recursive(base_dir, base_dir, &mut all_files)?;

    for (rel_str, full_path) in all_files {
        if patterns.iter().any(|pat| glob_match(pat, &rel_str)) {
            let bytes = fs::read(&full_path).map_err(|source| WorkspaceError::Io {
                path: full_path,
                source,
            })?;
            hashes.insert(rel_str, sha256_hex(&bytes));
        }
    }

    Ok(hashes)
}

fn collect_files_recursive(
    root: &Path,
    current: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), WorkspaceError> {
    let entries = fs::read_dir(current).map_err(|source| WorkspaceError::Io {
        path: current.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            path: current.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            collect_files_recursive(root, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}
