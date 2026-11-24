//! Record selection, grouping, metric aggregation, and CSV/Markdown table formatting (spec §13).

use crate::plan::{matches, RunDescriptor, Selector};
use calo_schema::common::{ScalarValue, Status};
use calo_schema::record::{Record, RegionNode};
use calo_schema::suite::ReportTable;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// A 2D pivot table generated from a `ReportTable` specification (`rows`, `cols`, `value`, spec §13).
#[derive(Debug, Clone, PartialEq)]
pub struct PivotTable {
    /// Row grouping key name (e.g. `"params.patch"`).
    pub row_header: String,
    /// Column grouping key name (e.g. `"params.N"`).
    pub col_header: String,
    /// Aggregated value metric name (e.g. `"speedup"` or `"wall_s.median"`).
    pub value_name: String,
    /// Ordered row key labels.
    pub row_keys: Vec<String>,
    /// Ordered column key labels.
    pub col_keys: Vec<String>,
    /// Cell values indexed by `(row_idx, col_idx)`. Missing combinations are `None`.
    pub cells: Vec<Vec<Option<f64>>>,
}

impl PivotTable {
    /// Returns the cell value for `(row_key, col_key)` if present.
    pub fn get(&self, row_key: &str, col_key: &str) -> Option<f64> {
        let r_idx = self.row_keys.iter().position(|r| r == row_key)?;
        let c_idx = self.col_keys.iter().position(|c| c == col_key)?;
        self.cells.get(r_idx)?.get(c_idx).copied().flatten()
    }

    /// Renders the pivot table as RFC 4180 CSV (spec §13 R-REP-4).
    /// Missing cells are rendered as empty strings `""`.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        let mut header_fields = Vec::with_capacity(self.col_keys.len() + 1);
        header_fields.push(escape_csv(&format!(
            "{} \\ {}",
            self.row_header, self.col_header
        )));
        for col in &self.col_keys {
            header_fields.push(escape_csv(col));
        }
        let _ = writeln!(out, "{}", header_fields.join(","));

        for (r_idx, row_key) in self.row_keys.iter().enumerate() {
            let mut row_fields = Vec::with_capacity(self.col_keys.len() + 1);
            row_fields.push(escape_csv(row_key));
            for item in self.cells[r_idx].iter().take(self.col_keys.len()) {
                match item {
                    Some(v) => row_fields.push(format_metric_number(*v)),
                    None => row_fields.push(String::new()),
                }
            }
            let _ = writeln!(out, "{}", row_fields.join(","));
        }

        out
    }

    /// Renders the pivot table as a Github-flavored Markdown table (spec §13 R-REP-4).
    /// Missing cells are rendered as `"-"`.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let corner = format!("{} \\ {}", self.row_header, self.col_header);

        let _ = write!(out, "| {corner} |");
        for col in &self.col_keys {
            let _ = write!(out, " {col} |");
        }
        let _ = writeln!(out);

        let _ = write!(out, "| --- |");
        for _ in &self.col_keys {
            let _ = write!(out, " ---: |");
        }
        let _ = writeln!(out);

        for (r_idx, row_key) in self.row_keys.iter().enumerate() {
            let _ = write!(out, "| {row_key} |");
            for item in self.cells[r_idx].iter().take(self.col_keys.len()) {
                match item {
                    Some(v) => {
                        let formatted = format_metric_number(*v);
                        let _ = write!(out, " {formatted} |");
                    }
                    None => {
                        let _ = write!(out, " - |");
                    }
                }
            }
            let _ = writeln!(out);
        }

        out
    }
}

/// Filters `records` by `selector`, excluding warmup and failed runs by default (spec §8.4 R-STP-7, §8.5 R-BEN-1).
pub fn select_records<'a>(records: &'a [Record], selector: &Selector) -> Vec<&'a Record> {
    records
        .iter()
        .filter(|rec| {
            if rec.identity.warmup == Some(true) || rec.identity.status != Status::Ok {
                return false;
            }
            let desc = RunDescriptor {
                benchmark: &rec.identity.benchmark,
                group: rec.identity.group.as_deref(),
                mode: rec.identity.mode,
                params: &rec.identity.params,
                tiers: &rec.provenance.tiers_active,
                machine: None,
                node: None,
            };
            matches(selector, &desc)
        })
        .collect()
}

/// Builds a 2D `PivotTable` from `records` according to `spec` (`rows`, `cols`, `value`, spec §13).
///
/// If `spec.value` is `"speedup"` and not directly stored in the record, speedup is computed relative
/// to the first column in each row (`T(col_0) / T(col_j)`).
pub fn build_pivot_table(records: &[&Record], spec: &ReportTable) -> PivotTable {
    let (base_metric, agg_kind) = split_metric_estimator(&spec.value);
    let compute_speedup_from_wall = base_metric == "speedup"
        && records
            .iter()
            .all(|r| extract_record_metric(r, "speedup").is_none());
    let lookup_metric = if compute_speedup_from_wall {
        "wall_s"
    } else {
        base_metric
    };

    // Preserve natural scalar ordering for row and column keys
    let mut row_set: BTreeSet<OrderedKey> = BTreeSet::new();
    let mut col_set: BTreeSet<OrderedKey> = BTreeSet::new();
    let mut grouped: BTreeMap<(OrderedKey, OrderedKey), Vec<f64>> = BTreeMap::new();

    for rec in records {
        let Some(r_key) = extract_grouping_key(rec, &spec.rows) else {
            continue;
        };
        let Some(c_key) = extract_grouping_key(rec, &spec.cols) else {
            continue;
        };
        let Some(val) = extract_record_metric(rec, lookup_metric) else {
            continue;
        };

        row_set.insert(r_key.clone());
        col_set.insert(c_key.clone());
        grouped.entry((r_key, c_key)).or_default().push(val);
    }

    let row_ordered: Vec<OrderedKey> = row_set.into_iter().collect();
    let col_ordered: Vec<OrderedKey> = col_set.into_iter().collect();

    let mut cells = vec![vec![None; col_ordered.len()]; row_ordered.len()];

    for (r_idx, r_key) in row_ordered.iter().enumerate() {
        for (c_idx, c_key) in col_ordered.iter().enumerate() {
            if let Some(vals) = grouped.get(&(r_key.clone(), c_key.clone())) {
                if !vals.is_empty() {
                    cells[r_idx][c_idx] = Some(aggregate_values(vals, agg_kind));
                }
            }
        }

        if compute_speedup_from_wall {
            // Find the baseline time in the first populated column of this row
            if let Some(t_base) = cells[r_idx].iter().copied().flatten().next() {
                if t_base > 0.0 {
                    for cell in &mut cells[r_idx] {
                        if let Some(t) = *cell {
                            if t > 0.0 {
                                *cell = Some(t_base / t);
                            }
                        }
                    }
                }
            }
        }
    }

    PivotTable {
        row_header: spec.rows.clone(),
        col_header: spec.cols.clone(),
        value_name: spec.value.clone(),
        row_keys: row_ordered.into_iter().map(|k| k.display).collect(),
        col_keys: col_ordered.into_iter().map(|k| k.display).collect(),
        cells,
    }
}

/// Aggregates a slice of metric values across records using the metric's estimator suffix.
pub(crate) fn aggregate_metric_across_records(
    records: &[&Record],
    metric_spec: &str,
) -> Option<f64> {
    let (base_metric, agg_kind) = split_metric_estimator(metric_spec);
    let vals: Vec<f64> = records
        .iter()
        .filter_map(|r| extract_record_metric(r, base_metric))
        .collect();
    if vals.is_empty() {
        None
    } else {
        Some(aggregate_values(&vals, agg_kind))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AggKind {
    Median,
    Mean,
    Min,
    Max,
}

pub(crate) fn split_metric_estimator(spec: &str) -> (&str, AggKind) {
    if let Some(base) = spec.strip_suffix(".median") {
        (base, AggKind::Median)
    } else if let Some(base) = spec.strip_suffix(".mean") {
        (base, AggKind::Mean)
    } else if let Some(base) = spec.strip_suffix(".min") {
        (base, AggKind::Min)
    } else if let Some(base) = spec.strip_suffix(".max") {
        (base, AggKind::Max)
    } else {
        (spec, AggKind::Median)
    }
}

pub(crate) fn aggregate_values(vals: &[f64], kind: AggKind) -> f64 {
    match kind {
        AggKind::Mean => vals.iter().sum::<f64>() / (vals.len() as f64),
        AggKind::Min => vals.iter().copied().fold(f64::INFINITY, f64::min),
        AggKind::Max => vals.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        AggKind::Median => {
            let mut sorted = vals.to_vec();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = sorted.len() / 2;
            if sorted.len() % 2 == 1 {
                sorted[mid]
            } else {
                0.5 * (sorted[mid - 1] + sorted[mid])
            }
        }
    }
}

/// Extracts a numeric metric value from a single `Record` (spec §10, §13).
pub fn extract_record_metric(rec: &Record, metric: &str) -> Option<f64> {
    // 1. Check aggregate scalars first if present (`run.yaml`)
    if let Some(agg) = &rec.aggregate {
        if let Some(stat) = agg.scalars.get(metric) {
            return Some(stat.mean);
        }
    }

    // 2. Check `observations.scalars`
    if let Some(s) = &rec.observations.scalars {
        match metric {
            "wall_s" => return s.wall_s,
            "user_cpu_s" => return s.user_cpu_s,
            "sys_cpu_s" => return s.sys_cpu_s,
            "peak_rss_bytes" => return s.peak_rss_bytes.map(|v| v as f64),
            "minor_faults" => return s.minor_faults.map(|v| v as f64),
            "major_faults" => return s.major_faults.map(|v| v as f64),
            "voluntary_ctx" => return s.voluntary_ctx.map(|v| v as f64),
            "involuntary_ctx" => return s.involuntary_ctx.map(|v| v as f64),
            "block_in_bytes" => return s.block_in_bytes.map(|v| v as f64),
            "block_out_bytes" => return s.block_out_bytes.map(|v| v as f64),
            "energy_j.pkg" => return s.energy_j.as_ref().and_then(|e| e.pkg),
            "energy_j.dram" => return s.energy_j.as_ref().and_then(|e| e.dram),
            "energy_j.gpu" => return s.energy_j.as_ref().and_then(|e| e.gpu),
            "ipc" => {
                let instr = s.counters.get("instructions")?.value as f64;
                let cyc = s.counters.get("cycles")?.value as f64;
                if cyc > 0.0 {
                    return Some(instr / cyc);
                }
            }
            "cache_miss_rate" => {
                let misses = s.counters.get("cache-misses")?.value as f64;
                let instr = s.counters.get("instructions")?.value as f64;
                if instr > 0.0 {
                    return Some(misses / instr);
                }
            }
            _ => {
                if let Some(c) = s.counters.get(metric) {
                    return Some(c.value as f64);
                }
                if let Some(c_alias) = metric.strip_prefix("counters.") {
                    if let Some(c) = s.counters.get(c_alias) {
                        return Some(c.value as f64);
                    }
                }
            }
        }
    }

    // 3. Check `observations.reported` (both `reported.<k>` and bare `<k>`)
    if let Some(rep) = &rec.observations.reported {
        let key = metric.strip_prefix("reported.").unwrap_or(metric);
        if let Some(m) = rep.get(key) {
            return Some(m.value);
        }
    }

    // 4. Check `observations.phases.<name>` -> duration in seconds
    if let Some(phase_name) = metric.strip_prefix("phase.") {
        let span = rec.observations.phases.as_ref()?.get(phase_name)?;
        return Some(((span.end_ns - span.start_ns) as f64) * 1e-9);
    }

    // 5. Check `observations.regions` (`region.<name>.incl_s` or `excl_s`)
    if let Some(rest) = metric.strip_prefix("region.") {
        let regions = rec.observations.regions.as_ref()?;
        if let Some(reg_name) = rest.strip_suffix(".incl_s") {
            return find_region_metric(&regions.threads, reg_name, true);
        }
        if let Some(reg_name) = rest.strip_suffix(".excl_s") {
            return find_region_metric(&regions.threads, reg_name, false);
        }
    }

    // 6. Check numeric parameter in `identity.params`
    let param_key = metric.strip_prefix("params.").unwrap_or(metric);
    match rec.identity.params.get(param_key)? {
        ScalarValue::Int(i) => Some(*i as f64),
        ScalarValue::Float(f) => Some(*f),
        _ => None,
    }
}

fn find_region_metric(
    threads: &[calo_schema::record::ThreadRegions],
    target_name: &str,
    inclusive: bool,
) -> Option<f64> {
    fn search_nodes(nodes: &[RegionNode], target: &str, inclusive: bool) -> Option<f64> {
        for n in nodes {
            if n.name == target {
                return Some(if inclusive { n.incl_s } else { n.excl_s });
            }
            if let Some(found) = search_nodes(&n.children, target, inclusive) {
                return Some(found);
            }
        }
        None
    }

    for t in threads {
        if let Some(v) = search_nodes(&t.tree, target_name, inclusive) {
            return Some(v);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderedKey {
    int_sort: Option<i64>,
    display: String,
}

impl PartialOrd for OrderedKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self.int_sort, other.int_sort) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => self.display.cmp(&other.display),
        }
    }
}

fn extract_grouping_key(rec: &Record, key_spec: &str) -> Option<OrderedKey> {
    match key_spec {
        "benchmark" => Some(OrderedKey {
            int_sort: None,
            display: rec.identity.benchmark.clone(),
        }),
        "group" => rec.identity.group.as_ref().map(|g| OrderedKey {
            int_sort: None,
            display: g.clone(),
        }),
        "machine" => Some(OrderedKey {
            int_sort: None,
            display: rec.environment.machine.clone(),
        }),
        "node_class" => rec.environment.node_class.as_ref().map(|nc| OrderedKey {
            int_sort: None,
            display: nc.clone(),
        }),
        "rank" => rec.identity.rank.map(|r| OrderedKey {
            int_sort: Some(i64::from(r)),
            display: r.to_string(),
        }),
        "ranks" => rec.identity.ranks.map(|r| OrderedKey {
            int_sort: Some(i64::from(r)),
            display: r.to_string(),
        }),
        _ => {
            let param_name = key_spec.strip_prefix("params.").unwrap_or(key_spec);
            let val = rec.identity.params.get(param_name)?;
            let int_sort = match val {
                ScalarValue::Int(i) => Some(*i),
                _ => None,
            };
            Some(OrderedKey {
                int_sort,
                display: val.to_string(),
            })
        }
    }
}

fn escape_csv(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        let escaped = field.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        field.to_string()
    }
}

fn format_metric_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 {
        format!("{:.2}", v)
    } else {
        let s = format!("{:.4}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
