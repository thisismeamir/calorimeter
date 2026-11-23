//! Results directory layout, SHA-256 suite hashing, atomic manifest writes, and resume checks (spec §4, §14, §15).

use crate::plan::PinnedRun;
use crate::results::error::ResultsError;
use crate::yaml::{to_manifest_resolved, Layered};
use calo_schema::common::{ScalarValue, Status};
use calo_schema::inventory::Inventory;
use calo_schema::manifest::{Manifest, ManifestRun, ManifestSuite};
use calo_schema::record::Record;
use calo_schema::suite::Suite;
use calo_schema::{from_yaml_str, to_yaml_string, SchemaDoc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Sidecar subdirectory kinds under `results/<suite-hash>/` (spec §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarKind {
    /// `series/<run-id>/`
    Series,
    /// `profiles/<run-id>/`
    Profiles,
    /// `mpi/<run-id>/`
    Mpi,
    /// `gpu/<run-id>/`
    Gpu,
}

impl SidecarKind {
    /// Subdirectory name under the suite results root.
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::Series => "series",
            Self::Profiles => "profiles",
            Self::Mpi => "mpi",
            Self::Gpu => "gpu",
        }
    }
}

/// Validity status of an existing run record for `calo run` resume logic (spec §14, §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordValidity {
    /// Record file does not exist or is unreadable.
    Missing,
    /// Record exists, but its `suite_hash` or run parameters do not match.
    Stale,
    /// Record exists, but its `schema:` version is not a supported `calo.record` major.
    SchemaUnsupported,
    /// Record exists, has a supported schema, and matches `suite_hash` and `params`.
    Valid,
}

/// Managed `results/<suite-hash>/` directory handle (spec §4, §15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultsDir {
    /// Root path `results/<suite-hash>/`.
    pub root: PathBuf,
    /// Hex-encoded SHA-256 digest of the canonical resolved suite.
    pub suite_hash: String,
}

/// Computes the deterministic SHA-256 hex digest over the canonical YAML of `suite` (spec §4, §15).
pub fn compute_suite_hash(suite: &Suite) -> Result<String, ResultsError> {
    let canonical_yaml = to_yaml_string(suite)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical_yaml.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

impl ResultsDir {
    /// Computes `suite_hash` and creates `results/<suite-hash>/` along with all standard subdirectories (spec §4).
    pub fn create(
        base_results_dir: impl AsRef<Path>,
        suite: &Suite,
        _inventory: &Inventory,
    ) -> Result<Self, ResultsError> {
        let suite_hash = compute_suite_hash(suite)?;
        let root = base_results_dir.as_ref().join(&suite_hash);

        for sub in [
            "records",
            "series",
            "profiles",
            "mpi",
            "gpu",
            "analysis",
            "reports",
        ] {
            let p = root.join(sub);
            fs::create_dir_all(&p).map_err(|source| ResultsError::Io { path: p, source })?;
        }

        Ok(Self { root, suite_hash })
    }

    /// Opens an existing `ResultsDir` handle without re-hashing if `suite_hash` is already known.
    pub fn from_existing(root: impl Into<PathBuf>, suite_hash: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            suite_hash: suite_hash.into(),
        }
    }

    /// Path to `manifest.yaml` inside this results directory (spec §4, §15).
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.yaml")
    }

    /// Path to a rank record (`records/<run-id>/rank<k>.yaml`) or aggregate (`records/<run-id>/run.yaml`) (spec §4, §10).
    pub fn record_path(&self, run_id: &str, rank: Option<u32>) -> PathBuf {
        let dir = self.root.join("records").join(run_id);
        match rank {
            Some(k) => dir.join(format!("rank{k}.yaml")),
            None => dir.join("run.yaml"),
        }
    }

    /// Path to the sidecar directory `<kind>/<run-id>/` (spec §4).
    pub fn sidecar_dir(&self, kind: SidecarKind, run_id: &str) -> PathBuf {
        self.root.join(kind.dir_name()).join(run_id)
    }

    /// Builds and atomically writes `manifest.yaml` for `layered_suite`, `inventory`, `runs`, and `sources` (spec §15).
    pub fn write_manifest(
        &self,
        layered_suite: &Layered<Suite>,
        inventory: &Inventory,
        runs: &[PinnedRun],
        sources: BTreeMap<String, String>,
        calo_version: &str,
    ) -> Result<(), ResultsError> {
        let mut resolved_map = BTreeMap::new();
        for (k, origin) in to_manifest_resolved(layered_suite) {
            if let Ok(val) = calo_schema::yaml::to_value(origin) {
                resolved_map.insert(k, val);
            }
        }

        let manifest_runs: Vec<ManifestRun> = runs
            .iter()
            .map(|r| ManifestRun {
                run_id: r.run_id.clone(),
                benchmark: r.benchmark.clone(),
                mode: r.mode,
                machine: r.machine.clone(),
                params: r.params.clone(),
                status: Status::Ok,
                linked_runs: r.linked_from.iter().cloned().collect(),
                record: Some(format!("records/{}/run.yaml", r.run_id)),
            })
            .collect();

        let manifest = Manifest {
            schema: Manifest::SCHEMA.to_string(),
            suite: ManifestSuite {
                name: layered_suite.value.suite.name.clone(),
                version: layered_suite.value.suite.version.clone(),
                hash: self.suite_hash.clone(),
            },
            created: chrono::Utc::now().to_rfc3339(),
            calo: calo_version.to_string(),
            resolved: resolved_map,
            inventory_snapshot: inventory.clone(),
            runs: manifest_runs,
            sources,
        };

        let yaml_str = to_yaml_string(&manifest)?;
        write_atomic_bytes(&self.manifest_path(), yaml_str.as_bytes())
    }
}

/// Atomically writes `bytes` to `target_path` via a sibling temporary file, `sync_all`, and rename (spec §10 R-REC-9, §15).
pub fn write_atomic_bytes(target_path: &Path, bytes: &[u8]) -> Result<(), ResultsError> {
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|source| ResultsError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let file_name = target_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp_path = target_path.with_file_name(format!(".{file_name}.tmp.{pid}.{nanos}"));

    let write_res = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(source) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(ResultsError::Io {
            path: tmp_path,
            source,
        });
    }

    if let Err(source) = fs::rename(&tmp_path, target_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(ResultsError::Io {
            path: target_path.to_path_buf(),
            source,
        });
    }

    Ok(())
}

#[derive(Deserialize)]
struct MinimalRecordProbe {
    schema: Option<String>,
    identity: Option<MinimalIdentityProbe>,
}

#[derive(Deserialize)]
struct MinimalIdentityProbe {
    run_id: Option<String>,
    benchmark: Option<String>,
    suite_hash: Option<String>,
    #[serde(default)]
    params: BTreeMap<String, ScalarValue>,
}

/// Checks whether a record file at `path` is valid for resuming `run` under `suite_hash` (spec §14, §18).
pub fn is_valid_record(path: &Path, run: &PinnedRun, suite_hash: &str) -> RecordValidity {
    let Ok(content) = fs::read_to_string(path) else {
        return RecordValidity::Missing;
    };

    let Ok(probe) = calo_schema::yaml::from_str::<MinimalRecordProbe>(&content) else {
        return RecordValidity::Missing;
    };

    let Some(schema_str) = probe.schema else {
        return RecordValidity::SchemaUnsupported;
    };

    // Spec §18: `calo` reads the current (`calo.record/2`) and previous (`calo.record/1`) major.
    if schema_str != Record::SCHEMA && schema_str != "calo.record/1" {
        return RecordValidity::SchemaUnsupported;
    }

    let Some(id) = probe.identity else {
        return RecordValidity::Stale;
    };

    if id.suite_hash.as_deref() != Some(suite_hash) {
        return RecordValidity::Stale;
    }

    if id.benchmark.as_deref() != Some(run.benchmark.as_str()) {
        return RecordValidity::Stale;
    }

    if id.run_id.as_deref() != Some(run.run_id.as_str()) {
        return RecordValidity::Stale;
    }

    if id.params != run.params {
        return RecordValidity::Stale;
    }

    RecordValidity::Valid
}

/// Filters `runs` for `calo run` resume semantics (spec §14):
/// skips runs that already have a valid `run.yaml` (or `rank0.yaml`) unless `force` is `true`.
pub fn plan_resume(runs: &[PinnedRun], results: &ResultsDir, force: bool) -> Vec<PinnedRun> {
    if force {
        return runs.to_vec();
    }

    runs.iter()
        .filter(|run| {
            let agg_path = results.record_path(&run.run_id, None);
            if is_valid_record(&agg_path, run, &results.suite_hash) == RecordValidity::Valid {
                return false;
            }
            let rank0_path = results.record_path(&run.run_id, Some(0));
            is_valid_record(&rank0_path, run, &results.suite_hash) != RecordValidity::Valid
        })
        .cloned()
        .collect()
}

struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_lock(lock_path: &Path, timeout: Duration) -> Result<LockGuard, ResultsError> {
    let start = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{}", std::process::id());
                return Ok(LockGuard {
                    path: lock_path.to_path_buf(),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if start.elapsed() >= timeout {
                    return Err(ResultsError::LockTimeout {
                        path: lock_path.to_path_buf(),
                    });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(source) => {
                return Err(ResultsError::Io {
                    path: lock_path.to_path_buf(),
                    source,
                });
            }
        }
    }
}

/// Idempotently updates (or inserts) a `ManifestRun` entry in `manifest.yaml` under an exclusive lock file (spec §15).
pub fn update_manifest_run(
    results: &ResultsDir,
    run_id: &str,
    status: Status,
    record_rel_path: Option<&str>,
    linked_runs: &[String],
) -> Result<(), ResultsError> {
    let manifest_path = results.manifest_path();
    let lock_path = manifest_path.with_extension("yaml.lock");
    let _guard = acquire_lock(&lock_path, Duration::from_secs(10))?;

    let content = fs::read_to_string(&manifest_path).map_err(|source| ResultsError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let mut manifest: Manifest = from_yaml_str(&content)?;

    if let Some(entry) = manifest.runs.iter_mut().find(|r| r.run_id == run_id) {
        entry.status = status;
        if let Some(rec) = record_rel_path {
            entry.record = Some(rec.to_string());
        }
        for link in linked_runs {
            if !entry.linked_runs.contains(link) {
                entry.linked_runs.push(link.clone());
            }
        }
    }

    let updated = to_yaml_string(&manifest)?;
    write_atomic_bytes(&manifest_path, updated.as_bytes())
}
