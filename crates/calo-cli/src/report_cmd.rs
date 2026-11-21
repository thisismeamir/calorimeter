//! Execution of `calo report` and `calo diff` (spec §13, §14, §17, §18).

use crate::exit_codes;
use calo_core::report::{diff_results_dirs, generate_report_artifacts};
use calo_core::results::compute_suite_hash;
use calo_core::yaml::{load_with_includes, Layer, LayeredBuilder};
use calo_schema::from_yaml_str;
use calo_schema::record::Record;
use calo_schema::suite::Suite;
use std::fs;
use std::path::Path;

/// Runs `calo report` and returns exit code `4` (`GATE_FAILURE`) if any gate fails (spec §13, §17).
pub fn execute_report(
    suite_path: &Path,
    results_dir: &Path,
    baseline_dir: Option<&Path>,
    cli_sets: &[String],
    quiet: bool,
) -> anyhow::Result<u8> {
    let suite_val = load_with_includes(suite_path)?;
    let mut builder = LayeredBuilder::new();
    builder.add_layer(Layer::Suite, suite_val);
    builder.add_cli_sets(cli_sets.iter().cloned());
    let suite = builder.resolve::<Suite>()?.value;
    let suite_hash = compute_suite_hash(&suite)?;

    let suite_results = if results_dir.join("records").is_dir() {
        results_dir.to_path_buf()
    } else {
        results_dir.join(&suite_hash)
    };

    let records = load_all_records(&suite_results)?;
    let baseline_records = match baseline_dir {
        Some(b) => Some(load_all_records(b)?),
        None => None,
    };

    let suite_root = suite_path.parent().unwrap_or_else(|| Path::new("."));
    let mut any_gate_failed = false;

    for rep in &suite.reports {
        let gen = generate_report_artifacts(
            &suite_results,
            suite_root,
            &suite,
            &suite_hash,
            rep,
            &records,
            baseline_records.as_deref(),
        )?;
        for n in &gen.notices {
            if !quiet {
                eprintln!("notice: {n}");
            }
        }
        for g in &gen.gates {
            if !g.passed {
                any_gate_failed = true;
                if !quiet {
                    eprintln!(
                        "GATE FAILED [{}]: {} observed={:.4} baseline={:.4} threshold={}",
                        rep.name, g.metric, g.observed, g.baseline, g.threshold
                    );
                }
            }
        }
    }

    if any_gate_failed {
        Ok(exit_codes::GATE_FAILURE)
    } else {
        Ok(exit_codes::SUCCESS)
    }
}

/// Runs `calo diff <dirA> <dirB>` (spec §14, §18).
pub fn execute_diff(
    dir_a: &Path,
    dir_b: &Path,
    allow_mixed: bool,
    quiet: bool,
) -> anyhow::Result<u8> {
    let md = diff_results_dirs(dir_a, dir_b, "wall_s", allow_mixed)?;
    if !quiet {
        print!("{md}");
    }
    Ok(exit_codes::SUCCESS)
}

fn load_all_records(dir: &Path) -> anyhow::Result<Vec<Record>> {
    let mut out = Vec::new();
    let rec_root = dir.join("records");
    if !rec_root.is_dir() {
        return Ok(out);
    }
    for entry in fs::read_dir(&rec_root)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            let run_yaml = p.join("run.yaml");
            let target = if run_yaml.exists() {
                run_yaml
            } else {
                p.join("rank0.yaml")
            };
            if target.exists() {
                let raw = fs::read_to_string(&target)?;
                let rec: Record = from_yaml_str(&raw)?;
                out.push(rec);
            }
        }
    }
    Ok(out)
}
