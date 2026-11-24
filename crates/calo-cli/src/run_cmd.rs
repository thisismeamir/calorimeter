//! Execution of `calo run` (spec §14, §15, §17).

use crate::exit_codes;
use calo_core::adapters::SlurmAdapter;
use calo_core::exec::schedule::execute_suite_runs_with_cwds;
use calo_core::exec::{resolve_probe_binary, Adapter, LocalAdapter};
use calo_core::plan::{expand_suite, matches, RunDescriptor, Selector};
use calo_core::proc::{CommandRunner, RealCommandRunner};
use calo_core::results::ResultsDir;
use calo_core::workspace::{execute_build, resolve_sources, BuildTargetContext, Worktree};
use calo_core::yaml::{load_with_includes, Layer, LayeredBuilder};
use calo_schema::common::{ScalarValue, TiersActive};
use calo_schema::inventory::{ConnectType, Inventory};
use calo_schema::suite::{SelectorClause, Suite};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
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

    let suite_root = suite_path.parent().unwrap_or_else(|| Path::new("."));
    let runner = Arc::new(RealCommandRunner);

    let source_shas = if !dry_run && !layered_suite.value.sources.is_empty() {
        resolve_sources(&layered_suite.value, runner.as_ref())?
    } else {
        BTreeMap::new()
    };

    let results = ResultsDir::create(results_root, &layered_suite.value, &inventory)?;
    if !dry_run {
        results.write_manifest(
            &layered_suite,
            &inventory,
            &runs,
            source_shas.clone(),
            env!("CARGO_PKG_VERSION"),
        )?;
    }

    let run_cwds = if !dry_run && !layered_suite.value.sources.is_empty() {
        prepare_workspaces_and_builds(
            &runs,
            &layered_suite.value,
            &inventory,
            &results.root,
            suite_root,
            &source_shas,
            runner.as_ref(),
            quiet,
        )?
    } else {
        BTreeMap::new()
    };

    let probe_bin = if dry_run {
        probe_opt
            .map(Path::to_path_buf)
            .or_else(|| resolve_probe_binary(None).ok())
            .unwrap_or_else(|| PathBuf::from("probe"))
    } else {
        resolve_probe_binary(probe_opt)?
    };

    let adapter = select_adapter(&runs, &inventory, runner)?;

    let summary = execute_suite_runs_with_cwds(
        &runs,
        &layered_suite.value,
        &inventory,
        &results,
        suite_root,
        &run_cwds,
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

#[allow(clippy::too_many_arguments)]
fn prepare_workspaces_and_builds(
    runs: &[calo_core::plan::PinnedRun],
    suite: &Suite,
    inventory: &Inventory,
    results_root: &Path,
    suite_root: &Path,
    source_shas: &BTreeMap<String, String>,
    runner: &dyn CommandRunner,
    quiet: bool,
) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    let Some((primary_src_name, primary_src_def)) = suite.sources.iter().next() else {
        return Ok(BTreeMap::new());
    };
    let primary_sha = source_shas
        .get(primary_src_name)
        .map(String::as_str)
        .unwrap_or(&primary_src_def.rev);

    let mut worktrees: BTreeMap<(String, String), Worktree> = BTreeMap::new();
    let mut built_keys: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut run_cwds = BTreeMap::new();

    for run in runs {
        let (machine_name, class_opt) = match run.machine.split_once('/') {
            Some((m, c)) => (m, Some(c)),
            None => (run.machine.as_str(), None),
        };
        let Some(machine) = inventory.machines.get(machine_name) else {
            continue;
        };
        let workdir = machine
            .workdir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| results_root.join("workspace"));

        let patchset_name = match run.params.get("patch") {
            Some(ScalarValue::Str(s)) => s.clone(),
            _ => "none".to_string(),
        };

        let wt_key = (machine_name.to_string(), patchset_name.clone());
        let wt = match worktrees.entry(wt_key) {
            Entry::Occupied(occ) => occ.into_mut(),
            Entry::Vacant(vac) => {
                let patch_files: Vec<PathBuf> = suite
                    .patches
                    .as_ref()
                    .and_then(|p| {
                        p.sets.get(&patchset_name).map(|files| {
                            files
                                .iter()
                                .map(|f| {
                                    suite_root
                                        .join(p.dir.as_deref().unwrap_or("patches"))
                                        .join(f)
                                })
                                .collect()
                        })
                    })
                    .unwrap_or_default();
                let apply_cfg = suite.patches.as_ref().and_then(|p| p.apply.as_ref());
                let created = Worktree::create(
                    primary_src_name,
                    primary_src_def,
                    primary_sha,
                    &patchset_name,
                    &patch_files,
                    machine_name,
                    &workdir,
                    apply_cfg,
                    runner,
                )?;
                vac.insert(created)
            }
        };

        run_cwds.insert(run.run_id.clone(), wt.path.clone());

        let bench = suite.benchmarks.iter().find(|b| b.name == run.benchmark);
        let Some(build_name) = bench.and_then(|b| b.build.as_deref()) else {
            continue;
        };
        let Some(build_def) = suite.builds.get(build_name) else {
            continue;
        };

        let bkey = (
            machine_name.to_string(),
            patchset_name.clone(),
            build_name.to_string(),
        );
        if built_keys.insert(bkey) {
            let node = class_opt
                .and_then(|c| machine.node_classes.get(c).map(|nc| &nc.node))
                .or(machine.node.as_ref());
            let cpu_model = node
                .and_then(|n| n.cpu.as_ref())
                .and_then(|c| c.model.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let gpu_arch = node
                .and_then(|n| n.gpu.as_ref())
                .and_then(|g| g.arch.clone())
                .unwrap_or_else(|| "none".to_string());

            let ctx = BuildTargetContext {
                source_shas: source_shas.clone(),
                patch_tree_hash: wt.patchset.tree_hash.clone(),
                cpu_model,
                gpu_arch,
                toolchain: "default".to_string(),
                mpi_identity: "default".to_string(),
            };
            let outcome = execute_build(
                build_name,
                build_def,
                &ctx,
                &run.params,
                &machine.modules,
                &wt.path,
                &workdir.join("builds"),
                runner,
            )?;
            if !quiet {
                let tag = if outcome.cache_hit {
                    "cache hit"
                } else {
                    "built"
                };
                println!(
                    "build `{build_name}` [patch={patchset_name}] ({tag}): {}",
                    outcome.artifact_dir.display()
                );
            }
        }
    }

    Ok(run_cwds)
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
