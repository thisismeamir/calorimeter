//! Resource cost upper-bound estimation (`calo plan`, spec §8.5 R-BEN-2).

use crate::expr::{eval, interpolate, parse, Value};
use crate::plan::error::PlanError;
use crate::plan::expand::PinnedRun;
use crate::plan::selector::{resolve_machine_node, RunExprEnv};
use calo_schema::inventory::Inventory;
use calo_schema::suite::{IntOrExpr, Suite};
use serde::{Deserialize, Serialize};

/// Upper-bound resource cost summary for a set of expanded runs (spec §8.5 R-BEN-2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// Total number of expanded runs (including warmup and repetitions).
    pub runs: usize,
    /// Upper-bound node-hours computed from step `timeout` and `resources.nodes`.
    pub node_hours: f64,
    /// Upper-bound GPU-hours computed from step `timeout` and allocated GPUs per run.
    pub gpu_hours: f64,
}

/// Estimates upper-bound `runs`, `node_hours`, and `gpu_hours` for `runs` (spec §8.5 R-BEN-2).
pub fn estimate_cost(
    runs: &[PinnedRun],
    suite: &Suite,
    inventory: &Inventory,
) -> Result<Cost, PlanError> {
    let mut total_node_hours = 0.0;
    let mut total_gpu_hours = 0.0;

    for run in runs {
        let Some(bench) = suite.benchmarks.iter().find(|b| b.name == run.benchmark) else {
            continue;
        };

        let (mach_entry, node_entry) = match resolve_machine_node(inventory, &run.machine) {
            Some((m, n)) => (Some(m), n),
            None => (None, None),
        };
        let env = RunExprEnv {
            params: &run.params,
            machine: mach_entry,
            node: node_entry,
        };

        for step_name in &bench.steps {
            let Some(step) = suite.steps.get(step_name) else {
                continue;
            };
            let is_measured = step.measured.unwrap_or(false)
                || bench.measure.contains(step_name)
                || bench.steps.len() == 1;
            if !is_measured {
                continue;
            }

            let hours = match &step.timeout {
                Some(t_str) => parse_timeout_hours(step_name, t_str)?,
                None => 0.0,
            };

            let (nodes, ranks_per_node, gpus_per_rank) = if let Some(res) = &step.resources {
                let n = eval_int_or_expr(res.nodes.as_ref(), &env)
                    .unwrap_or(1)
                    .max(1);
                let rpn = eval_int_or_expr(res.ranks_per_node.as_ref(), &env)
                    .unwrap_or(1)
                    .max(1);
                let gpr = eval_int_or_expr(res.gpus_per_rank.as_ref(), &env).unwrap_or(0);
                (n, rpn, gpr)
            } else {
                (1, 1, 0)
            };

            let gpus_on_run = if gpus_per_rank > 0 {
                nodes * ranks_per_node * gpus_per_rank
            } else if let Some(per_node) = node_entry
                .and_then(|n| n.gpu.as_ref())
                .and_then(|g| g.per_node)
            {
                if step.resources.is_some() {
                    nodes * per_node
                } else {
                    0
                }
            } else {
                0
            };

            total_node_hours += (nodes as f64) * hours;
            total_gpu_hours += (gpus_on_run as f64) * hours;
        }
    }

    Ok(Cost {
        runs: runs.len(),
        node_hours: total_node_hours,
        gpu_hours: total_gpu_hours,
    })
}

fn eval_int_or_expr(val: Option<&IntOrExpr>, env: &RunExprEnv<'_>) -> Option<u32> {
    let val = val?;
    match val {
        IntOrExpr::Int(v) => Some(*v),
        IntOrExpr::Expr(s) => {
            if s.contains("${{") {
                let rendered = interpolate(s, env).ok()?;
                rendered.trim().parse::<u32>().ok()
            } else {
                let ast = parse(s).ok()?;
                match eval(&ast, env).ok()? {
                    Value::Int(i) => u32::try_from(i).ok(),
                    Value::Float(f) => Some(f.round() as u32),
                    _ => None,
                }
            }
        }
    }
}

/// Parses a timeout string (`"HH:MM:SS"`, `"MM:SS"`, or numeric seconds) into hours.
pub fn parse_timeout_hours(step_name: &str, timeout: &str) -> Result<f64, PlanError> {
    let parts: Vec<&str> = timeout.trim().split(':').collect();
    match parts.as_slice() {
        [hh, mm, ss] => {
            let h: f64 = hh.parse().map_err(|_| PlanError::InvalidTimeout {
                step: step_name.to_string(),
                timeout: timeout.to_string(),
            })?;
            let m: f64 = mm.parse().map_err(|_| PlanError::InvalidTimeout {
                step: step_name.to_string(),
                timeout: timeout.to_string(),
            })?;
            let s: f64 = ss.parse().map_err(|_| PlanError::InvalidTimeout {
                step: step_name.to_string(),
                timeout: timeout.to_string(),
            })?;
            Ok(h + m / 60.0 + s / 3600.0)
        }
        [mm, ss] => {
            let m: f64 = mm.parse().map_err(|_| PlanError::InvalidTimeout {
                step: step_name.to_string(),
                timeout: timeout.to_string(),
            })?;
            let s: f64 = ss.parse().map_err(|_| PlanError::InvalidTimeout {
                step: step_name.to_string(),
                timeout: timeout.to_string(),
            })?;
            Ok(m / 60.0 + s / 3600.0)
        }
        [ss] => {
            let s: f64 = ss.parse().map_err(|_| PlanError::InvalidTimeout {
                step: step_name.to_string(),
                timeout: timeout.to_string(),
            })?;
            Ok(s / 3600.0)
        }
        _ => Err(PlanError::InvalidTimeout {
            step: step_name.to_string(),
            timeout: timeout.to_string(),
        }),
    }
}
