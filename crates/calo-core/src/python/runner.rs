//! CSV wire protocol, Python detection, timeout & secret-redaction isolation (spec §12.4).

use crate::proc::{CommandRunner, CommandSpec};
use crate::services::{Column, Frame, Options, ServiceError};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Configuration for executing out-of-process Python services and plots (spec §12.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonRunnerConfig {
    /// Path to `python/calo_runner.py`.
    pub runner_script: PathBuf,
    /// Timeout in seconds (`options.python_timeout_s`, default `300`).
    pub timeout_s: u64,
    /// Environment variable names to strip from child process (`options.redact`, spec R-SAF-5).
    pub redact: Vec<String>,
    /// Optional explicit Python interpreter path (defaults to `"python3"`).
    pub python_bin: String,
}

impl Default for PythonRunnerConfig {
    fn default() -> Self {
        Self {
            runner_script: PathBuf::from("python/calo_runner.py"),
            timeout_s: 300,
            redact: Vec::new(),
            python_bin: "python3".to_string(),
        }
    }
}

/// Checks whether `python3` is available via `CommandRunner` (spec §12.4 R-PY-1).
pub fn detect_python(python_bin: &str, runner: &dyn CommandRunner) -> Result<String, ServiceError> {
    let spec = CommandSpec::new(python_bin, vec!["--version".to_string()]);
    match runner.run(&spec) {
        Ok(out) if out.success() => Ok(out.stdout.trim().to_string()),
        Ok(out) => Err(ServiceError::PythonUnavailable {
            service: "python3".to_string(),
            reason: format!("python3 check exited with {:?}: {}", out.exit_code, out.stderr.trim()),
        }),
        Err(e) => Err(ServiceError::PythonUnavailable {
            service: "python3".to_string(),
            reason: e.to_string(),
        }),
    }
}

/// Executes an out-of-process Python service (`pkg.module:function`) over `frame`,
/// returning a `Frame` with the validated `produces` columns (spec §12.4 R-PY-1).
#[allow(clippy::too_many_arguments)]
pub fn run_python_service(
    service_name: &str,
    entrypoint: &str,
    inputs: &[String],
    produces: &[String],
    frame: &Frame,
    opts: &Options,
    cfg: &PythonRunnerConfig,
    work_dir: &Path,
    runner: &dyn CommandRunner,
) -> Result<Frame, ServiceError> {
    let (module_name, func_name) =
        entrypoint
            .split_once(':')
            .ok_or_else(|| ServiceError::InvalidInput {
                service: service_name.to_string(),
                reason: format!("invalid python entrypoint `{entrypoint}` (expected `pkg.module:function`)"),
            })?;

    detect_python(&cfg.python_bin, runner)?;

    fs::create_dir_all(work_dir).map_err(|e| ServiceError::InvalidInput {
        service: service_name.to_string(),
        reason: e.to_string(),
    })?;

    let in_csv = work_dir.join("in.csv");
    let out_csv = work_dir.join("out.csv");
    let opts_yaml = work_dir.join("opts.yaml");

    let csv_str = serialize_frame_typed_csv(frame, inputs, service_name)?;
    fs::write(&in_csv, csv_str).map_err(|e| ServiceError::InvalidInput {
        service: service_name.to_string(),
        reason: e.to_string(),
    })?;

    let opts_map = BTreeMap::from([
        ("metric", opts.metric.clone()),
        ("estimator", opts.estimator.clone()),
        ("seed", opts.ci.seed.to_string()),
    ]);
    let opts_str = calo_schema::yaml::to_string(&opts_map).unwrap_or_default();
    fs::write(&opts_yaml, opts_str).map_err(|e| ServiceError::InvalidInput {
        service: service_name.to_string(),
        reason: e.to_string(),
    })?;

    let mut spec = CommandSpec::new(
        &cfg.python_bin,
        vec![
            cfg.runner_script.display().to_string(),
            "--module".to_string(),
            module_name.to_string(),
            "--function".to_string(),
            func_name.to_string(),
            "--in".to_string(),
            in_csv.display().to_string(),
            "--out".to_string(),
            out_csv.display().to_string(),
            "--opts".to_string(),
            opts_yaml.display().to_string(),
        ],
    )
    .with_timeout(Duration::from_secs(cfg.timeout_s.max(1)));

    spec.env_remove = cfg.redact.clone();

    let out = runner.run(&spec).map_err(|e| ServiceError::PythonUnavailable {
        service: service_name.to_string(),
        reason: e.to_string(),
    })?;

    if out.timed_out {
        return Err(ServiceError::PythonUnavailable {
            service: service_name.to_string(),
            reason: format!("timed out after {}s", cfg.timeout_s),
        });
    }
    if !out.success() {
        return Err(ServiceError::PythonUnavailable {
            service: service_name.to_string(),
            reason: format!(
                "exited with {:?}: {}",
                out.exit_code,
                out.stderr.trim()
            ),
        });
    }

    let raw_out = fs::read_to_string(&out_csv).map_err(|e| ServiceError::InvalidInput {
        service: service_name.to_string(),
        reason: format!("failed to read Python output CSV: {e}"),
    })?;

    parse_and_validate_output_csv(service_name, &raw_out, frame, produces)
}

/// Executes an out-of-process Python plot renderer (`plot: { python: pkg.module:function }`, spec §12.4 R-PY-2).
#[allow(clippy::too_many_arguments)]
pub fn run_python_plot(
    plot_name: &str,
    entrypoint: &str,
    frame: &Frame,
    opts: &Options,
    cfg: &PythonRunnerConfig,
    work_dir: &Path,
    svg_out: &Path,
    runner: &dyn CommandRunner,
) -> Result<(), ServiceError> {
    let (module_name, func_name) =
        entrypoint
            .split_once(':')
            .ok_or_else(|| ServiceError::InvalidInput {
                service: plot_name.to_string(),
                reason: format!("invalid python plot entrypoint `{entrypoint}`"),
            })?;

    detect_python(&cfg.python_bin, runner)?;
    fs::create_dir_all(work_dir).map_err(|e| ServiceError::InvalidInput {
        service: plot_name.to_string(),
        reason: e.to_string(),
    })?;

    let in_csv = work_dir.join("plot_in.csv");
    let opts_yaml = work_dir.join("plot_opts.yaml");
    let all_cols: Vec<String> = frame.columns().keys().cloned().collect();
    let csv_str = serialize_frame_typed_csv(frame, &all_cols, plot_name)?;
    fs::write(&in_csv, csv_str).map_err(|e| ServiceError::InvalidInput {
        service: plot_name.to_string(),
        reason: e.to_string(),
    })?;
    fs::write(&opts_yaml, format!("metric: {}\n", opts.metric)).map_err(|e| {
        ServiceError::InvalidInput {
            service: plot_name.to_string(),
            reason: e.to_string(),
        }
    })?;

    let mut spec = CommandSpec::new(
        &cfg.python_bin,
        vec![
            cfg.runner_script.display().to_string(),
            "--module".to_string(),
            module_name.to_string(),
            "--function".to_string(),
            func_name.to_string(),
            "--in".to_string(),
            in_csv.display().to_string(),
            "--svg".to_string(),
            svg_out.display().to_string(),
            "--opts".to_string(),
            opts_yaml.display().to_string(),
        ],
    )
    .with_timeout(Duration::from_secs(cfg.timeout_s.max(1)));
    spec.env_remove = cfg.redact.clone();

    let out = runner.run(&spec).map_err(|e| ServiceError::PythonUnavailable {
        service: plot_name.to_string(),
        reason: e.to_string(),
    })?;
    if out.timed_out || !out.success() {
        return Err(ServiceError::PythonUnavailable {
            service: plot_name.to_string(),
            reason: out.stderr.trim().to_string(),
        });
    }
    Ok(())
}

fn serialize_frame_typed_csv(
    frame: &Frame,
    selected_cols: &[String],
    service_name: &str,
) -> Result<String, ServiceError> {
    let mut cols_to_write: Vec<String> = Vec::new();
    if frame.column("run_id").is_some() && !selected_cols.iter().any(|c| c == "run_id") {
        cols_to_write.push("run_id".to_string());
    }
    for c in selected_cols {
        if frame.column(c).is_none() {
            return Err(ServiceError::MissingColumn {
                service: service_name.to_string(),
                column: c.clone(),
            });
        }
        cols_to_write.push(c.clone());
    }

    let mut out = String::new();
    let header_cells: Vec<String> = cols_to_write
        .iter()
        .map(|name| {
            let t = frame
                .column(name)
                .map_or("str", Column::type_name);
            format!("{name}:{t}")
        })
        .collect();
    let _ = writeln!(out, "{}", header_cells.join(","));

    for r in 0..frame.num_rows() {
        let row_cells: Vec<String> = cols_to_write
            .iter()
            .map(|name| {
                frame
                    .column(name)
                    .and_then(|c| c.get_value(r))
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            })
            .collect();
        let _ = writeln!(out, "{}", row_cells.join(","));
    }

    Ok(out)
}

fn parse_and_validate_output_csv(
    service_name: &str,
    raw_csv: &str,
    input_frame: &Frame,
    produces: &[String],
) -> Result<Frame, ServiceError> {
    let mut lines = raw_csv.lines().filter(|l| !l.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return Err(ServiceError::InvalidInput {
            service: service_name.to_string(),
            reason: "Python output CSV is empty".to_string(),
        });
    };

    let mut col_specs: Vec<(String, String)> = Vec::new();
    for cell in header_line.split(',') {
        let trimmed = cell.trim();
        if let Some((name, ty)) = trimmed.split_once(':') {
            col_specs.push((name.to_string(), ty.to_string()));
        } else {
            col_specs.push((trimmed.to_string(), "float".to_string()));
        }
    }

    for req in produces {
        if !col_specs.iter().any(|(n, _)| n == req) {
            return Err(ServiceError::InvalidInput {
                service: service_name.to_string(),
                reason: format!("Python output missing declared `produces` column `{req}`"),
            });
        }
    }

    let mut raw_cols: Vec<Vec<String>> = vec![Vec::new(); col_specs.len()];
    let mut row_count = 0usize;
    for line in lines {
        let cells: Vec<&str> = line.split(',').collect();
        if cells.len() != col_specs.len() {
            return Err(ServiceError::InvalidInput {
                service: service_name.to_string(),
                reason: format!(
                    "row {} column count {} != header count {}",
                    row_count,
                    cells.len(),
                    col_specs.len()
                ),
            });
        }
        for (i, cell) in cells.into_iter().enumerate() {
            raw_cols[i].push(cell.trim().to_string());
        }
        row_count += 1;
    }

    if row_count != input_frame.num_rows() {
        return Err(ServiceError::InvalidInput {
            service: service_name.to_string(),
            reason: format!(
                "Python output row count {row_count} does not match input row count {}",
                input_frame.num_rows()
            ),
        });
    }

    // Verify `run_id` preservation if both input and output carry `run_id`
    if let (Some(Column::Str(in_ids)), Some(out_pos)) = (
        input_frame.column("run_id"),
        col_specs.iter().position(|(n, _)| n == "run_id"),
    ) {
        if in_ids != &raw_cols[out_pos] {
            return Err(ServiceError::InvalidInput {
                service: service_name.to_string(),
                reason: "Python output altered `run_id` key ordering or values".to_string(),
            });
        }
    }

    let mut out_frame = Frame::new();
    for (idx, (col_name, ty)) in col_specs.iter().enumerate() {
        let vals = &raw_cols[idx];
        let col = match ty.as_str() {
            "int" => {
                let mut parsed = Vec::with_capacity(vals.len());
                for v in vals {
                    let i = v.parse::<i64>().map_err(|_| ServiceError::ColumnType {
                        column: col_name.clone(),
                        expected: "int",
                        actual: "unparseable",
                    })?;
                    parsed.push(i);
                }
                Column::Int(parsed)
            }
            "float" => {
                let mut parsed = Vec::with_capacity(vals.len());
                for v in vals {
                    let f = v.parse::<f64>().map_err(|_| ServiceError::ColumnType {
                        column: col_name.clone(),
                        expected: "float",
                        actual: "unparseable",
                    })?;
                    parsed.push(f);
                }
                Column::Float(parsed)
            }
            "bool" => {
                let mut parsed = Vec::with_capacity(vals.len());
                for v in vals {
                    let b = v.parse::<bool>().map_err(|_| ServiceError::ColumnType {
                        column: col_name.clone(),
                        expected: "bool",
                        actual: "unparseable",
                    })?;
                    parsed.push(b);
                }
                Column::Bool(parsed)
            }
            _ => Column::Str(vals.clone()),
        };
        out_frame.insert_column(col_name.clone(), col)?;
    }

    Ok(out_frame)
}
