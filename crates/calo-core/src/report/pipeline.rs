//! Full analysis pipeline (`analyze`), multi-format report rendering (`md`, `html`, `csv`, `svg`),
//! provenance footers, and `calo diff` comparison (spec §12, §13, §14, §18).

use crate::flame::xml_escape;
use crate::plan::Selector;
use crate::plot::{render_plot, PlotSpec, ScaleKind};
use crate::report::error::ReportError;
use crate::report::gate::evaluate_report_gates;
use crate::report::pivot::{build_pivot_table, extract_record_metric, select_records};
use crate::report::GateOutcome;
use crate::results::write_atomic_bytes;
use crate::services::{Column, Frame, ServiceError, ServiceLayer, ServiceRegistry};
use crate::symbols::Symbolizer;
use crate::table::crc32_ieee;
use calo_schema::common::{ScalarValue, Status};
use calo_schema::record::Record;
use calo_schema::suite::{ReportDef, ReportOutputFormat, Suite};
use calo_schema::table::{ColumnDef, ColumnType, TableHeader};
use calo_schema::{from_yaml_str, to_yaml_string, SchemaDoc};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Summary returned by `run_analysis_pipeline` (`calo analyze`, spec §12.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisReport {
    /// Notices for skipped services (unmet tiers) or skipped symbolization stage.
    pub notices: Vec<String>,
    /// Sidecar file paths written under `analysis/<service>/`.
    pub written_tables: Vec<PathBuf>,
}

/// Converts a slice of `Record`s into a columnar `Frame` for service execution (spec §12.2).
pub fn build_frame_from_records(records: &[&Record]) -> Result<Frame, ServiceError> {
    let mut frame = Frame::new();
    if records.is_empty() {
        return Ok(frame);
    }

    let run_ids: Vec<String> = records.iter().map(|r| r.identity.run_id.clone()).collect();
    let benchmarks: Vec<String> = records.iter().map(|r| r.identity.benchmark.clone()).collect();
    frame.insert_column("run_id", Column::Str(run_ids))?;
    frame.insert_column("benchmark", Column::Str(benchmarks))?;

    // Collect union of param keys
    let mut param_keys = BTreeSet::new();
    for r in records {
        for k in r.identity.params.keys() {
            param_keys.insert(k.as_str());
        }
    }
    for pk in param_keys {
        let mut all_int = true;
        let mut ints = Vec::with_capacity(records.len());
        let mut floats = Vec::with_capacity(records.len());
        for r in records {
            match r.identity.params.get(pk) {
                Some(ScalarValue::Int(i)) => {
                    ints.push(*i);
                    floats.push(*i as f64);
                }
                Some(ScalarValue::Float(f)) => {
                    all_int = false;
                    floats.push(*f);
                }
                _ => {
                    all_int = false;
                    floats.push(f64::NAN);
                }
            }
        }
        if all_int {
            frame.insert_column(format!("params.{pk}"), Column::Int(ints))?;
        } else {
            frame.insert_column(format!("params.{pk}"), Column::Float(floats))?;
        }
    }

    // Standard scalar metrics
    for metric in ["wall_s", "user_cpu_s", "sys_cpu_s", "instructions", "cycles", "cache-misses"] {
        let vals: Vec<f64> = records
            .iter()
            .map(|r| extract_record_metric(r, metric).unwrap_or(f64::NAN))
            .collect();
        if vals.iter().any(|v| v.is_finite()) {
            frame.insert_column(metric, Column::Float(vals))?;
        }
    }

    Ok(frame)
}

/// Runs services in dependency order over `records`, honoring `requires` tiers,
/// and writes `calo.table/1` sidecars under `<results_root>/analysis/<service>/` (spec §12.1, §12.2).
pub fn run_analysis_pipeline(
    results_root: &Path,
    suite: &Suite,
    records: &[Record],
    symbolizer: Option<&dyn Symbolizer>,
) -> Result<AnalysisReport, ServiceError> {
    let mut notices = Vec::new();
    let mut written_tables = Vec::new();

    if symbolizer.is_none() {
        notices.push("stage 2 (symbolization) skipped: no Symbolizer registered".to_string());
    }

    let mut reg = ServiceRegistry::new();
    reg.apply_layer(ServiceLayer::Suite, &suite.services)?;
    let order = reg.execution_order()?;

    let valid_recs: Vec<&Record> = records
        .iter()
        .filter(|r| r.identity.warmup != Some(true) && r.identity.status == Status::Ok)
        .collect();

    if valid_recs.is_empty() {
        return Ok(AnalysisReport {
            notices,
            written_tables,
        });
    }

    for svc_name in order {
        let Some(svc) = reg.get(svc_name) else {
            continue;
        };

        // Filter records satisfying `svc.requires()`
        let eligible: Vec<&Record> = valid_recs
            .iter()
            .copied()
            .filter(|r| {
                svc.requires()
                    .iter()
                    .all(|req| r.provenance.tiers_active.satisfies(*req))
            })
            .collect();

        if eligible.is_empty() {
            notices.push(format!(
                "skipped service `{svc_name}`: no records satisfy required tiers {:?}",
                svc.requires()
            ));
            continue;
        }

        let frame = build_frame_from_records(&eligible)?;
        let opts = reg.options_for(svc_name);

        let out_frame = match svc.run(&frame, &opts) {
            Ok(f) => f,
            Err(ServiceError::MissingColumn { .. })
            | Err(ServiceError::NotYetImplemented { .. })
            | Err(ServiceError::PythonUnsupported { .. }) => {
                notices.push(format!("skipped service `{svc_name}` (inputs unavailable or deferred)"));
                continue;
            }
            Err(e) => return Err(e),
        };

        let svc_dir = results_root.join("analysis").join(svc_name);
        let out_path = svc_dir.join("summary.calotbl");
        let opts_hash = hash_str(&format!("{:?}", opts));

        let mut meta = BTreeMap::new();
        meta.insert("options_hash".to_string(), ScalarValue::Str(opts_hash));
        meta.insert(
            "bootstrap_seed".to_string(),
            ScalarValue::Int(opts.ci.seed as i64),
        );

        if let Ok(bytes) = encode_frame_as_calotbl(&format!("analysis.{svc_name}"), &out_frame, meta)
        {
            if write_atomic_bytes(&out_path, &bytes).is_ok() {
                written_tables.push(out_path);
            }
        }
    }

    Ok(AnalysisReport {
        notices,
        written_tables,
    })
}

/// Provenance summary attached to every generated report (Task 20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceFooter {
    /// Suite SHA-256 hash.
    pub suite_hash: String,
    /// Distinct `probe` versions observed across records.
    pub probe_versions: BTreeSet<String>,
    /// Distinct machine fingerprints observed across records.
    pub machine_fingerprints: BTreeSet<String>,
    /// Aggregated warning counts keyed by warning code string.
    pub warning_counts: BTreeMap<String, usize>,
    /// Run IDs excluded due to failure, warmup, or tier mismatch.
    pub excluded_runs: Vec<String>,
}

/// Generated outputs for a single `ReportDef` (spec §13).
#[derive(Debug, Clone)]
pub struct GeneratedReport {
    /// Report name.
    pub name: String,
    /// Gate evaluation outcomes.
    pub gates: Vec<GateOutcome>,
    /// Files written under `reports/`.
    pub files: Vec<PathBuf>,
    /// Non-fatal notices (e.g. unmet report tier requirements).
    pub notices: Vec<String>,
}

/// Renders tables, plots, gates, and `md`/`html`/`csv`/`svg` files for `report_def` (spec §13).
#[allow(clippy::too_many_arguments)]
pub fn generate_report_artifacts(
    results_root: &Path,
    suite_root: &Path,
    suite: &Suite,
    suite_hash: &str,
    report_def: &ReportDef,
    records: &[Record],
    baseline_records: Option<&[Record]>,
) -> Result<GeneratedReport, ReportError> {
    let selector = report_def
        .select
        .as_ref()
        .map_or(Selector::All, Selector::from);
    let selected = select_records(records, &selector);

    let mut notices = Vec::new();
    for req_tier in &report_def.requires {
        let missing_count = selected
            .iter()
            .filter(|r| !r.provenance.tiers_active.satisfies(*req_tier))
            .count();
        if missing_count > 0 {
            notices.push(format!(
                "report `{}` requires tier `{req_tier}`, unmet on {missing_count} selected record(s)",
                report_def.name
            ));
        }
    }

    let reports_dir = results_root.join("reports").join(&report_def.name);
    let _ = fs::create_dir_all(&reports_dir);

    let mut files = Vec::new();
    let mut csv_outputs = Vec::new();
    let mut md_tables = Vec::new();

    for (t_idx, tbl_spec) in report_def.tables.iter().enumerate() {
        let pivot = build_pivot_table(&selected, tbl_spec);
        let csv_str = pivot.to_csv();
        let md_str = pivot.to_markdown();
        let csv_path = reports_dir.join(format!("table_{t_idx}.csv"));
        if report_def.output.contains(&ReportOutputFormat::Csv) {
            let _ = write_atomic_bytes(&csv_path, csv_str.as_bytes());
            files.push(csv_path);
        }
        csv_outputs.push(csv_str);
        md_tables.push((tbl_spec.value.clone(), md_str));
    }

    let mut svgs = Vec::new();
    if !report_def.plots.is_empty() {
        if let Ok(frame) = build_frame_from_records(&selected) {
            for (p_idx, p_def) in report_def.plots.iter().enumerate() {
                let p_spec = PlotSpec {
                    plot_type: p_def.plot_type.clone(),
                    x: p_def.x.clone(),
                    y: p_def.y.clone(),
                    line_by: p_def.line_by.clone(),
                    facet_by: p_def.facet_by.clone(),
                    x_scale: ScaleKind::from_str_opt(
                        p_def.scale.as_ref().and_then(|s| s.x.as_deref()),
                    ),
                    y_scale: ScaleKind::from_str_opt(
                        p_def.scale.as_ref().and_then(|s| s.y.as_deref()),
                    ),
                    errorbars: p_def.errorbars.clone(),
                    title: p_def.title.clone(),
                    color_scale: ScaleKind::Linear,
                };
                if let Ok(svg_str) = render_plot(&p_spec, &frame) {
                    if report_def.output.contains(&ReportOutputFormat::Svg) {
                        let svg_path = reports_dir.join(format!("plot_{p_idx}.svg"));
                        let _ = write_atomic_bytes(&svg_path, svg_str.as_bytes());
                        files.push(svg_path);
                    }
                    svgs.push((p_def.title.clone().unwrap_or_else(|| p_def.plot_type.clone()), svg_str));
                }
            }
        }
    }

    let gates = evaluate_report_gates(&selector, &report_def.gates, records, baseline_records)?;

    let footer = build_provenance_footer(suite_hash, records, &report_def.requires);

    // Load `context:` files verbatim (R-REP-5)
    let mut context_blocks = Vec::new();
    for ctx_rel in &suite.context {
        let p = suite_root.join(ctx_rel);
        if let Ok(body) = fs::read_to_string(&p) {
            context_blocks.push((ctx_rel.clone(), body));
        }
    }

    if report_def.output.is_empty() || report_def.output.contains(&ReportOutputFormat::Md) {
        let md = render_markdown_report(&report_def.name, &md_tables, &gates, &context_blocks, &footer);
        let md_path = reports_dir.join("report.md");
        let _ = write_atomic_bytes(&md_path, md.as_bytes());
        files.push(md_path);
    }

    if report_def.output.contains(&ReportOutputFormat::Html) {
        let html = render_html_report(&report_def.name, &md_tables, &svgs, &gates, &context_blocks, &footer);
        let html_path = reports_dir.join("report.html");
        let _ = write_atomic_bytes(&html_path, html.as_bytes());
        files.push(html_path);
    }

    Ok(GeneratedReport {
        name: report_def.name.clone(),
        gates,
        files,
        notices,
    })
}

fn build_provenance_footer(
    suite_hash: &str,
    records: &[Record],
    requires: &[calo_schema::Tier],
) -> ProvenanceFooter {
    let mut probe_versions = BTreeSet::new();
    let mut machine_fingerprints = BTreeSet::new();
    let mut warning_counts = BTreeMap::new();
    let mut excluded_runs = Vec::new();

    for r in records {
        probe_versions.insert(r.provenance.probe_version.clone());
        if let Some(fp) = &r.environment.fingerprint {
            machine_fingerprints.insert(fp.clone());
        }
        for w in &r.provenance.warnings {
            let code_str = format!("{:?}", w.code);
            *warning_counts.entry(code_str).or_insert(0) += 1;
        }
        let tier_unmet = requires
            .iter()
            .any(|t| !r.provenance.tiers_active.satisfies(*t));
        if r.identity.status != Status::Ok || r.identity.warmup == Some(true) || tier_unmet {
            excluded_runs.push(r.identity.run_id.clone());
        }
    }

    ProvenanceFooter {
        suite_hash: suite_hash.to_string(),
        probe_versions,
        machine_fingerprints,
        warning_counts,
        excluded_runs,
    }
}

fn render_markdown_report(
    name: &str,
    tables: &[(String, String)],
    gates: &[GateOutcome],
    contexts: &[(String, String)],
    footer: &ProvenanceFooter,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Report: {name}\n");

    for (label, md_tbl) in tables {
        let _ = writeln!(out, "## Table: {label}\n\n{md_tbl}");
    }

    if !gates.is_empty() {
        let _ = writeln!(
            out,
            "## Gates\n\n| Metric | Observed | Baseline | Threshold | Passed |\n| --- | ---: | ---: | --- | --- |"
        );
        for g in gates {
            let _ = writeln!(
                out,
                "| {} | {:.4} | {:.4} | {} | {} |",
                g.metric, g.observed, g.baseline, g.threshold, g.passed
            );
        }
        out.push('\n');
    }

    for (ctx_name, body) in contexts {
        let _ = writeln!(out, "## Context: {ctx_name}\n\n{body}\n");
    }

    let _ = writeln!(out, "## Provenance\n");
    let _ = writeln!(out, "- **Suite Hash:** `{}`", footer.suite_hash);
    let _ = writeln!(
        out,
        "- **Probe Versions:** {:?}",
        footer.probe_versions
    );
    let _ = writeln!(
        out,
        "- **Machine Fingerprints:** {:?}",
        footer.machine_fingerprints
    );
    let _ = writeln!(out, "- **Warnings:** {:?}", footer.warning_counts);
    let _ = writeln!(out, "- **Excluded Runs:** {:?}", footer.excluded_runs);
    out
}

fn render_html_report(
    name: &str,
    tables: &[(String, String)],
    svgs: &[(String, String)],
    gates: &[GateOutcome],
    contexts: &[(String, String)],
    footer: &ProvenanceFooter,
) -> String {
    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\"/>\n");
    let _ = writeln!(html, "<title>Calorimeter Report: {}</title>", xml_escape(name));
    html.push_str("<style>\nbody { font-family: 'IBM Plex Sans', sans-serif; color: #161616; max-width: 1100px; margin: 2rem auto; padding: 0 1rem; }\ntable { border-collapse: collapse; margin: 1rem 0; width: 100%; }\nth, td { border: 1px solid #c6c6c6; padding: 0.4rem 0.6rem; text-align: left; }\nth { background: #e0e0e0; }\n.pass { color: #161616; font-weight: 600; }\n.fail { color: #7017FF; font-weight: 700; }\npre { background: #f4f4f4; padding: 0.8rem; border: 1px solid #e0e0e0; overflow-x: auto; }\n</style>\n</head>\n<body>\n");
    let _ = writeln!(html, "<h1>Report: {}</h1>", xml_escape(name));

    for (title, svg_body) in svgs {
        let _ = writeln!(html, "<h2>{}</h2>\n<div>{svg_body}</div>", xml_escape(title));
    }

    for (label, md_tbl) in tables {
        let _ = writeln!(
            html,
            "<h2>Table: {}</h2>\n<pre>{}</pre>",
            xml_escape(label),
            xml_escape(md_tbl)
        );
    }

    if !gates.is_empty() {
        html.push_str("<h2>Gates</h2>\n<table><thead><tr><th>Metric</th><th>Observed</th><th>Baseline</th><th>Threshold</th><th>Status</th></tr></thead><tbody>\n");
        for g in gates {
            let cls = if g.passed { "pass" } else { "fail" };
            let lbl = if g.passed { "PASS" } else { "FAIL" };
            let _ = writeln!(
                html,
                "<tr><td>{}</td><td>{:.4}</td><td>{:.4}</td><td>{}</td><td class=\"{cls}\">{lbl}</td></tr>",
                xml_escape(&g.metric),
                g.observed,
                g.baseline,
                xml_escape(&g.threshold)
            );
        }
        html.push_str("</tbody></table>\n");
    }

    for (ctx_name, body) in contexts {
        let _ = writeln!(
            html,
            "<h2>Context: {}</h2>\n<pre>{}</pre>",
            xml_escape(ctx_name),
            xml_escape(body)
        );
    }

    html.push_str("<footer><h2>Provenance</h2><ul>\n");
    let _ = writeln!(html, "<li>Suite Hash: <code>{}</code></li>", xml_escape(&footer.suite_hash));
    let _ = writeln!(html, "<li>Probe Versions: {:?}</li>", footer.probe_versions);
    let _ = writeln!(html, "<li>Machine Fingerprints: {:?}</li>", footer.machine_fingerprints);
    let _ = writeln!(html, "<li>Warnings: {:?}</li>", footer.warning_counts);
    let _ = writeln!(html, "<li>Excluded Runs: {:?}</li>", footer.excluded_runs);
    html.push_str("</ul></footer>\n</body>\n</html>\n");
    html
}

/// A single row in `calo diff <dirA> <dirB>` output (spec §14, §18).
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    /// Benchmark and parameter key.
    pub key: String,
    /// Metric name.
    pub metric: String,
    /// Median in directory A.
    pub a_median: f64,
    /// Median in directory B.
    pub b_median: f64,
    /// Relative percentage change `(B - A) / A * 100`.
    pub rel_change_pct: f64,
    /// Cliff's delta effect size.
    pub effect_size: f64,
    /// Mann-Whitney U p-value.
    pub p_value: f64,
    /// `true` if `p_value < 0.05`.
    pub significant: bool,
}

/// Compares matching `(benchmark, params)` pairs between two results directories using `compare` (spec §14, §18).
///
/// Refuses to mix records from incompatible `probe` major versions unless `allow_mixed` is `true` (spec §18).
pub fn diff_results_dirs(
    dir_a: &Path,
    dir_b: &Path,
    metric: &str,
    allow_mixed: bool,
) -> Result<String, ReportError> {
    let recs_a = load_records_from_dir(dir_a);
    let recs_b = load_records_from_dir(dir_b);

    if !allow_mixed {
        let majors_a: BTreeSet<u64> = recs_a
            .iter()
            .filter_map(|r| parse_major(&r.provenance.probe_version))
            .collect();
        let majors_b: BTreeSet<u64> = recs_b
            .iter()
            .filter_map(|r| parse_major(&r.provenance.probe_version))
            .collect();
        if !majors_a.is_empty() && !majors_b.is_empty() && majors_a != majors_b {
            return Err(ReportError::InvalidFailIf {
                raw: "probe_version".to_string(),
                reason: format!(
                    "refusing to mix incompatible probe major versions ({majors_a:?} vs {majors_b:?}); pass --allow-mixed to override"
                ),
            });
        }
    }

    let group_fn = |recs: &[Record]| -> BTreeMap<String, Vec<f64>> {
        let mut map: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in recs {
            if r.identity.status != Status::Ok || r.identity.warmup == Some(true) {
                continue;
            }
            if let Some(v) = extract_record_metric(r, metric) {
                let p_str: Vec<String> = r
                    .identity
                    .params
                    .iter()
                    .map(|(k, val)| format!("{k}={val}"))
                    .collect();
                let key = format!("{}({})", r.identity.benchmark, p_str.join(","));
                map.entry(key).or_default().push(v);
            }
        }
        map
    };

    let map_a = group_fn(&recs_a);
    let map_b = group_fn(&recs_b);
    let reg = ServiceRegistry::new();

    let mut rows = Vec::new();
    for (key, vals_a) in &map_a {
        let Some(vals_b) = map_b.get(key) else {
            continue;
        };
        let mut f_cmp = Frame::new();
        let max_len = vals_a.len().max(vals_b.len());
        let mut a_col = vals_a.clone();
        let mut b_col = vals_b.clone();
        while a_col.len() < max_len {
            a_col.push(vals_a[a_col.len() % vals_a.len()]);
        }
        while b_col.len() < max_len {
            b_col.push(vals_b[b_col.len() % vals_b.len()]);
        }
        let _ = f_cmp.insert_column("baseline", Column::Float(vals_a.clone()));
        let mut f_b = Frame::new();
        let _ = f_b.insert_column("observed", Column::Float(vals_b.clone()));
        // Build combined frame for `compare`
        let mut cmp_frame = Frame::new();
        let _ = cmp_frame.insert_column("baseline", Column::Float(a_col));
        let _ = cmp_frame.insert_column("observed", Column::Float(b_col));

        if let Ok(res) = reg.run_service("compare", &cmp_frame) {
            let delta = res
                .column("cliffs_delta")
                .and_then(|c| c.to_f64_vec("cliffs_delta").ok())
                .map(|v| v[0])
                .unwrap_or(0.0);
            let p_val = res
                .column("p_value")
                .and_then(|c| c.to_f64_vec("p_value").ok())
                .map(|v| v[0])
                .unwrap_or(1.0);
            let a_med = median_slice(vals_a);
            let b_med = median_slice(vals_b);
            let rel_pct = if a_med != 0.0 {
                ((b_med - a_med) / a_med.abs()) * 100.0
            } else {
                0.0
            };
            rows.push(DiffRow {
                key: key.clone(),
                metric: metric.to_string(),
                a_median: a_med,
                b_median: b_med,
                rel_change_pct: rel_pct,
                effect_size: delta,
                p_value: p_val,
                significant: p_val < 0.05,
            });
        }
    }

    let mut md = String::new();
    let _ = writeln!(
        md,
        "| Target | Metric | A Median | B Median | Rel Change | Effect Size | p-value | Sig |\n| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |"
    );
    for r in rows {
        let sig_mark = if r.significant { "*" } else { "" };
        let _ = writeln!(
            md,
            "| {} | {} | {:.4} | {:.4} | {:+.2}% | {:.3} | {:.4} | {} |",
            r.key,
            r.metric,
            r.a_median,
            r.b_median,
            r.rel_change_pct,
            r.effect_size,
            r.p_value,
            sig_mark
        );
    }
    Ok(md)
}

fn load_records_from_dir(dir: &Path) -> Vec<Record> {
    let mut out = Vec::new();
    let rec_root = if dir.join("records").is_dir() {
        dir.join("records")
    } else {
        dir.to_path_buf()
    };
    let Ok(entries) = fs::read_dir(&rec_root) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let run_yaml = p.join("run.yaml");
            let target = if run_yaml.exists() {
                run_yaml
            } else {
                p.join("rank0.yaml")
            };
            if let Ok(raw) = fs::read_to_string(&target) {
                if let Ok(rec) = from_yaml_str::<Record>(&raw) {
                    out.push(rec);
                }
            }
        }
    }
    out
}

fn parse_major(ver: &str) -> Option<u64> {
    ver.split('.').next()?.parse().ok()
}

fn median_slice(vals: &[f64]) -> f64 {
    let mut s = vals.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = s.len() / 2;
    if s.len() % 2 == 1 {
        s[m]
    } else {
        0.5 * (s[m - 1] + s[m])
    }
}

fn hash_str(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

fn encode_frame_as_calotbl(
    table_name: &str,
    frame: &Frame,
    meta: BTreeMap<String, ScalarValue>,
) -> Result<Vec<u8>, ReportError> {
    let mut columns = Vec::new();
    let mut strings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut payload = Vec::new();
    payload.extend_from_slice(&(frame.num_rows() as u32).to_le_bytes());

    for (name, col) in frame.columns() {
        match col {
            Column::Int(vs) => {
                columns.push(ColumnDef {
                    name: name.clone(),
                    col_type: ColumnType::Int64,
                });
                for v in vs {
                    payload.extend_from_slice(&v.to_le_bytes());
                }
            }
            Column::Float(vs) => {
                columns.push(ColumnDef {
                    name: name.clone(),
                    col_type: ColumnType::Float64,
                });
                for v in vs {
                    payload.extend_from_slice(&v.to_le_bytes());
                }
            }
            Column::Bool(vs) => {
                columns.push(ColumnDef {
                    name: name.clone(),
                    col_type: ColumnType::Uint8,
                });
                for v in vs {
                    payload.push(u8::from(*v));
                }
            }
            Column::Str(vs) => {
                columns.push(ColumnDef {
                    name: name.clone(),
                    col_type: ColumnType::StringId,
                });
                let mut dict = Vec::new();
                for s in vs {
                    let id = if let Some(pos) = dict.iter().position(|x| x == s) {
                        pos as u32
                    } else {
                        dict.push(s.clone());
                        (dict.len() - 1) as u32
                    };
                    payload.extend_from_slice(&id.to_le_bytes());
                }
                strings.insert(name.clone(), dict);
            }
        }
    }

    let header = TableHeader {
        schema: TableHeader::SCHEMA.to_string(),
        table: table_name.to_string(),
        columns,
        strings,
        meta,
    };
    let hdr_yaml = to_yaml_string(&header).map_err(|e| ReportError::InvalidFailIf {
        raw: table_name.to_string(),
        reason: e.to_string(),
    })?;

    let mut buf = Vec::new();
    buf.extend_from_slice(crate::table::TABLE_MAGIC);
    buf.extend_from_slice(&(hdr_yaml.len() as u32).to_le_bytes());
    buf.extend_from_slice(hdr_yaml.as_bytes());
    let crc = crc32_ieee(&payload);
    buf.extend_from_slice(&payload);
    buf.extend_from_slice(&crc.to_le_bytes());
    Ok(buf)
}
