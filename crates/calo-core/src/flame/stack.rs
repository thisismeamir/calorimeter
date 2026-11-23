//! `StackTable` ingestion, folded-stack text I/O, and deterministic `FlameTree` (spec §13 R-REP-3).

use crate::flame::error::FlameError;
use crate::symbols::Frame;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A single weighted call stack row inside `StackTable` (spec §13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackRow {
    /// Bottom-up call stack (`stack[0]` is root, `stack.last()` is leaf).
    pub stack: Vec<Frame>,
    /// Sample count or inclusive weight.
    pub weight: u64,
    /// Optional MPI rank grouping key.
    pub rank: Option<u32>,
}

/// Collection of weighted, deduplicated call stacks supporting rank/class grouping (spec §13).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StackTable {
    rows: Vec<StackRow>,
}

impl StackTable {
    /// Creates an empty `StackTable`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingests `(stack, weight)` and merges identical stacks with the same `rank` key.
    pub fn push(&mut self, stack: Vec<Frame>, weight: u64, rank: Option<u32>) {
        if weight == 0 {
            return;
        }
        if let Some(existing) = self
            .rows
            .iter_mut()
            .find(|r| r.rank == rank && r.stack == stack)
        {
            existing.weight = existing.weight.saturating_add(weight);
        } else {
            self.rows.push(StackRow {
                stack,
                weight,
                rank,
            });
        }
    }

    /// Returns all merged stack rows.
    pub fn rows(&self) -> &[StackRow] {
        &self.rows
    }

    /// Partitions this `StackTable` into per-rank `StackTable`s.
    pub fn group_by_rank(&self) -> BTreeMap<u32, StackTable> {
        let mut map: BTreeMap<u32, StackTable> = BTreeMap::new();
        for r in &self.rows {
            let rk = r.rank.unwrap_or(0);
            map.entry(rk)
                .or_default()
                .push(r.stack.clone(), r.weight, r.rank);
        }
        map
    }

    /// Collapses each stack frame to its `class` label (or `"other"` if unclassified) and merges identical class stacks.
    pub fn collapse_by_class(&self) -> StackTable {
        let mut out = StackTable::new();
        for r in &self.rows {
            let collapsed: Vec<Frame> = r
                .stack
                .iter()
                .map(|f| Frame {
                    name: f.class.clone().unwrap_or_else(|| "other".to_string()),
                    class: f.class.clone(),
                    file: None,
                    line: None,
                })
                .collect();
            out.push(collapsed, r.weight, r.rank);
        }
        out
    }
}

/// Serializes `table` into deterministic sorted folded-stack text (`a;b;c 42\n`, spec §13).
pub fn to_folded(table: &StackTable) -> String {
    let mut merged: BTreeMap<String, u64> = BTreeMap::new();
    for row in table.rows() {
        let key: String = row
            .stack
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        *merged.entry(key).or_insert(0) += row.weight;
    }

    let mut out = String::new();
    for (stack_str, weight) in merged {
        let _ = writeln!(out, "{stack_str} {weight}");
    }
    out
}

/// Parses folded-stack text (`a;b;c 42`) into a merged `StackTable` (spec §13).
pub fn from_folded(input: &str) -> Result<StackTable, FlameError> {
    let mut table = StackTable::new();
    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((stack_part, weight_part)) = trimmed.rsplit_once(char::is_whitespace) else {
            return Err(FlameError::InvalidFoldedLine {
                line: line_no,
                reason: "missing whitespace separator before weight".to_string(),
            });
        };
        let weight: u64 = weight_part
            .trim()
            .parse()
            .map_err(|_| FlameError::InvalidFoldedLine {
                line: line_no,
                reason: format!("invalid integer weight `{}`", weight_part.trim()),
            })?;

        let stack: Vec<Frame> = stack_part
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|name| Frame {
                name: name.to_string(),
                class: None,
                file: None,
                line: None,
            })
            .collect();

        table.push(stack, weight, None);
    }
    Ok(table)
}

/// Deterministic call-tree node for flamegraph rendering (spec §13 R-REP-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlameNode {
    /// Frame symbol or class name.
    pub name: String,
    /// Optional symbol class.
    pub class: Option<String>,
    /// Samples terminating directly in this frame.
    pub self_weight: u64,
    /// Total inclusive samples (self + all children).
    pub total_weight: u64,
    /// Ordered child frames keyed by name (`BTreeMap` guarantees deterministic diffing).
    pub children: BTreeMap<String, FlameNode>,
}

impl FlameNode {
    fn new(name: impl Into<String>, class: Option<String>) -> Self {
        Self {
            name: name.into(),
            class,
            self_weight: 0,
            total_weight: 0,
            children: BTreeMap::new(),
        }
    }

    /// Maximum depth of the subtree rooted at `self`.
    pub fn max_depth(&self) -> usize {
        if self.children.is_empty() {
            1
        } else {
            1 + self
                .children
                .values()
                .map(FlameNode::max_depth)
                .max()
                .unwrap_or(0)
        }
    }
}

/// Rooted flamegraph tree built from a `StackTable` (spec §13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlameTree {
    /// Synthetic root node (`"all"`).
    pub root: FlameNode,
}

impl FlameTree {
    /// Builds a `FlameTree` from `table`, pruning subtrees below `min_pct` (default `0.05`%)
    /// while preserving `root.total_weight` consistency.
    pub fn from_table(table: &StackTable, min_pct: f64, collapse_by_class: bool) -> Self {
        let source = if collapse_by_class {
            std::borrow::Cow::Owned(table.collapse_by_class())
        } else {
            std::borrow::Cow::Borrowed(table)
        };

        let mut root = FlameNode::new("all", None);
        for row in source.rows() {
            root.total_weight = root.total_weight.saturating_add(row.weight);
            if row.stack.is_empty() {
                root.self_weight = root.self_weight.saturating_add(row.weight);
                continue;
            }
            let mut cur = &mut root;
            for (i, frame) in row.stack.iter().enumerate() {
                let child = cur
                    .children
                    .entry(frame.name.clone())
                    .or_insert_with(|| FlameNode::new(frame.name.clone(), frame.class.clone()));
                child.total_weight = child.total_weight.saturating_add(row.weight);
                if i + 1 == row.stack.len() {
                    child.self_weight = child.self_weight.saturating_add(row.weight);
                }
                cur = child;
            }
        }

        if min_pct > 0.0 && root.total_weight > 0 {
            let min_weight = ((min_pct / 100.0) * (root.total_weight as f64)).ceil() as u64;
            prune_node(&mut root, min_weight);
        }

        Self { root }
    }

    /// Computes total inclusive weight per frame name across the tree (used for diff flamegraphs).
    pub fn totals_by_name(&self) -> BTreeMap<String, u64> {
        let mut map = BTreeMap::new();
        collect_totals(&self.root, &mut map, true);
        map
    }
}

fn prune_node(node: &mut FlameNode, min_weight: u64) {
    node.children.retain(|_, child| child.total_weight >= min_weight);
    for child in node.children.values_mut() {
        prune_node(child, min_weight);
    }
}

fn collect_totals(node: &FlameNode, out: &mut BTreeMap<String, u64>, is_root: bool) {
    if !is_root {
        *out.entry(node.name.clone()).or_insert(0) += node.total_weight;
    }
    for child in node.children.values() {
        collect_totals(child, out, false);
    }
}
