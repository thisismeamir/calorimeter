//! CLI handlers for `calo status` and `calo cancel` (spec §14).

use crate::cli::PlanFormat;
use crate::exit_codes;
use calo_core::exec::{cancel_runs, collect_status, format_status_table, LocalAdapter};
use calo_core::plan::Selector;
use calo_core::proc::RealCommandRunner;
use calo_core::results::ResultsDir;
use calo_schema::suite::SelectorClause;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Executes `calo status` on `results_dir` (spec §14).
pub fn execute_status(results_dir: &Path, format: PlanFormat, quiet: bool) -> anyhow::Result<u8> {
    let suite_results = resolve_suite_results_dir(results_dir)?;
    let res = ResultsDir::from_existing(suite_results, "");
    let adapter = LocalAdapter::new(Arc::new(RealCommandRunner));
    let out = collect_status(&res, &adapter, &BTreeMap::new())?;

    if !quiet {
        match format {
            PlanFormat::Table => print!("{}", format_status_table(&out)),
            PlanFormat::Yaml => print!("{}", calo_schema::yaml::to_string(&out)?),
        }
    }
    Ok(exit_codes::SUCCESS)
}

/// Executes `calo cancel` on `results_dir` for matching runs (spec §14).
pub fn execute_cancel(results_dir: &Path, select: Option<&str>, quiet: bool) -> anyhow::Result<u8> {
    let suite_results = resolve_suite_results_dir(results_dir)?;
    let res = ResultsDir::from_existing(suite_results, "");
    let adapter = LocalAdapter::new(Arc::new(RealCommandRunner));
    let selector = match select {
        Some(pat) => Selector::Clause(SelectorClause {
            benchmark: Some(pat.to_string()),
            ..SelectorClause::default()
        }),
        None => Selector::All,
    };

    let rep = cancel_runs(&res, &adapter, &selector, &BTreeMap::new())?;
    if !quiet {
        println!("cancelled {} run(s)", rep.cancelled_runs.len());
    }
    Ok(exit_codes::SUCCESS)
}

fn resolve_suite_results_dir(results_dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    if results_dir.join("manifest.yaml").exists() {
        return Ok(results_dir.to_path_buf());
    }
    if results_dir.is_dir() {
        for entry in std::fs::read_dir(results_dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.join("manifest.yaml").exists() {
                return Ok(p);
            }
        }
    }
    Ok(results_dir.to_path_buf())
}
