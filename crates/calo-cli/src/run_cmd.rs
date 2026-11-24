//! Execution of `calo run` (spec §14, §15, §17).

use crate::exit_codes;
use calo_core::adapters::SlurmAdapter;
use calo_core::plan::{expand_suite, matches, RunDescriptor, Selector};
use calo_core::exec::{execute_suite_runs, resolve_probe_binary, Adapter, LocalAdapter};
use calo_core::proc::RealCommandRunner;
use calo_core::results::ResultsDir;
use calo_core::yaml::{load_with_includes, Layer, LayeredBuilder};
use calo_schema::common::TiersActive;
use calo_schema::inventory::{ConnectType, Inventory};
use calo_schema::suite::{SelectorClause, Suite};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
/// Executes `calo run` and returns the normative exit code (spec §14, §17).
#[allow(clippy::too_many_arguments)]
pub fn execute_run(
    suite_path: &Path,
    inventory_path: &Path,
    results_root: &Path,
    cli_sets: &[String],
    select: Option<&str>,
    force: bool,
    dry_run: bool,
    probe_opt: Option<&Path>,
    quiet: bool,
) -> anyhow::Result<u8> {
    let inv_val = load_with_includes(inventory_path)?;
    let inventory: Inventory = calo_schema::yaml::from_value(inv_val)?;

    let suite_val = load_with_includes(suite_path)?;
    let mut builder = LayeredBuilder::new();
    builder.add_layer(Layer::Suite, suite_val);
    builder.add_cli_sets(cli_sets.iter().cloned());
    let layered_suite = builder.resolve::<Suite>()?;

    let mut runs = expand_suite(&layered_suite.value, &inventory)?;
    if let Some(pat) = select {
        let selector = Selector::Clause(SelectorClause {
            benchmark: Some(pat.to_string()),
            ..SelectorClause::default()
        });
        let empty_tiers = TiersActive::default();
        runs.retain(|r| {
            let desc = RunDescriptor {
                benchmark: &r.benchmark,
                group: r.group.as_deref(),
                mode: r.mode,
                params: &r.params,
                tiers: &empty_tiers,
                machine: None,
                node: None,
            };
            matches(&selector, &desc)
        });
    }

    let results = ResultsDir::create(results_root, &layered_suite.value, &inventory)?;
    if !dry_run {
        results.write_manifest(
            &layered_suite,
            &inventory,
            &runs,
            BTreeMap::new(),
            env!("CARGO_PKG_VERSION"),
        )?;
    }

    let probe_bin = if dry_run {
        probe_opt
            .map(Path::to_path_buf)
            .or_else(|| resolve_probe_binary(None).ok())
            .unwrap_or_else(|| PathBuf::from("probe"))
    } else {
        resolve_probe_binary(probe_opt)?
    };

    let suite_root = suite_path.parent().unwrap_or_else(|| Path::new("."));
    let runner = Arc::new(RealCommandRunner);
    let adapter = select_adapter(&runs, &inventory, runner)?;

    let summary = execute_suite_runs(
        &runs,
        &layered_suite.value,
        &inventory,
        &results,
        suite_root,
        &probe_bin,
        adapter.as_ref(),
        force,
        dry_run,
        1,
    )?;

    if dry_run && !quiet {
        for entry in &summary.dry_run_entries {
            println!("run_id: {}", entry.run_id);
            println!("  runspec: {}", entry.runspec_path.display());
            println!("  argv: {}", entry.argv.join(" "));
            println!("  env: {:?}", entry.env);
        }
    }

    if summary.failed > 0 {
        Ok(exit_codes::RUN_FAILURE)
    } else {
        Ok(exit_codes::SUCCESS)
    }
}

fn select_adapter(
    runs: &[calo_core::plan::PinnedRun],
    inventory: &Inventory,
    runner: Arc<RealCommandRunner>,
) -> anyhow::Result<Box<dyn Adapter>> {
    let first_machine = runs
        .first()
        .map(|r| r.machine.split('/').next().unwrap_or(r.machine.as_str()));

    let Some(machine_name) = first_machine else {
        return Ok(Box::new(LocalAdapter::new(runner)));
    };
    let Some(machine) = inventory.machines.get(machine_name) else {
        return Ok(Box::new(LocalAdapter::new(runner)));
    };

    match machine.connect.connect_type {
        ConnectType::Local => Ok(Box::new(LocalAdapter::new(runner))),
        ConnectType::Slurm => {
            let partition = machine.connect.partitions.first().cloned();
            let account = machine.connect.account.clone();
            Ok(Box::new(SlurmAdapter::new(runner, partition, account)))
        }
        ConnectType::Ssh => {
            anyhow::bail!(
                "ssh:// execution adapter is not yet wired into `calo run` for machine `{machine_name}`"
            )
        }
    }
}
