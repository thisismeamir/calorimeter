//! Regression gate evaluation (`fail_if: "> +5%"`, `"< -10%"`, or expression, spec §13 R-REP-4).

use crate::expr::{eval, parse, MapEnv, Value};
use crate::plan::Selector;
use crate::report::error::ReportError;
use crate::report::pivot::{aggregate_metric_across_records, select_records};
use calo_schema::record::Record;
use calo_schema::suite::ReportGate;
use serde::{Deserialize, Serialize};

/// Result of evaluating a single report regression gate (spec §13 R-REP-4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateOutcome {
    /// `true` if the gate passed (`fail_if` condition evaluated to `false`).
    pub passed: bool,
    /// Metric evaluated (e.g. `"wall_s.median"`).
    pub metric: String,
    /// Aggregated value observed in the target record set.
    pub observed: f64,
    /// Aggregated value observed in the baseline comparison record set.
    pub baseline: f64,
    /// Original `fail_if` threshold specification string.
    pub threshold: String,
}

/// Evaluates a single `ReportGate` comparing `observed_records` against `baseline_records` (spec §13 R-REP-4).
pub fn evaluate_gate(
    gate: &ReportGate,
    observed_records: &[&Record],
    baseline_records: &[&Record],
) -> Result<GateOutcome, ReportError> {
    let observed =
        aggregate_metric_across_records(observed_records, &gate.metric).ok_or_else(|| {
            ReportError::EmptyMetricSet {
                metric: gate.metric.clone(),
                context: "observed",
            }
        })?;

    let baseline =
        aggregate_metric_across_records(baseline_records, &gate.metric).ok_or_else(|| {
            ReportError::EmptyMetricSet {
                metric: gate.metric.clone(),
                context: "baseline",
            }
        })?;

    let failed = check_fail_if(&gate.fail_if, &gate.metric, observed, baseline)?;

    Ok(GateOutcome {
        passed: !failed,
        metric: gate.metric.clone(),
        observed,
        baseline,
        threshold: gate.fail_if.clone(),
    })
}

/// Evaluates all gates in a report over `suite_records` and an optional external `baseline_dir_records` set (spec §13 R-REP-4).
pub fn evaluate_report_gates(
    report_selector: &Selector,
    gates: &[ReportGate],
    suite_records: &[Record],
    baseline_dir_records: Option<&[Record]>,
) -> Result<Vec<GateOutcome>, ReportError> {
    let observed_recs = select_records(suite_records, report_selector);
    let mut outcomes = Vec::with_capacity(gates.len());

    for gate in gates {
        let base_recs: Vec<&Record> = if let Some(cmp_clause) = &gate.compare {
            let cmp_sel = Selector::Clause(cmp_clause.clone());
            let source = baseline_dir_records.unwrap_or(suite_records);
            select_records(source, &cmp_sel)
        } else if let Some(ext_base) = baseline_dir_records {
            select_records(ext_base, report_selector)
        } else {
            observed_recs.clone()
        };

        outcomes.push(evaluate_gate(gate, &observed_recs, &base_recs)?);
    }

    Ok(outcomes)
}

/// Evaluates whether the `fail_if` condition triggers (`true` = gate failed).
///
/// Supports:
/// 1. Relative percentage shorthand: `"> +5%"`, `"< -10%"`, `">= +2.5%"`, `"<= -5%"`
/// 2. Expression language (`calo_core::expr`) with `observed`, `baseline`, `delta`, `rel_diff`, and metric name bound.
fn check_fail_if(
    fail_if: &str,
    metric: &str,
    observed: f64,
    baseline: f64,
) -> Result<bool, ReportError> {
    let trimmed = fail_if.trim();

    if let Some(pct_check) = try_parse_percent_threshold(trimmed)? {
        if baseline == 0.0 {
            return Err(ReportError::ZeroBaseline {
                metric: metric.to_string(),
            });
        }
        let rel_pct = ((observed - baseline) / baseline.abs()) * 100.0;
        let failed = match pct_check.op {
            CmpOp::Gt => rel_pct > pct_check.pct,
            CmpOp::Ge => rel_pct >= pct_check.pct,
            CmpOp::Lt => rel_pct < pct_check.pct,
            CmpOp::Le => rel_pct <= pct_check.pct,
        };
        return Ok(failed);
    }

    // Evaluate as a general expression from spec §5.2
    let ast = parse(trimmed)?;
    let mut env = MapEnv::new();
    env.insert("observed", Value::Float(observed));
    env.insert("baseline", Value::Float(baseline));
    env.insert("delta", Value::Float(observed - baseline));
    if baseline != 0.0 {
        env.insert(
            "rel_diff",
            Value::Float((observed - baseline) / baseline.abs()),
        );
    }
    env.insert(metric, Value::Float(observed));
    let (base_metric, _) = crate::report::pivot::split_metric_estimator(metric);
    env.insert(base_metric, Value::Float(observed));

    match eval(&ast, &env)? {
        Value::Bool(b) => Ok(b),
        other => Err(ReportError::InvalidFailIf {
            raw: fail_if.to_string(),
            reason: format!(
                "gate expression must evaluate to bool, found {}",
                other.type_name()
            ),
        }),
    }
}

#[derive(Debug, Clone, Copy)]
enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
}

struct PercentThreshold {
    op: CmpOp,
    pct: f64,
}

fn try_parse_percent_threshold(s: &str) -> Result<Option<PercentThreshold>, ReportError> {
    let Some(without_pct) = s.strip_suffix('%') else {
        return Ok(None);
    };
    let rest = without_pct.trim();
    let (op, num_str) = if let Some(n) = rest.strip_prefix(">=") {
        (CmpOp::Ge, n.trim())
    } else if let Some(n) = rest.strip_prefix("<=") {
        (CmpOp::Le, n.trim())
    } else if let Some(n) = rest.strip_prefix('>') {
        (CmpOp::Gt, n.trim())
    } else if let Some(n) = rest.strip_prefix('<') {
        (CmpOp::Lt, n.trim())
    } else {
        return Err(ReportError::InvalidFailIf {
            raw: s.to_string(),
            reason: "percentage threshold must start with `>`, `>=`, `<`, or `<=`".to_string(),
        });
    };

    let pct: f64 = num_str.parse().map_err(|_| ReportError::InvalidFailIf {
        raw: s.to_string(),
        reason: format!("invalid percentage number `{num_str}`"),
    })?;

    Ok(Some(PercentThreshold { op, pct }))
}
