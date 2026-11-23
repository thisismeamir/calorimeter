//! Normative benchmark matrix expansion (spec §8.5 steps 1–7).

use crate::expr::{eval, parse, references, Expr, Value};
use crate::plan::error::PlanError;
use crate::plan::selector::{resolve_machine_node, scalars_equal, RunExprEnv};
use calo_schema::common::{Mode, ScalarValue};
use calo_schema::inventory::Inventory;
use calo_schema::suite::{BenchmarkDef, ExpandMode, MachineTarget, Suite};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A fully expanded, pinned benchmark run ready for scheduling or cost estimation (spec §3 I3, §8.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedRun {
    /// Deterministic run identifier (`<benchmark>.<sorted param values>.r<repeat>`).
    pub run_id: String,
    /// Benchmark name.
    pub benchmark: String,
    /// Optional benchmark group name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Execution mode (`measure`, `profile`, `kernel`).
    pub mode: Mode,
    /// Resolved target machine or `machine/node_class`.
    pub machine: String,
    /// Pinned parameter key-value map (including evaluated `derive` keys).
    pub params: BTreeMap<String, ScalarValue>,
    /// Repetition index (`0..repeat`).
    pub repeat: u32,
    /// `true` if this run is a warmup repetition (spec §8.5 R-BEN-1).
    pub warmup: bool,
    /// Source run ID when derived via `from:` (spec §8.5 step 6, R-BEN-4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_from: Option<String>,
}

/// Expands all benchmarks in `suite` against `inventory` following spec §8.5 steps 1–7.
pub fn expand_suite(suite: &Suite, inventory: &Inventory) -> Result<Vec<PinnedRun>, PlanError> {
    let mut all_runs: Vec<PinnedRun> = Vec::new();
    // Store expanded parameter sets (before warmup/repeat multiplication) per benchmark for `from:` lookups.
    #[allow(clippy::type_complexity)]
    let mut expanded_param_sets: BTreeMap<
        String,
        Vec<(String, BTreeMap<String, ScalarValue>)>,
    > = BTreeMap::new();

    for bench in &suite.benchmarks {
        let machine_name = resolve_benchmark_machine(bench, inventory)?;
        let (mach_entry, node_entry) =
            resolve_machine_node(inventory, &machine_name).ok_or_else(|| {
                PlanError::UnknownMachine {
                    benchmark: bench.name.clone(),
                    machine: machine_name.clone(),
                }
            })?;

        let mode = bench.mode.unwrap_or(Mode::Measure);
        let repeat_count = bench.repeat.unwrap_or(1).max(1);
        let warmup_count = bench.warmup.unwrap_or(0);
        let where_clause = bench
            .filter_where
            .as_deref()
            .or(bench.where_expr.as_deref());

        let derive_order = topo_sort_derive(&bench.name, &bench.derive)?;

        // Step 6: Check if `from` is set
        let mut points_with_links: Vec<(Option<String>, BTreeMap<String, ScalarValue>)> =
            if let Some(from_name) = &bench.from {
                let source_points = expanded_param_sets.get(from_name).ok_or_else(|| {
                    PlanError::UnknownFromBenchmark {
                        benchmark: bench.name.clone(),
                        from: from_name.clone(),
                    }
                })?;

                let mut inherited = Vec::new();
                for (src_base_id, src_params) in source_points {
                    let mut merged_params = src_params.clone();
                    for (k, v) in &bench.params {
                        merged_params.insert(k.clone(), v.clone());
                    }
                    evaluate_derives(
                        &bench.name,
                        &derive_order,
                        &mut merged_params,
                        Some(mach_entry),
                        node_entry,
                    )?;
                    if passes_where(
                        &bench.name,
                        where_clause,
                        &merged_params,
                        Some(mach_entry),
                        node_entry,
                    )? {
                        inherited.push((Some(src_base_id.clone()), merged_params));
                    }
                }
                inherited
            } else {
                // Steps 1–4: Base matrix expansion + include - exclude
                let raw_points = expand_matrix_points(bench)?;
                let mut evaluated = Vec::with_capacity(raw_points.len());
                for mut pt in raw_points {
                    // Step 5: Evaluate `derive` in topological dependency order
                    evaluate_derives(
                        &bench.name,
                        &derive_order,
                        &mut pt,
                        Some(mach_entry),
                        node_entry,
                    )?;
                    if passes_where(&bench.name, where_clause, &pt, Some(mach_entry), node_entry)? {
                        evaluated.push((None, pt));
                    }
                }
                evaluated
            };

        // Deduplicate identical parameter points while preserving order
        let mut unique_points: Vec<(Option<String>, BTreeMap<String, ScalarValue>)> = Vec::new();
        for (link, pt) in points_with_links.drain(..) {
            if !unique_points.iter().any(|(_, existing)| existing == &pt) {
                unique_points.push((link, pt));
            }
        }

        let mut bench_base_records = Vec::new();

        for (linked_base, params) in unique_points {
            let r0_id = format_run_id(&bench.name, &params, 0, false);
            bench_base_records.push((r0_id, params.clone()));

            // Emit warmup runs first (flagged `warmup: true`, R-BEN-1)
            for w in 0..warmup_count {
                let run_id = format_run_id(&bench.name, &params, w, true);
                all_runs.push(PinnedRun {
                    run_id,
                    benchmark: bench.name.clone(),
                    group: bench.group.clone(),
                    mode,
                    machine: machine_name.clone(),
                    params: params.clone(),
                    repeat: w,
                    warmup: true,
                    linked_from: linked_base.clone(),
                });
            }

            // Emit measured repetitions (`0..repeat_count`)
            for r in 0..repeat_count {
                let run_id = format_run_id(&bench.name, &params, r, false);
                all_runs.push(PinnedRun {
                    run_id,
                    benchmark: bench.name.clone(),
                    group: bench.group.clone(),
                    mode,
                    machine: machine_name.clone(),
                    params: params.clone(),
                    repeat: r,
                    warmup: false,
                    linked_from: linked_base.clone(),
                });
            }
        }

        expanded_param_sets.insert(bench.name.clone(), bench_base_records);
    }

    Ok(all_runs)
}

fn resolve_benchmark_machine(
    bench: &BenchmarkDef,
    inventory: &Inventory,
) -> Result<String, PlanError> {
    match &bench.machine {
        Some(MachineTarget::Name(name)) => Ok(name.clone()),
        Some(MachineTarget::Selector(_)) | None => inventory
            .machines
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| PlanError::UnknownMachine {
                benchmark: bench.name.clone(),
                machine: "<default>".to_string(),
            }),
    }
}

/// Implements spec §8.5 steps 1–4 (`axes`, `expand`, `include`, `exclude`).
fn expand_matrix_points(
    bench: &BenchmarkDef,
) -> Result<Vec<BTreeMap<String, ScalarValue>>, PlanError> {
    // Validate non-empty axes
    for (axis, vals) in &bench.axes {
        if vals.is_empty() {
            return Err(PlanError::EmptyAxis {
                benchmark: bench.name.clone(),
                axis: axis.clone(),
            });
        }
    }

    let expand_mode = bench.expand.unwrap_or(ExpandMode::All);

    // Step 1 & 2: Base set
    let mut points: Vec<BTreeMap<String, ScalarValue>> = if bench.axes.is_empty() {
        vec![bench.params.clone()]
    } else {
        match expand_mode {
            ExpandMode::All => cartesian_product(&bench.params, &bench.axes),
            ExpandMode::Defaults => {
                let mut single = bench.params.clone();
                for axis in bench.axes.keys() {
                    let def_val = bench
                        .defaults
                        .get(axis)
                        .or_else(|| bench.params.get(axis))
                        .ok_or_else(|| PlanError::MissingDefault {
                            benchmark: bench.name.clone(),
                            axis: axis.clone(),
                        })?;
                    single.insert(axis.clone(), def_val.clone());
                }
                vec![single]
            }
            ExpandMode::None => Vec::new(),
        }
    };

    // Step 3: Add each `include`. Partial pattern fills unnamed axes per `expand`.
    for inc_pattern in &bench.include {
        let mut fixed = bench.params.clone();
        for (k, v) in inc_pattern {
            fixed.insert(k.clone(), v.clone());
        }

        let mut remaining_axes: BTreeMap<String, Vec<ScalarValue>> = BTreeMap::new();
        for (axis, axis_vals) in &bench.axes {
            if fixed.contains_key(axis) && inc_pattern.contains_key(axis) {
                continue;
            }
            match expand_mode {
                ExpandMode::All => {
                    remaining_axes.insert(axis.clone(), axis_vals.clone());
                }
                ExpandMode::Defaults => {
                    let def_val = bench
                        .defaults
                        .get(axis)
                        .or_else(|| bench.params.get(axis))
                        .ok_or_else(|| PlanError::MissingDefault {
                            benchmark: bench.name.clone(),
                            axis: axis.clone(),
                        })?;
                    fixed.insert(axis.clone(), def_val.clone());
                }
                ExpandMode::None => {
                    if let Some(def_val) = bench.defaults.get(axis) {
                        fixed.insert(axis.clone(), def_val.clone());
                    } else {
                        return Err(PlanError::IncompleteIncludeForNone {
                            benchmark: bench.name.clone(),
                            axis: axis.clone(),
                        });
                    }
                }
            }
        }

        if remaining_axes.is_empty() {
            points.push(fixed);
        } else {
            points.extend(cartesian_product(&fixed, &remaining_axes));
        }
    }

    // Step 4: Remove every run matching any `exclude` (all pattern keys equal).
    if !bench.exclude.is_empty() {
        points.retain(|pt| !bench.exclude.iter().any(|excl| pattern_matches(excl, pt)));
    }

    Ok(points)
}

fn cartesian_product(
    base: &BTreeMap<String, ScalarValue>,
    axes: &BTreeMap<String, Vec<ScalarValue>>,
) -> Vec<BTreeMap<String, ScalarValue>> {
    let mut results = vec![base.clone()];
    for (axis_name, values) in axes {
        let mut next = Vec::with_capacity(results.len() * values.len());
        for prefix in &results {
            for val in values {
                let mut map = prefix.clone();
                map.insert(axis_name.clone(), val.clone());
                next.push(map);
            }
        }
        results = next;
    }
    results
}

fn pattern_matches(
    pattern: &BTreeMap<String, ScalarValue>,
    point: &BTreeMap<String, ScalarValue>,
) -> bool {
    if pattern.is_empty() {
        return false;
    }
    pattern.iter().all(|(k, expected)| {
        point
            .get(k)
            .is_some_and(|actual| scalars_equal(actual, expected))
    })
}

/// Topologically sorts `derive` expressions by their referenced parameter names (spec §8.5 step 5).
fn topo_sort_derive(
    benchmark: &str,
    derive_map: &BTreeMap<String, String>,
) -> Result<Vec<(String, Expr)>, PlanError> {
    if derive_map.is_empty() {
        return Ok(Vec::new());
    }

    let mut parsed: BTreeMap<String, Expr> = BTreeMap::new();
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for (key, expr_src) in derive_map {
        let ast = parse(expr_src).map_err(|source| PlanError::Expression {
            benchmark: benchmark.to_string(),
            context: format!("derive.{key}"),
            source,
        })?;
        let raw_refs = references(&ast);
        let mut key_deps = BTreeSet::new();
        for r in raw_refs {
            let bare = r.strip_prefix("params.").unwrap_or(&r);
            if derive_map.contains_key(bare) {
                key_deps.insert(bare.to_string());
            }
        }
        deps.insert(key.clone(), key_deps);
        parsed.insert(key.clone(), ast);
    }

    let mut sorted = Vec::with_capacity(derive_map.len());
    let mut resolved = BTreeSet::new();
    let mut remaining: BTreeSet<String> = derive_map.keys().cloned().collect();

    while !remaining.is_empty() {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|k| deps[*k].iter().all(|d| resolved.contains(d)))
            .cloned()
            .collect();

        if ready.is_empty() {
            let cycle_keys: Vec<String> = remaining.into_iter().collect();
            return Err(PlanError::DeriveCycle {
                benchmark: benchmark.to_string(),
                cycle: cycle_keys.join(" -> "),
            });
        }

        for k in ready {
            remaining.remove(&k);
            resolved.insert(k.clone());
            if let Some(ast) = parsed.remove(&k) {
                sorted.push((k, ast));
            }
        }
    }

    Ok(sorted)
}

fn evaluate_derives(
    benchmark: &str,
    derive_order: &[(String, Expr)],
    params: &mut BTreeMap<String, ScalarValue>,
    machine: Option<&calo_schema::inventory::MachineEntry>,
    node: Option<&calo_schema::inventory::InventoryNode>,
) -> Result<(), PlanError> {
    for (key, ast) in derive_order {
        let env = RunExprEnv {
            params,
            machine,
            node,
        };
        let val = eval(ast, &env).map_err(|source| PlanError::Expression {
            benchmark: benchmark.to_string(),
            context: format!("derive.{key}"),
            source,
        })?;
        let scalar = val.to_scalar().ok_or_else(|| PlanError::Expression {
            benchmark: benchmark.to_string(),
            context: format!("derive.{key}"),
            source: crate::expr::ExprError::TypeError {
                span: ast.span,
                message: "derived parameter must evaluate to a scalar value".to_string(),
            },
        })?;
        params.insert(key.clone(), scalar);
    }
    Ok(())
}

fn passes_where(
    benchmark: &str,
    where_clause: Option<&str>,
    params: &BTreeMap<String, ScalarValue>,
    machine: Option<&calo_schema::inventory::MachineEntry>,
    node: Option<&calo_schema::inventory::InventoryNode>,
) -> Result<bool, PlanError> {
    let Some(src) = where_clause else {
        return Ok(true);
    };
    let ast = parse(src).map_err(|source| PlanError::Expression {
        benchmark: benchmark.to_string(),
        context: "where".to_string(),
        source,
    })?;
    let env = RunExprEnv {
        params,
        machine,
        node,
    };
    match eval(&ast, &env) {
        Ok(Value::Bool(b)) => Ok(b),
        Ok(other) => Err(PlanError::Expression {
            benchmark: benchmark.to_string(),
            context: "where".to_string(),
            source: crate::expr::ExprError::TypeError {
                span: ast.span,
                message: format!("`where` must evaluate to bool, found {}", other.type_name()),
            },
        }),
        Err(source) => Err(PlanError::Expression {
            benchmark: benchmark.to_string(),
            context: "where".to_string(),
            source,
        }),
    }
}

/// Formats a deterministic run ID per spec §8.5 step 7:
/// `<benchmark>.<sorted param values>.r<repeat>` (or `.w<repeat>` for warmup runs).
/// IDs longer than 120 characters are truncated to 120 characters with a deterministic 16-hex-char hash suffix.
pub fn format_run_id(
    benchmark: &str,
    params: &BTreeMap<String, ScalarValue>,
    repeat: u32,
    warmup: bool,
) -> String {
    let mut parts = Vec::with_capacity(params.len() + 2);
    parts.push(sanitize_id_segment(benchmark));
    for (k, v) in params {
        parts.push(format!(
            "{}_{}",
            sanitize_id_segment(k),
            sanitize_id_segment(&v.to_string())
        ));
    }
    if warmup {
        parts.push(format!("w{repeat}"));
    } else {
        parts.push(format!("r{repeat}"));
    }

    let full = parts.join(".");
    if full.len() <= 120 {
        full
    } else {
        let hash = fnv1a_64_hex(&full);
        // 103 chars + '.' (1 char) + 16 hex chars = 120 chars
        let prefix = &full[..103];
        format!("{prefix}.{hash}")
    }
}

fn sanitize_id_segment(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '+' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn fnv1a_64_hex(s: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}
