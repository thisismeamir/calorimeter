//! `RunSpec` construction, template file rendering, counter/affinity resolution, and launcher argv (spec §7, §8.4, §9.9).

use crate::exec::error::ExecError;
use crate::expr::{interpolate, render_template, Env, Value};
use crate::plan::selector::resolve_machine_node;
use crate::plan::{PinnedRun};
use calo_schema::common::{Mode, ScalarValue};
use calo_schema::inventory::Inventory;
use calo_schema::record::RenderedFile;
use calo_schema::runspec::{
    GpuTraceMode, MpiTraceMode, RunSpec, RunSpecAffinity, RunSpecGpu, RunSpecMpi, RunSpecOutputs,
    RunSpecRank,
};
use calo_schema::suite::{BenchmarkMpiMode, IntOrExpr, StepDef, Suite};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Built-in hardware counter alias table (spec §7 R-INV-4).
pub fn builtin_counter_alias(alias: &str) -> &str {
    match alias {
        "cycles" => "cpu-cycles",
        "instructions" => "instructions",
        "cache-misses" => "cache-misses",
        "branch-misses" => "branch-misses",
        "l1d-misses" => "L1-dcache-load-misses",
        "llc-misses" => "LLC-load-misses",
        "stalled-backend" => "stalled-cycles-backend",
        other => other,
    }
}

/// Resolves requested counter aliases through `inventory.probe.counters` overrides and the built-in table (spec §7 R-INV-4).
pub fn resolve_counter_aliases(
    aliases: &[String],
    overrides: &BTreeMap<String, String>,
) -> Vec<String> {
    aliases
        .iter()
        .map(|a| {
            overrides
                .get(a)
                .cloned()
                .unwrap_or_else(|| builtin_counter_alias(a).to_string())
        })
        .collect()
}

/// Parses a Linux CPU list string (e.g. `"0-3,8"`) into a sorted set of CPU IDs.
pub fn parse_cpu_list(spec: &str) -> BTreeSet<u32> {
    let mut set = BTreeSet::new();
    for part in spec.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((start_s, end_s)) = trimmed.split_once('-') {
            if let (Ok(s), Ok(e)) = (start_s.trim().parse::<u32>(), end_s.trim().parse::<u32>()) {
                for id in s..=e {
                    set.insert(id);
                }
            }
        } else if let Ok(id) = trimmed.parse::<u32>() {
            set.insert(id);
        }
    }
    set
}

/// Formats a sorted set of CPU IDs as a compact Linux CPU list string (e.g. `"4-63"`).
pub fn format_cpu_list(cpus: &BTreeSet<u32>) -> String {
    let mut ranges = Vec::new();
    let mut iter = cpus.iter().copied();
    let Some(mut start) = iter.next() else {
        return String::new();
    };
    let mut end = start;
    for id in iter {
        if id == end + 1 {
            end = id;
        } else {
            if start == end {
                ranges.push(start.to_string());
            } else {
                ranges.push(format!("{start}-{end}"));
            }
            start = id;
            end = id;
        }
    }
    if start == end {
        ranges.push(start.to_string());
    } else {
        ranges.push(format!("{start}-{end}"));
    }
    ranges.join(",")
}

/// Removes `isolate_cpu_list` from `0..total_cpus` and returns the remaining workload CPU list (spec §3 I2, §7).
pub fn subtract_isolated_cpus(total_cpus: u32, isolate_cpu_list: &str) -> String {
    let isolated = parse_cpu_list(isolate_cpu_list);
    let available: BTreeSet<u32> = (0..total_cpus)
        .filter(|id| !isolated.contains(id))
        .collect();
    format_cpu_list(&available)
}

struct StepExprEnv<'a> {
    params: &'a BTreeMap<String, ScalarValue>,
    files: &'a BTreeMap<String, RenderedFile>,
}

impl Env for StepExprEnv<'_> {
    fn get(&self, path: &str) -> Option<Value> {
        if let Some(p) = path.strip_prefix("params.") {
            return self.params.get(p).map(Value::from_scalar);
        }
        if let Some(f) = path.strip_prefix("files.") {
            return self.files.get(f).map(|rf| Value::Str(rf.path.clone()));
        }
        self.params.get(path).map(Value::from_scalar)
    }
}

/// Renders all `step.files` templates into `inputs_dir` and returns `name -> RenderedFile` with SHA-256 (spec §8.4 R-STP-3, R-STP-8).
pub fn render_step_files(
    step: &StepDef,
    params: &BTreeMap<String, ScalarValue>,
    suite_root: &Path,
    inputs_dir: &Path,
) -> Result<BTreeMap<String, RenderedFile>, ExecError> {
    let mut rendered = BTreeMap::new();
    if step.files.is_empty() {
        return Ok(rendered);
    }

    fs::create_dir_all(inputs_dir).map_err(|source| ExecError::Io {
        path: inputs_dir.to_path_buf(),
        source,
    })?;

    let _env = StepExprEnv {
        params,
        files: &rendered,
    };

    for (name, tmpl_cfg) in &step.files {
        let tmpl_path = suite_root.join(&tmpl_cfg.template);
        let tmpl_src = fs::read_to_string(&tmpl_path).map_err(|source| ExecError::Io {
            path: tmpl_path.clone(),
            source,
        })?;
        let env = StepExprEnv {
            params,
            files: &rendered,
        };

        let mut vars = BTreeMap::new();
        for (vk, vv) in &tmpl_cfg.vars {
            let resolved_v = match vv {
                ScalarValue::Str(s) if s.contains("${{") => {
                    let interp = interpolate(s, &env)?;
                    if let Ok(i) = interp.parse::<i64>() {
                        ScalarValue::Int(i)
                    } else if let Ok(f) = interp.parse::<f64>() {
                        ScalarValue::Float(f)
                    } else {
                        ScalarValue::Str(interp)
                    }
                }
                other => other.clone(),
            };
            vars.insert(vk.clone(), resolved_v);
        }

        let body = render_template(&tmpl_src, &vars)?;
        let out_file = inputs_dir.join(format!("{name}.in"));
        fs::write(&out_file, body.as_bytes()).map_err(|source| ExecError::Io {
            path: out_file.clone(),
            source,
        })?;

        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        let sha256 = format!("{:x}", hasher.finalize());

        rendered.insert(
            name.clone(),
            RenderedFile {
                path: out_file.display().to_string(),
                sha256,
            },
        );
    }

    Ok(rendered)
}

/// Constructs a `calo.runspec/1` for `run`, enforcing `measure`-mode restrictions (spec §8.5 R-BEN-3, §9.9).
pub fn build_runspec(
    run: &PinnedRun,
    suite: &Suite,
    inventory: &Inventory,
    rendered_files: &BTreeMap<String, RenderedFile>,
    outputs_dir: &Path,
    cwd: &Path,
    channel_path: Option<&Path>,
) -> Result<RunSpec, ExecError> {
    let bench = suite
        .benchmarks
        .iter()
        .find(|b| b.name == run.benchmark)
        .ok_or_else(|| ExecError::MissingTarget {
            kind: "benchmark",
            name: run.benchmark.clone(),
        })?;

    let measured_step_name = bench
        .steps
        .iter()
        .find(|s| {
            suite
                .steps
                .get(*s)
                .is_some_and(|st| st.measured == Some(true))
                || bench.measure.contains(*s)
        })
        .or_else(|| bench.steps.first())
        .ok_or_else(|| ExecError::MissingTarget {
            kind: "step",
            name: format!("{} (no steps)", bench.name),
        })?;

    let step = suite
        .steps
        .get(measured_step_name)
        .ok_or_else(|| ExecError::MissingTarget {
            kind: "step",
            name: measured_step_name.clone(),
        })?;

    let (mach_entry, node_entry) =
        resolve_machine_node(inventory, &run.machine).map_or((None, None), |(m, n)| (Some(m), n));

    let env_ctx = StepExprEnv {
        params: &run.params,
        files: rendered_files,
    };

    let raw_cmd = step.run.as_deref().unwrap_or("");
    let cmd_str = interpolate(raw_cmd, &env_ctx)?;
    let command = if cmd_str.trim().is_empty() {
        Vec::new()
    } else {
        vec!["sh".to_string(), "-c".to_string(), cmd_str]
    };

    let mut env = mach_entry.map(|m| m.env.clone()).unwrap_or_default();
    if let Some(res) = &step.resources {
        if let Some(tpr) = &res.threads_per_rank {
            let t = eval_int_or_expr(tpr, &env_ctx)?;
            env.insert("OMP_NUM_THREADS".to_string(), t.to_string());
        }
    }

    // Affinity: remove `policy.isolate.cpu_list` from total node CPUs (spec §3 I2)
    let affinity = mach_entry
        .and_then(|m| m.policy.as_ref())
        .and_then(|p| p.isolate.as_ref())
        .map(|iso| {
            let total_cpus = node_entry
                .and_then(|n| n.cpu.as_ref())
                .and_then(|c| c.threads.or(c.cores))
                .unwrap_or(64);
            RunSpecAffinity {
                cpu_list: Some(subtract_isolated_cpus(total_cpus, &iso.cpu_list)),
                numa: None,
            }
        });

    let probe_cfg = bench.probe.as_ref();
    let counter_overrides = mach_entry
        .and_then(|m| m.probe.as_ref())
        .map(|p| &p.counters);
    let counters = probe_cfg.map_or_else(Vec::new, |p| {
        if let Some(ov) = counter_overrides {
            resolve_counter_aliases(&p.counters, ov)
        } else {
            resolve_counter_aliases(&p.counters, &BTreeMap::new())
        }
    });

    // Enforce R-BEN-3: `measure` mode forbids CPU sampling and shims
    if run.mode == Mode::Measure {
        if let Some(p) = probe_cfg {
            if p.profile.is_some() {
                return Err(ExecError::InvalidModeConfig {
                    benchmark: run.benchmark.clone(),
                    feature: "CPU sampling (`probe.profile`)",
                });
            }
            if matches!(
                p.mpi,
                Some(BenchmarkMpiMode::Summary | BenchmarkMpiMode::Events)
            ) {
                return Err(ExecError::InvalidModeConfig {
                    benchmark: run.benchmark.clone(),
                    feature: "MPI shim (`probe.mpi: summary|events`)",
                });
            }
            if p.gpu.as_ref().and_then(|g| g.trace) == Some(GpuTraceMode::Activity) {
                return Err(ExecError::InvalidModeConfig {
                    benchmark: run.benchmark.clone(),
                    feature: "GPU activity tracing (`probe.gpu.trace: activity`)",
                });
            }
        }
    }

    let mpi = match run.mode {
        Mode::Measure | Mode::Kernel => Some(RunSpecMpi {
            mode: MpiTraceMode::Off,
            trace_max_events: None,
        }),
        Mode::Profile => {
            let m = match probe_cfg.and_then(|p| p.mpi) {
                Some(BenchmarkMpiMode::Off) => MpiTraceMode::Off,
                Some(BenchmarkMpiMode::Events) => MpiTraceMode::Events,
                Some(BenchmarkMpiMode::Summary) | None => MpiTraceMode::Summary,
            };
            Some(RunSpecMpi {
                mode: m,
                trace_max_events: None,
            })
        }
    };

    let gpu = match run.mode {
        Mode::Measure => {
            let mut g = probe_cfg.and_then(|p| p.gpu.clone()).unwrap_or_default();
            g.trace = Some(GpuTraceMode::Off);
            Some(g)
        }
        Mode::Profile | Mode::Kernel => probe_cfg.and_then(|p| p.gpu.clone()).or_else(|| {
            Some(RunSpecGpu {
                state: Some(true),
                trace: Some(GpuTraceMode::Activity),
                ..RunSpecGpu::default()
            })
        }),
    };

    let profile = if run.mode == Mode::Profile {
        probe_cfg.and_then(|p| p.profile.clone())
    } else {
        None
    };

    let has_resources = step.resources.is_some();

    Ok(RunSpec {
        schema: <RunSpec as calo_schema::SchemaDoc>::SCHEMA.to_string(),
        run_id: run.run_id.clone(),
        mode: run.mode,
        command,
        cwd: cwd.display().to_string(),
        env,
        affinity,
        rlimits: None,
        counters,
        sample_hz: probe_cfg.and_then(|p| p.sample_hz),
        profile,
        mpi,
        gpu,
        shim_inject: None,
        regions: None,
        channel: channel_path.map(|p| p.display().to_string()),
        rank: if has_resources {
            Some(RunSpecRank { from_env: true })
        } else {
            None
        },
        outputs: RunSpecOutputs {
            dir: outputs_dir.display().to_string(),
        },
    })
}

/// Builds the execution argv vector (`<launcher...> probe --rank-aware --runspec <file>` or `probe --runspec <file>`, spec §8.4 R-STP-4).
pub fn build_launcher_argv(
    probe_bin: &Path,
    runspec_path: &Path,
    step: &StepDef,
    inventory: &Inventory,
    machine_name: &str,
    params: &BTreeMap<String, ScalarValue>,
) -> Result<Vec<String>, ExecError> {
    let Some(res) = &step.resources else {
        return Ok(vec![
            probe_bin.display().to_string(),
            "--runspec".to_string(),
            runspec_path.display().to_string(),
        ]);
    };

    let empty_files = BTreeMap::new();
    let env_ctx = StepExprEnv {
        params,
        files: &empty_files,
    };

    let nodes = res
        .nodes
        .as_ref()
        .map(|v| eval_int_or_expr(v, &env_ctx))
        .transpose()?
        .unwrap_or(1);
    let rpn = res
        .ranks_per_node
        .as_ref()
        .map(|v| eval_int_or_expr(v, &env_ctx))
        .transpose()?
        .unwrap_or(1);
    let ranks = nodes * rpn;

    let (mach_opt, _) =
        resolve_machine_node(inventory, machine_name).map_or((None, None), |(m, n)| (Some(m), n));
    let launcher_tmpl = mach_opt
        .and_then(|m| m.launcher.as_ref())
        .and_then(|l| l.mpi.as_deref())
        .unwrap_or("mpirun -np {ranks}");

    let expanded = launcher_tmpl
        .replace("{ranks}", &ranks.to_string())
        .replace("{ranks_per_node}", &rpn.to_string());

    let mut argv: Vec<String> = expanded.split_whitespace().map(String::from).collect();
    argv.push(probe_bin.display().to_string());
    argv.push("--rank-aware".to_string());
    argv.push("--runspec".to_string());
    argv.push(runspec_path.display().to_string());
    Ok(argv)
}

/// Locates the `probe` binary via `--probe`, `$CALO_PROBE`, or sibling of the `calo` executable (spec §3 I2).
pub fn resolve_probe_binary(explicit: Option<&Path>) -> Result<PathBuf, ExecError> {
    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(ExecError::ProbeNotFound);
    }
    if let Ok(env_p) = std::env::var("CALO_PROBE") {
        let p = PathBuf::from(env_p);
        if p.exists() {
            return Ok(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("probe");
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }
    Err(ExecError::ProbeNotFound)
}

fn eval_int_or_expr(val: &IntOrExpr, env: &StepExprEnv<'_>) -> Result<u32, ExecError> {
    match val {
        IntOrExpr::Int(v) => Ok(*v),
        IntOrExpr::Expr(s) => {
            let rendered = interpolate(s, env)?;
            rendered
                .trim()
                .parse::<u32>()
                .map_err(|_| ExecError::MissingTarget {
                    kind: "integer resource",
                    name: rendered,
                })
        }
    }
}
