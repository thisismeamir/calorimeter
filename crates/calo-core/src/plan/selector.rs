//! Benchmark and run selectors (spec §5.3).

use crate::expr::{eval, parse, Env, Value};
use calo_schema::common::{Mode, ScalarValue, Tier, TiersActive};
use calo_schema::inventory::{Inventory, InventoryNode, MachineEntry};
use calo_schema::suite::{ReportSelect, SelectorClause};
use std::collections::BTreeMap;

/// Descriptor of a candidate run or benchmark evaluated against a `Selector` (spec §5.3).
#[derive(Debug, Clone)]
pub struct RunDescriptor<'a> {
    /// Benchmark name.
    pub benchmark: &'a str,
    /// Optional benchmark group name.
    pub group: Option<&'a str>,
    /// Execution mode (`measure`, `profile`, `kernel`).
    pub mode: Mode,
    /// Pinned parameter map for the run.
    pub params: &'a BTreeMap<String, ScalarValue>,
    /// Active or supported capability tiers for the run.
    pub tiers: &'a TiersActive,
    /// Optional machine entry from `Inventory` for `machine.*` / `node.*` lookups in `where`.
    pub machine: Option<&'a MachineEntry>,
    /// Optional resolved `InventoryNode` (either single-node `machine.node` or selected `node_class.node`).
    pub node: Option<&'a InventoryNode>,
}

/// Compiled or structured selector matching `all`, a single clause, or a union of clauses (spec §5.3).
#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    /// Selects all runs (`select: all`).
    All,
    /// Single selector clause where all specified keys intersect.
    Clause(SelectorClause),
    /// List of selector clauses where clauses form a union.
    Union(Vec<SelectorClause>),
}

impl From<&ReportSelect> for Selector {
    fn from(rs: &ReportSelect) -> Self {
        match rs {
            ReportSelect::All(s) if s.trim() == "all" => Self::All,
            ReportSelect::All(bench_name) => Self::Clause(SelectorClause {
                benchmark: Some(bench_name.clone()),
                ..SelectorClause::default()
            }),
            ReportSelect::Single(c) => Self::Clause(c.clone()),
            ReportSelect::List(list) => Self::Union(list.clone()),
        }
    }
}

/// Evaluates whether `desc` matches `selector` (spec §5.3).
///
/// Keys within one selector clause intersect; a list of clauses is a union; `all` selects everything.
pub fn matches(selector: &Selector, desc: &RunDescriptor<'_>) -> bool {
    match selector {
        Selector::All => true,
        Selector::Clause(clause) => clause_matches(clause, desc),
        Selector::Union(clauses) => clauses.iter().any(|c| clause_matches(c, desc)),
    }
}

fn clause_matches(clause: &SelectorClause, desc: &RunDescriptor<'_>) -> bool {
    if let Some(pat) = &clause.benchmark {
        if !glob_match(pat, desc.benchmark) {
            return false;
        }
    }

    if let Some(expected_group) = &clause.group {
        let Some(actual_group) = desc.group else {
            return false;
        };
        if !glob_match(expected_group, actual_group) {
            return false;
        }
    }

    if let Some(expected_mode) = clause.mode {
        if desc.mode != expected_mode {
            return false;
        }
    }

    for (k, expected_val) in &clause.params {
        let Some(actual_val) = desc.params.get(k) else {
            return false;
        };
        if !scalars_equal(actual_val, expected_val) {
            return false;
        }
    }

    for req_tier in &clause.tier {
        if !desc.tiers.satisfies(*req_tier) {
            return false;
        }
    }

    if let Some(where_src) = &clause.where_expr {
        let Ok(ast) = parse(where_src) else {
            return false;
        };
        let env = RunExprEnv {
            params: desc.params,
            machine: desc.machine,
            node: desc.node,
        };
        match eval(&ast, &env) {
            Ok(Value::Bool(true)) => {}
            _ => return false,
        }
    }

    true
}

/// Compares two `ScalarValue`s with numeric coercion between `Int` and `Float` when numerically equal.
pub(crate) fn scalars_equal(a: &ScalarValue, b: &ScalarValue) -> bool {
    match (a, b) {
        (ScalarValue::Int(x), ScalarValue::Int(y)) => x == y,
        (ScalarValue::Float(x), ScalarValue::Float(y)) => x == y,
        (ScalarValue::Int(x), ScalarValue::Float(y)) => (*x as f64) == *y,
        (ScalarValue::Float(x), ScalarValue::Int(y)) => *x == (*y as f64),
        (ScalarValue::Bool(x), ScalarValue::Bool(y)) => x == y,
        (ScalarValue::Str(x), ScalarValue::Str(y)) => x == y,
        _ => false,
    }
}

/// Simple wildcard glob matcher supporting `*` and `?` (spec §5.3).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_idx: Option<usize> = None;
    let mut match_idx = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_idx = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(s_idx) = star_idx {
            pi = s_idx + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }

    pi == p.len()
}

/// Resolves a `"machine"` or `"machine/node_class"` string against an `Inventory` (spec §6.5, §7).
pub fn resolve_machine_node<'a>(
    inventory: &'a Inventory,
    machine_spec: &str,
) -> Option<(&'a MachineEntry, Option<&'a InventoryNode>)> {
    if let Some((m_name, class_name)) = machine_spec.split_once('/') {
        let m = inventory.machines.get(m_name)?;
        let nc = m.node_classes.get(class_name)?;
        Some((m, Some(&nc.node)))
    } else {
        let m = inventory.machines.get(machine_spec)?;
        let node = m
            .node
            .as_ref()
            .or_else(|| m.node_classes.values().next().map(|nc| &nc.node));
        Some((m, node))
    }
}

/// Expression evaluation environment providing `params.<k>`, bare `<k>`, `machine.<path>`, and `node.<path>` (spec §5.2).
pub(crate) struct RunExprEnv<'a> {
    pub params: &'a BTreeMap<String, ScalarValue>,
    pub machine: Option<&'a MachineEntry>,
    pub node: Option<&'a InventoryNode>,
}

impl Env for RunExprEnv<'_> {
    fn get(&self, path: &str) -> Option<Value> {
        if let Some(param_key) = path.strip_prefix("params.") {
            return self.params.get(param_key).map(Value::from_scalar);
        }
        if let Some(val) = self.params.get(path) {
            return Some(Value::from_scalar(val));
        }
        if let Some(node_sub) = path.strip_prefix("machine.node.") {
            return lookup_node_field(self.node?, node_sub);
        }
        if let Some(node_sub) = path.strip_prefix("node.") {
            return lookup_node_field(self.node?, node_sub);
        }
        if let Some(mach_sub) = path.strip_prefix("machine.") {
            let m = self.machine?;
            return match mach_sub {
                "fingerprint" => m.fingerprint.as_ref().map(|s| Value::Str(s.clone())),
                "workdir" => m.workdir.as_ref().map(|s| Value::Str(s.clone())),
                "network.bw_gbps" => m.network.as_ref()?.bw_gbps.map(Value::Float),
                "launcher.gpu_aware_mpi" => m.launcher.as_ref()?.gpu_aware_mpi.map(Value::Bool),
                _ => None,
            };
        }
        let _ = Tier::T0;
        None
    }
}

fn lookup_node_field(node: &InventoryNode, subpath: &str) -> Option<Value> {
    match subpath {
        "cpu.model" => node.cpu.as_ref()?.model.as_ref().map(|s| Value::Str(s.clone())),
        "cpu.microarch" => node
            .cpu
            .as_ref()?
            .microarch
            .as_ref()
            .map(|s| Value::Str(s.clone())),
        "cpu.sockets" => node.cpu.as_ref()?.sockets.map(|v| Value::Int(i64::from(v))),
        "cpu.cores" => node.cpu.as_ref()?.cores.map(|v| Value::Int(i64::from(v))),
        "cpu.threads" => node.cpu.as_ref()?.threads.map(|v| Value::Int(i64::from(v))),
        "cpu.smt" => node.cpu.as_ref()?.smt.map(Value::Bool),
        " cpu.isa" | "cpu.isa" => {
            let isa = &node.cpu.as_ref()?.isa;
            Some(Value::List(
                isa.iter().map(|s| Value::Str(s.clone())).collect(),
            ))
        }
        "cache.l2_kb" => node.cache.as_ref()?.l2_kb.map(|v| Value::Int(v as i64)),
        "cache.l3_mb" => node.cache.as_ref()?.l3_mb.map(|v| Value::Int(v as i64)),
        "memory.gb" => node.memory.as_ref()?.gb.map(Value::Float),
        "memory.channels" => node
            .memory
            .as_ref()?
            .channels
            .map(|v| Value::Int(i64::from(v))),
        "memory.numa_nodes" => node
            .memory
            .as_ref()?
            .numa_nodes
            .map(|v| Value::Int(i64::from(v))),
        "gpu.vendor" => node
            .gpu
            .as_ref()?
            .vendor
            .as_ref()
            .map(|s| Value::Str(s.clone())),
        "gpu.model" => node.gpu.as_ref()?.model.as_ref().map(|s| Value::Str(s.clone())),
        "gpu.arch" => node.gpu.as_ref()?.arch.as_ref().map(|s| Value::Str(s.clone())),
        "gpu.per_node" => node
            .gpu
            .as_ref()?
            .per_node
            .map(|v| Value::Int(i64::from(v))),
        "gpu.mem_gb" => node.gpu.as_ref()?.mem_gb.map(Value::Float),
        _ => None,
    }
}
