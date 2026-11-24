//! Run ordering (`interleaved`, `random`, `sequential`), execution loop, and dry-run planning (spec §15).

use crate::aggregate::aggregate_run;
use crate::exec::adapter::{Adapter, JobState, StagedRun};
use crate::exec::error::ExecError;
use crate::exec::runspec::{build_launcher_argv, build_runspec, render_step_files};
use crate::plan::{parse_timeout_hours, PinnedRun};
use crate::results::{plan_resume, update_manifest_run, write_atomic_bytes, ResultsDir};
use calo_schema::common::Status;
use calo_schema::inventory::Inventory;
use calo_schema::record::{Record, RenderedFile};
use calo_schema::suite::{OnFailurePolicy, RunOrder, Suite};
use calo_schema::{from_yaml_str, to_yaml_string};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Dry-run preview entry for `calo run --dry-run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRunEntry {
    /// Run identifier.
    pub run_id: String,
    /// Full argv vector.
    pub argv: Vec<String>,
    /// Path where `runspec.yaml` would be written.
    pub runspec_path: PathBuf,
    /// Environment variables on the `RunSpec`.
    pub env: BTreeMap<String, String>,
}

/// Orders `runs` according to `order` (`interleaved`, `random`, `sequential`, spec §15).
pub fn order_runs(runs: &[PinnedRun], order: RunOrder, seed: u64) -> Vec<PinnedRun> {
    match order {
        RunOrder::Sequential => runs.to_vec(),
        RunOrder::Interleaved => {
            // Group by (benchmark, params), then round-robin across repetitions
            let mut groups: BTreeMap<(String, String), Vec<PinnedRun>> = BTreeMap::new();
            for r in runs {
                let param_key: String = r
                    .params
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                groups
                    .entry((r.benchmark.clone(), param_key))
                    .or_default()
                    .push(r.clone());
            }
            let max_len = groups.values().map(Vec::len).max().unwrap_or(0);
            let mut out = Vec::with_capacity(runs.len());
            for idx in 0..max_len {
                for vec in groups.values() {
                    if let Some(item) = vec.get(idx) {
                        out.push(item.clone());
                    }
                }
            }
            out
        }
        RunOrder::Random => {
            let mut out = runs.to_vec();
            let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
            for i in (1..out.len()).rev() {
                state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = state;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                let j = (z as usize) % (i + 1);
                out.swap(i, j);
            }
            out
        }
    }
}

/// Summary returned by `execute_suite_runs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunExecutionSummary {
    /// Total runs scheduled (after resume filtering).
    pub scheduled: usize,
    /// Number of runs that succeeded (`Status::Ok`).
    pub succeeded: usize,
    /// Number of runs that failed or timed out.
    pub failed: usize,
    /// Dry-run entries (populated when `dry_run == true`).
    pub dry_run_entries: Vec<DryRunEntry>,
}

/// Executes (or dry-runs) expanded runs via `adapter`, updating `manifest.yaml` after every run (spec §14, §15).
#[allow(clippy::too_many_arguments)]
pub fn execute_suite_runs(
    expanded: &[PinnedRun],
    suite: &Suite,
    inventory: &Inventory,
    results: &ResultsDir,
    suite_root: &Path,
    probe_bin: &Path,
    adapter: &dyn Adapter,
    force: bool,
    dry_run: bool,
    seed: u64,
) -> Result<RunExecutionSummary, ExecError> {
    execute_suite_runs_with_cwds(
        expanded,
        suite,
        inventory,
        results,
        suite_root,
        &BTreeMap::new(),
        probe_bin,
        adapter,
        force,
        dry_run,
        seed,
    )
}

/// Same as [`execute_suite_runs`], using `run_cwds[run_id]` when a run has a prepared worktree.
#[allow(clippy::too_many_arguments)]
pub fn execute_suite_runs_with_cwds(
    expanded: &[PinnedRun],
    suite: &Suite,
    inventory: &Inventory,
    results: &ResultsDir,
    suite_root: &Path,
    run_cwds: &BTreeMap<String, PathBuf>,
    probe_bin: &Path,
    adapter: &dyn Adapter,
    force: bool,
    dry_run: bool,
    seed: u64,
) -> Result<RunExecutionSummary, ExecError> {
    let to_run = plan_resume(expanded, results, force);
    let order = suite
        .options
        .as_ref()
        .and_then(|o| o.order)
        .unwrap_or(RunOrder::Interleaved);
    let ordered = order_runs(&to_run, order, seed);
    let on_failure = suite
        .options
        .as_ref()
        .and_then(|o| o.on_failure)
        .unwrap_or(OnFailurePolicy::Record);

    let mut summary = RunExecutionSummary {
        scheduled: ordered.len(),
        succeeded: 0,
        failed: 0,
        dry_run_entries: Vec::new(),
    };

    for run in ordered {
        let bench = suite
            .benchmarks
            .iter()
            .find(|b| b.name == run.benchmark)
            .ok_or_else(|| ExecError::MissingTarget {
                kind: "benchmark",
                name: run.benchmark.clone(),
            })?;

        let step_name = bench
            .steps
            .iter()
            .find(|s| {
                suite
                    .steps
                    .get(*s)
                    .is_some_and(|st| st.measured == Some(true))
                    || bench.measure.contains(*s)
            })
            .or_else(|| bench.steps.first())
            .ok_or_else(|| ExecError::MissingTarget {
                kind: "step",
                name: bench.name.clone(),
            })?;

        let step = suite
            .steps
            .get(step_name)
            .ok_or_else(|| ExecError::MissingTarget {
                kind: "step",
                name: step_name.clone(),
            })?;

        let run_dir = results.root.join("records").join(&run.run_id);
        let runspec_path = run_dir.join("runspec.yaml");
        let inputs_dir = run_dir.join("inputs");

        let rendered_files = if dry_run {
            step.files
                .keys()
                .map(|name| {
                    let out_file = inputs_dir.join(format!("{name}.in"));
                    (
                        name.clone(),
                        RenderedFile {
                            path: out_file.display().to_string(),
                            sha256: String::new(),
                        },
                    )
                })
                .collect()
        } else {
            render_step_files(step, &run.params, suite_root, &inputs_dir)?
        };

        let run_cwd = run_cwds
            .get(&run.run_id)
            .map_or(suite_root, PathBuf::as_path);

        let runspec = build_runspec(
            &run,
            suite,
            inventory,
            &rendered_files,
            &run_dir,
            run_cwd,
            None,
        )?;
        let argv = build_launcher_argv(
            probe_bin,
            &runspec_path,
            step,
            inventory,
            &run.machine,
            &run.params,
        )?;

        if dry_run {
            summary.dry_run_entries.push(DryRunEntry {
                run_id: run.run_id.clone(),
                argv,
                runspec_path,
                env: runspec.env.clone(),
            });
            continue;
        }

        let timeout = step
            .timeout
            .as_deref()
            .and_then(|t| parse_timeout_hours(step_name, t).ok())
            .map(|hrs| Duration::from_secs_f64(hrs * 3600.0));

        let staged = adapter.stage(StagedRun {
            run_id: run.run_id.clone(),
            runspec: runspec.clone(),
            runspec_path: runspec_path.clone(),
            argv,
            cwd: run_cwd.to_path_buf(),
            env: runspec.env.clone(),
            timeout,
        })?;

        let handle = adapter.submit(&staged)?;
        let state = adapter.poll(&handle)?;
        adapter.fetch(&handle, &run_dir)?;
        finalize_run_record(&run_dir, &run, &results.suite_hash)?;

        let status = match state {
            JobState::Completed => {
                summary.succeeded += 1;
                Status::Ok
            }
            JobState::TimedOut => {
                summary.failed += 1;
                Status::Timeout
            }
            JobState::Failed { .. }
            | JobState::Cancelled
            | JobState::Pending
            | JobState::Running => {
                summary.failed += 1;
                Status::Failed
            }
        };

        let rec_rel = format!("records/{}/run.yaml", run.run_id);
        update_manifest_run(results, &run.run_id, status, Some(&rec_rel), &[])?;

        if status != Status::Ok && on_failure == OnFailurePolicy::Stop {
            break;
        }
    }

    Ok(summary)
}

/// Aggregates fetched `rank*.yaml` records into `run.yaml` and stamps control-plane
/// identity/environment fields (`suite_hash`, `params`, `benchmark`, `machine`) while
/// leaving raw rank records immutable (spec §10 R-REC-4, R-REC-7).
fn finalize_run_record(run_dir: &Path, run: &PinnedRun, suite_hash: &str) -> Result<(), ExecError> {
    let run_yaml_path = run_dir.join("run.yaml");
    if run_yaml_path.exists() || !run_dir.is_dir() {
        return Ok(());
    }

    let mut rank_files = Vec::new();
    for entry in fs::read_dir(run_dir).map_err(|source| ExecError::Io {
        path: run_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ExecError::Io {
            path: run_dir.to_path_buf(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("rank") && name.ends_with(".yaml") {
            rank_files.push(entry.path());
        }
    }
    if rank_files.is_empty() {
        return Ok(());
    }
    rank_files.sort();

    let mut rank_records = Vec::with_capacity(rank_files.len());
    for rf in &rank_files {
        let raw = fs::read_to_string(rf).map_err(|source| ExecError::Io {
            path: rf.clone(),
            source,
        })?;
        rank_records.push(from_yaml_str::<Record>(&raw)?);
    }

    let Ok(mut agg) = aggregate_run(&rank_records) else {
        return Ok(());
    };

    agg.identity.benchmark = run.benchmark.clone();
    agg.identity.group = run.group.clone();
    agg.identity.params = run.params.clone();
    agg.identity.repeat = Some(run.repeat);
    agg.identity.warmup = Some(run.warmup);
    agg.identity.suite_hash = Some(suite_hash.to_string());
    if let Some(ref linked) = run.linked_from {
        if !agg.identity.linked_runs.contains(linked) {
            agg.identity.linked_runs.push(linked.clone());
        }
    }

    let (machine, node_class) = match run.machine.split_once('/') {
        Some((m, c)) => (m.to_string(), Some(c.to_string())),
        None => (run.machine.clone(), None),
    };
    agg.environment.machine = machine;
    agg.environment.node_class = node_class;

    let yaml = to_yaml_string(&agg)?;
    write_atomic_bytes(&run_yaml_path, yaml.as_bytes())?;
    Ok(())
}
