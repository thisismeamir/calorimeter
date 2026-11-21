//! Partial `calo analyze` command scanning results directories and writing `run.yaml` (spec §12.1, §14).

use crate::exit_codes;
use calo_core::aggregate::aggregate_run;
use calo_core::results::write_atomic_bytes;
use calo_schema::record::Record;
use calo_schema::{from_yaml_str, to_yaml_string};
use std::fs;
use std::path::Path;

/// Scans `results_dir` for run directories, writes `run.yaml` for complete runs,
/// and reports incomplete runs (spec §10 R-REC-7, §14).
pub fn execute_analyze(results_dir: &Path, quiet: bool) -> anyhow::Result<u8> {
    let mut aggregated = 0usize;
    let mut incomplete = 0usize;

    // Support either `results/<suite-hash>` directly or `results/` containing `<suite-hash>/records`
    let mut record_roots = Vec::new();
    if results_dir.join("records").is_dir() {
        record_roots.push(results_dir.join("records"));
    } else if results_dir.is_dir() {
        for entry in fs::read_dir(results_dir)? {
            let entry = entry?;
            let sub_records = entry.path().join("records");
            if sub_records.is_dir() {
                record_roots.push(sub_records);
            }
        }
    }

    for rec_root in record_roots {
        for run_entry in fs::read_dir(&rec_root)? {
            let run_entry = run_entry?;
            let run_dir = run_entry.path();
            if !run_dir.is_dir() {
                continue;
            }
            let run_id = run_entry.file_name().to_string_lossy().into_owned();

            let mut rank_files = Vec::new();
            for f in fs::read_dir(&run_dir)? {
                let f = f?;
                let fname = f.file_name().to_string_lossy().into_owned();
                if fname.starts_with("rank") && fname.ends_with(".yaml") {
                    rank_files.push(f.path());
                }
            }
            rank_files.sort();

            if rank_files.is_empty() {
                if !run_dir.join("run.yaml").exists() {
                    incomplete += 1;
                    if !quiet {
                        eprintln!("incomplete run `{run_id}`: no rank records found");
                    }
                }
                continue;
            }

            let mut rank_records = Vec::with_capacity(rank_files.len());
            for rf in &rank_files {
                let raw = fs::read_to_string(rf)?;
                let rec: Record = from_yaml_str(&raw)?;
                rank_records.push(rec);
            }

            let expected_ranks = rank_records[0].identity.ranks.unwrap_or(1) as usize;
            if rank_records.len() < expected_ranks {
                incomplete += 1;
                if !quiet {
                    eprintln!(
                        "incomplete run `{run_id}`: found {}/{} rank records",
                        rank_records.len(),
                        expected_ranks
                    );
                }
                continue;
            }

            let agg = aggregate_run(&rank_records)?;
            let agg_yaml = to_yaml_string(&agg)?;
            write_atomic_bytes(&run_dir.join("run.yaml"), agg_yaml.as_bytes())?;
            aggregated += 1;
        }
    }

    if !quiet {
        println!("analyze complete: {aggregated} run(s) aggregated, {incomplete} incomplete");
    }
    Ok(exit_codes::SUCCESS)
}
