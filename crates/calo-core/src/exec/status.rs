//! `calo status` and `calo cancel` orchestration over `manifest.yaml` and `Adapter` (spec §14).

use crate::exec::adapter::{Adapter, JobHandle, JobState};
use crate::exec::error::ExecError;
use crate::plan::{matches, RunDescriptor, Selector};
use crate::results::{update_manifest_run, ResultsDir};
use calo_schema::common::{Status, TiersActive};
use calo_schema::from_yaml_str;
use calo_schema::manifest::Manifest;
use serde::Serialize;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

/// Per-run status row emitted by `calo status` (spec §14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusRow {
    /// Run identifier.
    pub run_id: String,
    /// Live or terminal state string (`done`, `failed`, `running`, `pending`, `timeout`, `cancelled`).
    pub state: String,
    /// Target machine.
    pub machine: String,
    /// Elapsed wall time or `"-"` if unavailable.
    pub elapsed: String,
    /// Allocated node(s) or `"-"` if unavailable.
    pub nodes: String,
    /// Discrepancy note if manifest and adapter disagree (e.g. after a crash).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discrepancy: Option<String>,
}

/// Summary counts (`done/failed/running/pending`) for `calo status` (spec §14).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct StatusSummary {
    /// Completed runs.
    pub done: usize,
    /// Failed, timed-out, or cancelled runs.
    pub failed: usize,
    /// Currently running jobs.
    pub running: usize,
    /// Pending / queued jobs.
    pub pending: usize,
}

/// Complete output of `collect_status` (spec §14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusOutput {
    /// Per-run status rows.
    pub rows: Vec<StatusRow>,
    /// Aggregated summary counts.
    pub summary: StatusSummary,
}

/// Outcome of `cancel_runs` (spec §14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelReport {
    /// Run IDs for which `Adapter::cancel` was invoked.
    pub cancelled_runs: Vec<String>,
}

/// Queries `manifest.yaml` and live `adapter` states for all runs, reporting any crash discrepancies (spec §14).
pub fn collect_status(
    results: &ResultsDir,
    adapter: &dyn Adapter,
    live_handles: &std::collections::BTreeMap<String, JobHandle>,
) -> Result<StatusOutput, ExecError> {
    let manifest_path = results.manifest_path();
    let raw = fs::read_to_string(&manifest_path).map_err(|source| ExecError::Io {
        path: manifest_path,
        source,
    })?;
    let manifest: Manifest = from_yaml_str(&raw)?;

    let mut rows = Vec::with_capacity(manifest.runs.len());
    let mut summary = StatusSummary::default();

    for entry in &manifest.runs {
        let rec_exists = entry
            .record
            .as_ref()
            .is_some_and(|rel| results.root.join(rel).exists());

        let (state_str, discrepancy) = if let Some(handle) = live_handles.get(&entry.run_id) {
            match adapter.poll(handle)? {
                JobState::Completed => {
                    summary.done += 1;
                    ("done".to_string(), None)
                }
                JobState::Running => {
                    summary.running += 1;
                    ("running".to_string(), None)
                }
                JobState::Pending => {
                    summary.pending += 1;
                    let disc = if !rec_exists && entry.status == Status::Ok {
                        Some(
                            "manifest missing active state; adapter reports pending/gone"
                                .to_string(),
                        )
                    } else {
                        None
                    };
                    ("pending".to_string(), disc)
                }
                JobState::Failed { .. } => {
                    summary.failed += 1;
                    let disc = if entry.status == Status::Ok && !rec_exists {
                        Some("crash discrepancy: manifest has no terminal record, adapter reports failed/gone".to_string())
                    } else {
                        None
                    };
                    ("failed".to_string(), disc)
                }
                JobState::TimedOut => {
                    summary.failed += 1;
                    ("timeout".to_string(), None)
                }
                JobState::Cancelled => {
                    summary.failed += 1;
                    ("cancelled".to_string(), None)
                }
            }
        } else if rec_exists {
            match entry.status {
                Status::Ok => {
                    summary.done += 1;
                    ("done".to_string(), None)
                }
                Status::Timeout => {
                    summary.failed += 1;
                    ("timeout".to_string(), None)
                }
                Status::Failed | Status::Invalid => {
                    summary.failed += 1;
                    ("failed".to_string(), None)
                }
            }
        } else {
            // No live handle and no record file on disk
            summary.pending += 1;
            let disc = if entry.status != Status::Ok {
                Some("manifest non-terminal, adapter reports no active job".to_string())
            } else {
                None
            };
            ("pending".to_string(), disc)
        };

        rows.push(StatusRow {
            run_id: entry.run_id.clone(),
            state: state_str,
            machine: entry.machine.clone(),
            elapsed: "-".to_string(),
            nodes: "-".to_string(),
            discrepancy,
        });
    }

    Ok(StatusOutput { rows, summary })
}

/// Formats `StatusOutput` as an aligned table plus a one-line summary (spec §14).
pub fn format_status_table(out: &StatusOutput) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{:<40} {:<10} {:<16} {:<10} NODES",
        "RUN_ID", "STATE", "MACHINE", "ELAPSED"
    );
    for r in &out.rows {
        let _ = writeln!(
            s,
            "{:<40} {:<10} {:<16} {:<10} {}",
            r.run_id, r.state, r.machine, r.elapsed, r.nodes
        );
        if let Some(d) = &r.discrepancy {
            let _ = writeln!(s, "  ! discrepancy: {d}");
        }
    }
    let _ = writeln!(
        s,
        "\nSummary: {} done / {} failed / {} running / {} pending",
        out.summary.done, out.summary.failed, out.summary.running, out.summary.pending
    );
    s
}

/// Cancels matching non-terminal runs via `adapter`, updates `manifest.yaml`, and never deletes records (spec §14).
pub fn cancel_runs(
    results: &ResultsDir,
    adapter: &dyn Adapter,
    selector: &Selector,
    live_handles: &std::collections::BTreeMap<String, JobHandle>,
) -> Result<CancelReport, ExecError> {
    let manifest_path = results.manifest_path();
    let raw = fs::read_to_string(&manifest_path).map_err(|source| ExecError::Io {
        path: manifest_path,
        source,
    })?;
    let manifest: Manifest = from_yaml_str(&raw)?;
    let empty_tiers = TiersActive::default();

    let mut cancelled_runs = Vec::new();
    for entry in &manifest.runs {
        let desc = RunDescriptor {
            benchmark: &entry.benchmark,
            group: None,
            mode: entry.mode,
            params: &entry.params,
            tiers: &empty_tiers,
            machine: None,
            node: None,
        };
        if !matches(selector, &desc) {
            continue;
        }

        let rec_exists = entry
            .record
            .as_ref()
            .is_some_and(|rel| results.root.join(rel).exists());
        if rec_exists && entry.status == Status::Ok {
            continue;
        }

        let handle = live_handles
            .get(&entry.run_id)
            .cloned()
            .unwrap_or(JobHandle {
                id: entry.run_id.clone(),
                runspec_path: PathBuf::new(),
                outputs_dir: results.root.join("records").join(&entry.run_id),
            });

        adapter.cancel(&handle)?;
        update_manifest_run(results, &entry.run_id, Status::Failed, None, &[])?;
        cancelled_runs.push(entry.run_id.clone());
    }

    Ok(CancelReport { cancelled_runs })
}
