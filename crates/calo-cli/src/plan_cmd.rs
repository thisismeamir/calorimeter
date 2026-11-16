//! Execution of `calo plan` (spec §8.5 R-BEN-2, §14).

use crate::cli::PlanFormat;
use calo_core::plan::{estimate_cost, expand_suite, Cost, PinnedRun};
use calo_core::yaml::{load_with_includes, Layer, LayeredBuilder};
use calo_schema::inventory::Inventory;
use calo_schema::suite::Suite;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Serialize)]
struct YamlPlanOutput<'a> {
    runs: &'a [PinnedRun],
    cost: &'a Cost,
}

/// Expands the suite and formats the execution plan as either an ASCII table or YAML (spec §14).
pub fn execute_plan(
    suite_path: &Path,
    inventory_path: &Path,
    cli_sets: &[String],
    format: PlanFormat,
) -> anyhow::Result<String> {
    let inv_val = load_with_includes(inventory_path)?;
    let inventory: Inventory = calo_schema::yaml::from_value(inv_val)?;

    let suite_val = load_with_includes(suite_path)?;
    let mut builder = LayeredBuilder::new();
    builder.add_layer(Layer::Suite, suite_val);
    builder.add_cli_sets(cli_sets.iter().cloned());
    let suite = builder.resolve::<Suite>()?.value;

    let runs = expand_suite(&suite, &inventory)?;
    let cost = estimate_cost(&runs, &suite, &inventory)?;

    match format {
        PlanFormat::Yaml => {
            let payload = YamlPlanOutput {
                runs: &runs,
                cost: &cost,
            };
            let s = calo_schema::yaml::to_string(&payload)?;
            Ok(s)
        }
        PlanFormat::Table => Ok(format_plan_table(&runs, &cost)),
    }
}

fn format_plan_table(runs: &[PinnedRun], cost: &Cost) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<44} {:<18} {:<10} PARAMS",
        "RUN_ID", "MACHINE", "MODE"
    );

    for r in runs {
        let mode_str = match r.mode {
            calo_schema::Mode::Measure => "measure",
            calo_schema::Mode::Profile => "profile",
            calo_schema::Mode::Kernel => "kernel",
        };
        let params_str: Vec<String> = r
            .params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let _ = writeln!(
            out,
            "{:<44} {:<18} {:<10} {}",
            r.run_id,
            r.machine,
            mode_str,
            params_str.join(", ")
        );
    }

    let _ = writeln!(
        out,
        "\nTotal runs: {} | Upper-bound node-hours: {:.2} | Upper-bound GPU-hours: {:.2}",
        cost.runs, cost.node_hours, cost.gpu_hours
    );
    out
}
