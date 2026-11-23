//! Parsers for `sinfo`, `scontrol show partition`, `scontrol show node -o`, and R-SPC-1..3 (spec §6.2).

use crate::adapters::error::AdapterError;
use calo_schema::inventory::Inventory;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Parsed Slurm partition metadata (spec §6.2 R-SPC-2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PartitionInfo {
    /// Partition name (e.g. `"gpu"`, `"cpu"`).
    pub name: String,
    /// Whether this partition is the cluster default (`*`).
    pub is_default: bool,
    /// Total node count in the partition.
    pub nodes: u32,
    /// Maximum wall-clock time limit string (e.g. `"2-00:00:00"`, `"infinite"`).
    pub time_limit: Option<String>,
    /// Allowed accounts (`AllowAccounts`).
    pub allow_accounts: Vec<String>,
    /// Allowed QOS (`AllowQOS`).
    pub allow_qos: Vec<String>,
    /// Node-type class summaries observed in this partition.
    pub node_classes: Vec<NodeClassSummary>,
}

/// Parsed Slurm node hardware & feature metadata from `scontrol show node -o` (spec §6.2 R-SPC-2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeInfo {
    /// Node hostname (`NodeName`).
    pub node_name: String,
    /// Partition names (`Partitions`).
    pub partitions: Vec<String>,
    /// Total logical CPUs (`CPUTot`).
    pub cpus: u32,
    /// Real memory in MiB (`RealMemory`).
    pub real_memory_mb: u64,
    /// Available node features (`AvailableFeatures` or `ActiveFeatures`).
    pub features: Vec<String>,
    /// Generic resources string (`Gres`, e.g. `"gpu:a100:4(S:0-1)"`).
    pub gres: Option<String>,
}

/// Summary of a homogeneous node class within a Slurm partition (spec §6.2 R-SPC-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeClassSummary {
    /// Number of nodes in this class.
    pub count: u32,
    /// CPUs per node.
    pub cpus: u32,
    /// Memory in MiB per node.
    pub memory_mb: u64,
    /// Sorted node features.
    pub features: Vec<String>,
    /// Normalized GRES string (if any).
    pub gres: Option<String>,
}

/// Enforces spec §6.2 R-SPC-1: Calorimeter MUST NOT submit any job to a partition not named
/// via `--partition` (`requested`) or `inventory.machines.<name>.connect.partitions`.
pub fn assert_partition_named(
    requested: &[String],
    inventory: Option<&Inventory>,
    machine_name: Option<&str>,
    partition: &str,
) -> Result<(), AdapterError> {
    if requested.iter().any(|p| p == partition) {
        return Ok(());
    }
    if let (Some(inv), Some(m_name)) = (inventory, machine_name) {
        if let Some(m) = inv.machines.get(m_name) {
            if m.connect.partitions.iter().any(|p| p == partition) {
                return Ok(());
            }
        }
    }
    Err(AdapterError::UnnamedPartition {
        partition: partition.to_string(),
    })
}

/// Parses `sinfo -h -o "%P|%D|%l|%c|%m|%f|%G"` output into `PartitionInfo` records (spec §6.2).
pub fn parse_sinfo_partitions(output: &str) -> Vec<PartitionInfo> {
    let mut by_name: BTreeMap<String, PartitionInfo> = BTreeMap::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('|').collect();
        if cols.is_empty() {
            continue;
        }
        let raw_name = cols[0].trim();
        let is_default = raw_name.ends_with('*');
        let name = raw_name.trim_end_matches('*').to_string();
        if name.is_empty() {
            continue;
        }

        let nodes = cols
            .get(1)
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let time_limit = cols
            .get(2)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let cpus = cols
            .get(3)
            .and_then(|s| s.trim().trim_end_matches('+').parse::<u32>().ok())
            .unwrap_or(0);
        let memory_mb = cols
            .get(4)
            .and_then(|s| s.trim().trim_end_matches('+').parse::<u64>().ok())
            .unwrap_or(0);
        let features: Vec<String> = cols
            .get(5)
            .map(|s| {
                let t = s.trim();
                if t.is_empty() || t == "(null)" {
                    Vec::new()
                } else {
                    t.split(',').map(|f| f.trim().to_string()).collect()
                }
            })
            .unwrap_or_default();
        let gres = cols
            .get(6)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "(null)");

        let entry = by_name.entry(name.clone()).or_insert_with(|| PartitionInfo {
            name,
            is_default,
            nodes: 0,
            time_limit: time_limit.clone(),
            ..PartitionInfo::default()
        });
        entry.nodes = entry.nodes.saturating_add(nodes);
        if cpus > 0 || memory_mb > 0 || gres.is_some() {
            entry.node_classes.push(NodeClassSummary {
                count: nodes,
                cpus,
                memory_mb,
                features,
                gres,
            });
        }
    }

    by_name.into_values().collect()
}

/// Parses `scontrol show partition -o` output into `PartitionInfo` records (spec §6.2 R-SPC-2).
pub fn parse_scontrol_partitions(output: &str) -> Vec<PartitionInfo> {
    let mut out = Vec::new();
    for line in output.lines() {
        let kv = parse_scontrol_kv_line(line);
        let Some(name) = kv.get("PartitionName") else {
            continue;
        };
        let is_default = kv
            .get("Default")
            .is_some_and(|v| v.eq_ignore_ascii_case("YES"));
        let nodes = kv
            .get("TotalNodes")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let time_limit = kv.get("MaxTime").cloned();
        let allow_accounts = split_list(kv.get("AllowAccounts").map(String::as_str));
        let allow_qos = split_list(kv.get("AllowQos").or_else(|| kv.get("AllowQOS")).map(String::as_str));

        out.push(PartitionInfo {
            name: name.clone(),
            is_default,
            nodes,
            time_limit,
            allow_accounts,
            allow_qos,
            node_classes: Vec::new(),
        });
    }
    out
}

/// Parses `scontrol show node -o` output into `NodeInfo` records (spec §6.2 R-SPC-2).
pub fn parse_scontrol_nodes(output: &str) -> Vec<NodeInfo> {
    let mut out = Vec::new();
    for line in output.lines() {
        let kv = parse_scontrol_kv_line(line);
        let Some(node_name) = kv.get("NodeName") else {
            continue;
        };
        let partitions = split_list(kv.get("Partitions").map(String::as_str));
        let cpus = kv
            .get("CPUTot")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let real_memory_mb = kv
            .get("RealMemory")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let features = split_list(
            kv.get("AvailableFeatures")
                .or_else(|| kv.get("ActiveFeatures"))
                .map(String::as_str),
        );
        let gres = kv
            .get("Gres")
            .filter(|v| !v.is_empty() && *v != "(null)")
            .cloned();

        out.push(NodeInfo {
            node_name: node_name.clone(),
            partitions,
            cpus,
            real_memory_mb,
            features,
            gres,
        });
    }
    out
}

/// Groups `NodeInfo` records into homogeneous `NodeClassSummary` groups by `(cpus, memory, features, gres)` (spec §6.2 R-SPC-2).
pub fn group_node_classes(nodes: &[NodeInfo]) -> Vec<NodeClassSummary> {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    struct Key {
        cpus: u32,
        memory_mb: u64,
        features: Vec<String>,
        gres: Option<String>,
    }

    let mut groups: BTreeMap<Key, u32> = BTreeMap::new();
    for n in nodes {
        let mut feats = n.features.clone();
        feats.sort();
        feats.dedup();
        let norm_gres = n.gres.as_deref().map(normalize_gres);
        let key = Key {
            cpus: n.cpus,
            memory_mb: n.real_memory_mb,
            features: feats,
            gres: norm_gres,
        };
        *groups.entry(key).or_insert(0) += 1;
    }

    groups
        .into_iter()
        .map(|(k, count)| NodeClassSummary {
            count,
            cpus: k.cpus,
            memory_mb: k.memory_mb,
            features: k.features,
            gres: k.gres,
        })
        .collect()
}

/// Formats the passive `--list-partitions` report and ready-to-run `calo spec` command (spec §6.2 R-SPC-2, R-SPC-3).
pub fn list_partitions_report(partitions: &[PartitionInfo], login_target: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<16} {:<7} {:<8} {:<14} {:<18} CLASSES",
        "PARTITION", "NODES", "DEFAULT", "TIME_LIMIT", "ACCOUNTS"
    );

    let mut suggested = BTreeSet::new();
    for p in partitions {
        suggested.insert(p.name.as_str());
        let def_flag = if p.is_default { "yes" } else { "no" };
        let tl = p.time_limit.as_deref().unwrap_or("-");
        let accts = if p.allow_accounts.is_empty() {
            "*".to_string()
        } else {
            p.allow_accounts.join(",")
        };
        let classes_str = if p.node_classes.is_empty() {
            "-".to_string()
        } else {
            p.node_classes
                .iter()
                .map(|c| {
                    let g = c.gres.as_deref().unwrap_or("cpu-only");
                    format!("{}x({}c,{}MiB,{})", c.count, c.cpus, c.memory_mb, g)
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        let _ = writeln!(
            out,
            "{:<16} {:<7} {:<8} {:<14} {:<18} {}",
            p.name, p.nodes, def_flag, tl, accts, classes_str
        );
    }

    if !suggested.is_empty() {
        let flags: Vec<String> = suggested
            .iter()
            .map(|p| format!("--partition {p}"))
            .collect();
        let _ = writeln!(
            out,
            "\nReady-to-run discovery command (R-SPC-3):\n  calo spec --connect slurm://{login_target} {}",
            flags.join(" ")
        );
    }

    out
}

fn normalize_gres(raw: &str) -> String {
    // Strip socket binding suffix like `(S:0-1)`
    raw.split('(').next().unwrap_or(raw).trim().to_string()
}

fn split_list(raw: Option<&str>) -> Vec<String> {
    let Some(s) = raw else {
        return Vec::new();
    };
    let t = s.trim();
    if t.is_empty() || t == "(null)" || t.eq_ignore_ascii_case("ALL") {
        return Vec::new();
    }
    t.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn parse_scontrol_kv_line(line: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for token in line.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}
