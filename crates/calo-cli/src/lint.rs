//! Static validation for `calo lint` (spec §2, §5.2, §8.3 R-BLD-3, §8.4 R-STP-2, §12.2 R-SVC-2, §14).

use calo_core::expr::{parse, references};
use calo_core::plan::selector::resolve_machine_node;
use calo_core::plan::{matches, RunDescriptor, Selector};
use calo_core::yaml::{load_with_includes, Layer, LayeredBuilder, LoadError};
use calo_schema::common::{CpuTier, GpuTier, Mode, MpiTier, Tier, TiersActive};
use calo_schema::inventory::Inventory;
use calo_schema::suite::{
    BenchmarkDef, DebugInfoPolicy, IntOrExpr, MachineTarget, ReportSelect, Suite,
};
use calo_schema::yaml::Value;
use calo_schema::SchemaDoc;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// A single lint diagnostic formatted as `file:line: message` (spec §14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintDiagnostic {
    /// Source file path.
    pub file: PathBuf,
    /// 1-based line number in `file`.
    pub line: usize,
    /// Diagnostic message.
    pub message: String,
}

impl fmt::Display for LintDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file.display(), self.line, self.message)
    }
}

/// Built-in service names from spec §12.3.
const BUILTIN_SERVICES: &[&str] = &[
    "stats.summary",
    "compare",
    "ipc",
    "cache_miss_rate",
    "speedup",
    "parallel_efficiency",
    "scaling_fit",
    "comm_fraction",
    "imbalance",
    "collective_wait",
    "comm_matrix",
    "message_size_profile",
    "overlap",
    "gpu_utilization",
    "kernel_hotspots",
    "transfer_summary",
    "launch_overhead",
    "gpu_idle_gaps",
    "host_device_overlap",
    "occupancy",
    "hotspots",
    "region_breakdown",
    "profile_diff",
    "boundness",
    "time_budget",
];

/// Built-in scalar/observation metrics valid as bare expression/service inputs (spec §5.2, §10).
const BUILTIN_METRICS: &[&str] = &[
    "wall_s",
    "user_cpu_s",
    "sys_cpu_s",
    "peak_rss_bytes",
    "minor_faults",
    "major_faults",
    "voluntary_ctx",
    "involuntary_ctx",
    "block_in_bytes",
    "block_out_bytes",
    "cycles",
    "instructions",
    "cache-misses",
    "branch-misses",
    "l1d-misses",
    "llc-misses",
    "stalled-backend",
];

/// Runs all `calo lint` checks on `suite_path` and `inventory_path` with optional `--set` overrides (spec §14).
pub fn lint_files(
    suite_path: &Path,
    inventory_path: &Path,
    cli_sets: &[String],
) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    let suite_src = fs::read_to_string(suite_path).unwrap_or_default();
    let inv_src = fs::read_to_string(inventory_path).unwrap_or_default();

    // 1. Load & validate `inventory.yaml`
    let inventory: Option<Inventory> =
        match load_and_parse_doc::<Inventory>(inventory_path, &inv_src, &[]) {
            Ok(inv) => Some(inv),
            Err(d) => {
                diags.push(d);
                None
            }
        };

    // 2. Load & validate `suite.yaml`
    let suite: Option<Suite> = match load_and_parse_doc::<Suite>(suite_path, &suite_src, cli_sets) {
        Ok(s) => Some(s),
        Err(d) => {
            diags.push(d);
            None
        }
    };

    let (Some(suite), Some(inventory)) = (suite, inventory) else {
        return diags;
    };

    // 3. Cross-reference & semantic checks on Suite + Inventory
    check_benchmarks(&suite, &inventory, suite_path, &suite_src, &mut diags);
    check_services(&suite, suite_path, &suite_src, &mut diags);
    check_reports(&suite, &inventory, suite_path, &suite_src, &mut diags);

    diags
}

fn load_and_parse_doc<T>(
    path: &Path,
    raw_src: &str,
    cli_sets: &[String],
) -> Result<T, LintDiagnostic>
where
    T: SchemaDoc + serde::de::DeserializeOwned,
{
    let raw_val = load_with_includes(path).map_err(|e| load_err_to_diag(path, raw_src, e))?;

    // Check top-level `schema:` header before full deserialization (spec §0.2, §14)
    let found_schema = match &raw_val {
        Value::Mapping(map) => map
            .get(Value::String("schema".to_string()))
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            }),
        _ => None,
    };

    let Some(schema_str) = found_schema else {
        return Err(LintDiagnostic {
            file: path.to_path_buf(),
            line: 1,
            message: "missing required `schema:` header".to_string(),
        });
    };

    if schema_str != T::SCHEMA {
        let line = find_line_containing(raw_src, "schema:").unwrap_or(1);
        return Err(LintDiagnostic {
            file: path.to_path_buf(),
            line,
            message: format!(
                "schema version mismatch: expected `{}`, found `{schema_str}`",
                T::SCHEMA
            ),
        });
    }

    let mut builder = LayeredBuilder::new();
    builder.add_layer(Layer::Suite, raw_val);
    builder.add_cli_sets(cli_sets.iter().cloned());

    let layered = builder
        .resolve::<T>()
        .map_err(|e| load_err_to_diag(path, raw_src, e))?;

    layered.value.check().map_err(|e| LintDiagnostic {
        file: path.to_path_buf(),
        line: 1,
        message: e.to_string(),
    })?;

    Ok(layered.value)
}

fn load_err_to_diag(default_path: &Path, raw_src: &str, err: LoadError) -> LintDiagnostic {
    match err {
        LoadError::YamlSyntax {
            path,
            line,
            message,
            ..
        } => LintDiagnostic {
            file: path,
            line: line.unwrap_or(1),
            message,
        },
        LoadError::Io { path, source } => LintDiagnostic {
            file: path,
            line: 1,
            message: source.to_string(),
        },
        other => LintDiagnostic {
            file: default_path.to_path_buf(),
            line: find_line_containing(raw_src, "include").unwrap_or(1),
            message: other.to_string(),
        },
    }
}

fn check_benchmarks(
    suite: &Suite,
    inventory: &Inventory,
    suite_path: &Path,
    suite_src: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    let known_benchmarks: BTreeSet<&str> =
        suite.benchmarks.iter().map(|b| b.name.as_str()).collect();

    for bench in &suite.benchmarks {
        let bench_line = find_line_containing(suite_src, &format!("name: {}", bench.name))
            .or_else(|| find_line_containing(suite_src, &bench.name))
            .unwrap_or(1);

        // Check machine reference
        if let Some(MachineTarget::Name(m_spec)) = &bench.machine {
            if resolve_machine_node(inventory, m_spec).is_none() {
                let line = find_line_containing(suite_src, m_spec).unwrap_or(bench_line);
                diags.push(LintDiagnostic {
                    file: suite_path.to_path_buf(),
                    line,
                    message: format!(
                        "benchmark `{}` references unknown machine `{m_spec}`",
                        bench.name
                    ),
                });
            }
        }

        // Check build reference and R-BLD-3 (profile-mode requires build with `debug_info: required`)
        let mode = bench.mode.unwrap_or(Mode::Measure);
        if let Some(build_name) = &bench.build {
            match suite.builds.get(build_name) {
                None => {
                    let line = find_line_containing(suite_src, build_name).unwrap_or(bench_line);
                    diags.push(LintDiagnostic {
                        file: suite_path.to_path_buf(),
                        line,
                        message: format!(
                            "benchmark `{}` references unknown build `{build_name}`",
                            bench.name
                        ),
                    });
                }
                Some(build_def) => {
                    if mode == Mode::Profile
                        && build_def.debug_info != Some(DebugInfoPolicy::Required)
                    {
                        let line =
                            find_line_containing(suite_src, build_name).unwrap_or(bench_line);
                        diags.push(LintDiagnostic {
                            file: suite_path.to_path_buf(),
                            line,
                            message: format!(
                                "profile-mode benchmark `{}` uses build `{build_name}` without `debug_info: required` (R-BLD-3)",
                                bench.name
                            ),
                        });
                    }
                }
            }
        } else if mode == Mode::Profile {
            diags.push(LintDiagnostic {
                file: suite_path.to_path_buf(),
                line: bench_line,
                message: format!(
                    "profile-mode benchmark `{}` must name a build with `debug_info: required` (R-BLD-3)",
                    bench.name
                ),
            });
        }

        // Check `from` reference
        if let Some(from_bench) = &bench.from {
            if !known_benchmarks.contains(from_bench.as_str()) {
                let line = find_line_containing(suite_src, from_bench).unwrap_or(bench_line);
                diags.push(LintDiagnostic {
                    file: suite_path.to_path_buf(),
                    line,
                    message: format!(
                        "benchmark `{}` derives `from` unknown benchmark `{from_bench}`",
                        bench.name
                    ),
                });
            }
        }

        // Check patch-set references if `patch` or `patchset` appears in `params` or `axes`
        if let Some(patches_cfg) = &suite.patches {
            if !patches_cfg.sets.is_empty() {
                check_patch_values(
                    bench,
                    &patches_cfg.sets,
                    suite_path,
                    suite_src,
                    bench_line,
                    diags,
                );
            }
        }

        // Check step names and R-STP-2 (exactly one measured step per benchmark unless `measure: [steps]` is set)
        let mut measured_count = 0usize;
        for step_name in &bench.steps {
            match suite.steps.get(step_name) {
                None => {
                    let line = find_line_containing(suite_src, step_name).unwrap_or(bench_line);
                    diags.push(LintDiagnostic {
                        file: suite_path.to_path_buf(),
                        line,
                        message: format!(
                            "benchmark `{}` references unknown step `{step_name}`",
                            bench.name
                        ),
                    });
                }
                Some(step_def) => {
                    if step_def.measured == Some(true) {
                        measured_count += 1;
                    }
                }
            }
        }
        if bench.measure.is_empty() && !bench.steps.is_empty() && measured_count != 1 {
            diags.push(LintDiagnostic {
                file: suite_path.to_path_buf(),
                line: bench_line,
                message: format!(
                    "benchmark `{}` must have exactly one measured step (found {measured_count}) unless `measure:` is set (R-STP-2)",
                    bench.name
                ),
            });
        }

        // Collect known parameters for this benchmark (including `from` benchmark if applicable)
        let known_params = collect_known_params(bench, suite);

        // Check `derive` expressions and references
        for (d_key, expr_src) in &bench.derive {
            let line = find_line_containing(suite_src, expr_src).unwrap_or(bench_line);
            check_expression_refs(
                expr_src,
                &known_params,
                suite,
                suite_path,
                line,
                &format!("benchmark `{}` derive `{d_key}`", bench.name),
                diags,
            );
        }

        // Check `where` expression and references
        if let Some(where_src) = bench
            .filter_where
            .as_deref()
            .or(bench.where_expr.as_deref())
        {
            let line = find_line_containing(suite_src, where_src).unwrap_or(bench_line);
            check_expression_refs(
                where_src,
                &known_params,
                suite,
                suite_path,
                line,
                &format!("benchmark `{}` where", bench.name),
                diags,
            );
        }

        // Check step `${{ ... }}` expressions in the context of this benchmark
        for step_name in &bench.steps {
            if let Some(step_def) = suite.steps.get(step_name) {
                let known_files: BTreeSet<&str> =
                    step_def.files.keys().map(String::as_str).collect();
                if let Some(run_cmd) = &step_def.run {
                    check_interpolated_refs(
                        run_cmd,
                        &known_params,
                        &known_files,
                        suite,
                        suite_path,
                        suite_src,
                        bench_line,
                        &format!("step `{step_name}`"),
                        diags,
                    );
                }
                if let Some(res) = &step_def.resources {
                    for ioe in [
                        res.nodes.as_ref(),
                        res.ranks_per_node.as_ref(),
                        res.threads_per_rank.as_ref(),
                        res.gpus_per_rank.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if let IntOrExpr::Expr(s) = ioe {
                            check_interpolated_refs(
                                s,
                                &known_params,
                                &known_files,
                                suite,
                                suite_path,
                                suite_src,
                                bench_line,
                                &format!("step `{step_name}` resources"),
                                diags,
                            );
                        }
                    }
                }
            }
        }
    }
}

fn check_patch_values(
    bench: &BenchmarkDef,
    sets: &BTreeMap<String, Vec<String>>,
    suite_path: &Path,
    suite_src: &str,
    bench_line: usize,
    diags: &mut Vec<LintDiagnostic>,
) {
    for key in ["patch", "patchset"] {
        if let Some(calo_schema::ScalarValue::Str(s)) = bench.params.get(key) {
            if !sets.contains_key(s) {
                let line = find_line_containing(suite_src, s).unwrap_or(bench_line);
                diags.push(LintDiagnostic {
                    file: suite_path.to_path_buf(),
                    line,
                    message: format!(
                        "benchmark `{}` references unknown patch set `{s}`",
                        bench.name
                    ),
                });
            }
        }
        if let Some(vals) = bench.axes.get(key) {
            for v in vals {
                if let calo_schema::ScalarValue::Str(s) = v {
                    if !sets.contains_key(s) {
                        let line = find_line_containing(suite_src, s).unwrap_or(bench_line);
                        diags.push(LintDiagnostic {
                            file: suite_path.to_path_buf(),
                            line,
                            message: format!(
                                "benchmark `{}` axis `{key}` references unknown patch set `{s}`",
                                bench.name
                            ),
                        });
                    }
                }
            }
        }
    }
}

fn collect_known_params(bench: &BenchmarkDef, suite: &Suite) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(from_name) = &bench.from {
        if let Some(parent) = suite.benchmarks.iter().find(|b| &b.name == from_name) {
            out.extend(parent.params.keys().cloned());
            out.extend(parent.axes.keys().cloned());
            out.extend(parent.derive.keys().cloned());
            out.extend(parent.defaults.keys().cloned());
        }
    }
    out.extend(bench.params.keys().cloned());
    out.extend(bench.axes.keys().cloned());
    out.extend(bench.derive.keys().cloned());
    out.extend(bench.defaults.keys().cloned());
    for inc in &bench.include {
        out.extend(inc.keys().cloned());
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn check_interpolated_refs(
    template: &str,
    known_params: &BTreeSet<String>,
    known_files: &BTreeSet<&str>,
    suite: &Suite,
    suite_path: &Path,
    suite_src: &str,
    fallback_line: usize,
    context: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    let mut cursor = 0;
    while let Some(rel_start) = template[cursor..].find("${{") {
        let open_pos = cursor + rel_start;
        let expr_start = open_pos + 3;
        let Some(rel_end) = template[expr_start..].find("}}") else {
            let line = find_line_containing(suite_src, template).unwrap_or(fallback_line);
            diags.push(LintDiagnostic {
                file: suite_path.to_path_buf(),
                line,
                message: format!("unclosed `${{{{` in {context}"),
            });
            break;
        };
        let expr_end = expr_start + rel_end;
        let expr_str = &template[expr_start..expr_end];
        let line = find_line_containing(suite_src, expr_str).unwrap_or(fallback_line);
        check_expression_refs_with_files(
            expr_str,
            known_params,
            known_files,
            suite,
            suite_path,
            line,
            context,
            diags,
        );
        cursor = expr_end + 2;
    }
}

fn check_expression_refs(
    expr_str: &str,
    known_params: &BTreeSet<String>,
    suite: &Suite,
    suite_path: &Path,
    line: usize,
    context: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    check_expression_refs_with_files(
        expr_str,
        known_params,
        &BTreeSet::new(),
        suite,
        suite_path,
        line,
        context,
        diags,
    );
}

#[allow(clippy::too_many_arguments)]
fn check_expression_refs_with_files(
    expr_str: &str,
    known_params: &BTreeSet<String>,
    known_files: &BTreeSet<&str>,
    suite: &Suite,
    suite_path: &Path,
    line: usize,
    context: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    let ast = match parse(expr_str) {
        Ok(a) => a,
        Err(e) => {
            diags.push(LintDiagnostic {
                file: suite_path.to_path_buf(),
                line,
                message: format!("invalid expression in {context}: {e}"),
            });
            return;
        }
    };

    for r in references(&ast) {
        if !is_valid_reference(&r, known_params, known_files, suite) {
            diags.push(LintDiagnostic {
                file: suite_path.to_path_buf(),
                line,
                message: format!("unknown reference `{r}` in {context}"),
            });
        }
    }
}

fn is_valid_reference(
    r: &str,
    known_params: &BTreeSet<String>,
    known_files: &BTreeSet<&str>,
    suite: &Suite,
) -> bool {
    if let Some(p) = r.strip_prefix("params.") {
        return known_params.contains(p);
    }
    if known_params.contains(r) {
        return true;
    }
    if let Some(f) = r.strip_prefix("files.") {
        return known_files.contains(f);
    }
    if r.starts_with("machine.") || r.starts_with("node.") {
        return is_valid_machine_or_node_ref(r);
    }
    if r.starts_with("env.") || r.starts_with("run.") || r.starts_with("reported.") {
        return true;
    }
    if r.starts_with("phase.") || r.starts_with("region.") {
        return true;
    }
    if r == "calo.link_flags" {
        return true;
    }
    if BUILTIN_METRICS.contains(&r) || BUILTIN_SERVICES.contains(&r) {
        return true;
    }
    if suite.services.contains_key(r) {
        return true;
    }
    for svc in suite.services.values() {
        if svc.produces.iter().any(|p| p == r) {
            return true;
        }
    }
    false
}

fn is_valid_machine_or_node_ref(r: &str) -> bool {
    let node_sub = r
        .strip_prefix("machine.node.")
        .or_else(|| r.strip_prefix("node."));
    if let Some(sub) = node_sub {
        return matches!(
            sub,
            "cpu.model"
                | "cpu.microarch"
                | "cpu.sockets"
                | "cpu.cores"
                | "cpu.threads"
                | "cpu.smt"
                | "cpu.isa"
                | "cache.l2_kb"
                | "cache.l3_mb"
                | "memory.gb"
                | "memory.channels"
                | "memory.numa_nodes"
                | "gpu.vendor"
                | "gpu.model"
                | "gpu.arch"
                | "gpu.per_node"
                | "gpu.mem_gb"
                | "gpu.link"
        );
    }
    matches!(
        r,
        "machine.fingerprint"
            | "machine.workdir"
            | "machine.network.type"
            | "machine.network.bw_gbps"
            | "machine.network.topology"
            | "machine.launcher.gpu_aware_mpi"
    )
}

fn check_services(
    suite: &Suite,
    suite_path: &Path,
    suite_src: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    // Collect all parameter keys across the suite for expression-defined services
    let mut all_suite_params = BTreeSet::new();
    for b in &suite.benchmarks {
        all_suite_params.extend(collect_known_params(b, suite));
    }

    // Map from produced metric/service name -> service name
    let mut producers: BTreeMap<String, String> = BTreeMap::new();
    for &b_svc in BUILTIN_SERVICES {
        producers.insert(b_svc.to_string(), b_svc.to_string());
    }
    for (svc_name, cfg) in &suite.services {
        producers.insert(svc_name.clone(), svc_name.clone());
        for prod in &cfg.produces {
            producers.insert(prod.clone(), svc_name.clone());
        }
    }

    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for (svc_name, cfg) in &suite.services {
        let svc_line = find_line_containing(suite_src, &format!("{svc_name}:"))
            .or_else(|| find_line_containing(suite_src, svc_name))
            .unwrap_or(1);

        let mut svc_deps = BTreeSet::new();

        if let Some(expr_src) = &cfg.expr {
            let expr_line = find_line_containing(suite_src, expr_src).unwrap_or(svc_line);
            check_expression_refs(
                expr_src,
                &all_suite_params,
                suite,
                suite_path,
                expr_line,
                &format!("service `{svc_name}`"),
                diags,
            );
            if let Ok(ast) = parse(expr_src) {
                for r in references(&ast) {
                    if let Some(provider) = producers.get(&r) {
                        svc_deps.insert(provider.clone());
                    }
                }
            }
        }

        for inp in &cfg.inputs {
            if let Some(provider) = producers.get(inp) {
                svc_deps.insert(provider.clone());
            } else if !is_valid_reference(inp, &all_suite_params, &BTreeSet::new(), suite) {
                diags.push(LintDiagnostic {
                    file: suite_path.to_path_buf(),
                    line: svc_line,
                    message: format!("service `{svc_name}` references unknown input `{inp}`"),
                });
            }
        }

        deps.insert(svc_name.clone(), svc_deps);
    }

    // Detect service dependency cycles (spec §12.2 R-SVC-2, §14)
    let mut resolved = BTreeSet::new();
    let mut remaining: BTreeSet<String> = deps.keys().cloned().collect();

    while !remaining.is_empty() {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|k| {
                deps[*k]
                    .iter()
                    .all(|d| !remaining.contains(d) || resolved.contains(d))
            })
            .cloned()
            .collect();

        if ready.is_empty() {
            let cycle_list: Vec<String> = remaining.into_iter().collect();
            let first = &cycle_list[0];
            let line = find_line_containing(suite_src, &format!("{first}:")).unwrap_or(1);
            diags.push(LintDiagnostic {
                file: suite_path.to_path_buf(),
                line,
                message: format!(
                    "service dependency cycle detected involving: {}",
                    cycle_list.join(" -> ")
                ),
            });
            break;
        }

        for k in ready {
            remaining.remove(&k);
            resolved.insert(k);
        }
    }
}

fn check_reports(
    suite: &Suite,
    inventory: &Inventory,
    suite_path: &Path,
    suite_src: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    // Collect top-level reports plus benchmark-nested shorthand reports (spec §13 R-REP-2)
    let mut all_reports = suite.reports.clone();
    for b in &suite.benchmarks {
        for mut rep in b.reports.clone() {
            if rep.select.is_none() {
                rep.select = Some(ReportSelect::Single(calo_schema::suite::SelectorClause {
                    benchmark: Some(b.name.clone()),
                    ..Default::default()
                }));
            }
            all_reports.push(rep);
        }
    }

    for rep in &all_reports {
        if rep.requires.is_empty() {
            continue;
        }
        let rep_line = find_line_containing(suite_src, &format!("name: {}", rep.name))
            .or_else(|| find_line_containing(suite_src, &rep.name))
            .unwrap_or(1);

        let selector = rep.select.as_ref().map_or(Selector::All, Selector::from);

        let mut matched_any = false;
        for bench in &suite.benchmarks {
            let mode = bench.mode.unwrap_or(Mode::Measure);
            let tiers = max_tiers_for_benchmark(bench, mode);
            let m_spec = match &bench.machine {
                Some(MachineTarget::Name(s)) => s.as_str(),
                _ => "",
            };
            let (mach_opt, node_opt) =
                resolve_machine_node(inventory, m_spec).map_or((None, None), |(m, n)| (Some(m), n));

            let desc = RunDescriptor {
                benchmark: &bench.name,
                group: bench.group.as_deref(),
                mode,
                params: &bench.params,
                tiers: &tiers,
                machine: mach_opt,
                node: node_opt,
            };

            if matches(&selector, &desc) {
                matched_any = true;
                for req_tier in &rep.requires {
                    if !tiers.satisfies(*req_tier) {
                        diags.push(LintDiagnostic {
                            file: suite_path.to_path_buf(),
                            line: rep_line,
                            message: format!(
                                "report `{}` requires tier `{req_tier}`, which is not enabled by selected benchmark `{}` (mode `{mode:?}`)",
                                rep.name, bench.name
                            ),
                        });
                    }
                }
            }
        }

        if !matched_any && !suite.benchmarks.is_empty() {
            diags.push(LintDiagnostic {
                file: suite_path.to_path_buf(),
                line: rep_line,
                message: format!(
                    "report `{}` requires {:?}, but no benchmarks match its selector",
                    rep.name, rep.requires
                ),
            });
        }
    }
}

/// Computes the capability tiers enabled by a benchmark's `mode` and `probe` config (spec §2, §8.5 R-BEN-3).
fn max_tiers_for_benchmark(bench: &BenchmarkDef, mode: Mode) -> TiersActive {
    match mode {
        Mode::Measure => {
            // Spec §8.5 R-BEN-3: `measure`: no shims, no sampling. T0, G0, M0 only.
            let _ = Tier::T0;
            TiersActive {
                cpu: Some(CpuTier::T0),
                mpi: Some(MpiTier::M0),
                gpu: Some(GpuTier::G0),
            }
        }
        Mode::Profile => {
            let mpi_tier = match bench.probe.as_ref().and_then(|p| p.mpi) {
                Some(calo_schema::suite::BenchmarkMpiMode::Off) => MpiTier::M0,
                Some(calo_schema::suite::BenchmarkMpiMode::Events) => MpiTier::M2,
                Some(calo_schema::suite::BenchmarkMpiMode::Summary) | None => MpiTier::M1,
            };
            let gpu_tier = match bench
                .probe
                .as_ref()
                .and_then(|p| p.gpu.as_ref())
                .and_then(|g| g.trace)
            {
                Some(calo_schema::runspec::GpuTraceMode::Off) => GpuTier::G0,
                Some(calo_schema::runspec::GpuTraceMode::Activity) | None => GpuTier::G1,
            };
            TiersActive {
                cpu: Some(CpuTier::T2),
                mpi: Some(mpi_tier),
                gpu: Some(gpu_tier),
            }
        }
        Mode::Kernel => TiersActive {
            cpu: Some(CpuTier::T0),
            mpi: Some(MpiTier::M0),
            gpu: Some(GpuTier::G2),
        },
    }
}

fn find_line_containing(src: &str, needle: &str) -> Option<usize> {
    let trimmed = needle.trim();
    if trimmed.is_empty() {
        return None;
    }
    for (idx, line) in src.lines().enumerate() {
        if line.contains(trimmed) {
            return Some(idx + 1);
        }
    }
    None
}
