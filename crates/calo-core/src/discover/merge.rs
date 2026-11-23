//! Inventory merge rules (`--merge`), override/stale tracking, and node-class grouping (spec §6.4, §6.5).

use crate::discover::fingerprint::fingerprint;
use calo_schema::discovery::Discovery;
use calo_schema::inventory::{
    Inventory, InventoryCache, InventoryCpu, InventoryDiscoveredMeta, InventoryGpu,
    InventoryMemory, InventoryNode, MachineConnect, MachineEntry, NodeClass,
};
use calo_schema::SchemaDoc;
use std::collections::BTreeMap;

/// Category of diagnostic note emitted during `merge_inventory` (spec §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeNoteKind {
    /// Hand-written value for a discovered key was kept (`override: true`).
    OverrideKept,
    /// Machine fingerprint changed; stored `reference` marked `stale: true`.
    FingerprintChanged,
}

/// Diagnostic note returned by `merge_inventory` (spec §6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeNote {
    /// Machine name.
    pub machine: String,
    /// Note category.
    pub kind: MergeNoteKind,
    /// Field path or description.
    pub field: String,
    /// Whether an explicit `override: true` applies.
    pub is_override: bool,
    /// Human-readable message.
    pub message: String,
}

/// Converts `Discovery.observed` into an `InventoryNode` summary (spec §6.3, §7).
pub fn discovery_to_inventory_node(disc: &Discovery) -> InventoryNode {
    let obs = &disc.observed;
    let cpu = obs.cpu.as_ref().map(|c| InventoryCpu {
        model: c.model.clone(),
        microarch: c.microarch.clone(),
        sockets: c.sockets,
        cores: c.cores,
        threads: c.threads,
        smt: c.smt,
        isa: c.isa.clone(),
    });
    let cache = obs
        .cpu
        .as_ref()
        .and_then(|c| c.cache.as_ref())
        .map(|ca| InventoryCache {
            l2_kb: ca.l2_kb,
            l3_mb: ca.l3_mb,
        });
    let memory = obs.memory.as_ref().map(|m| InventoryMemory {
        gb: m
            .total_bytes
            .map(|b| (b as f64) / (1024.0 * 1024.0 * 1024.0)),
        channels: m.channels,
        numa_nodes: m.numa_nodes,
    });
    let gpu = obs.gpus.first().map(|g| InventoryGpu {
        vendor: g.vendor.clone(),
        model: g.model.clone(),
        arch: g.arch.clone(),
        per_node: Some(obs.gpus.len() as u32),
        mem_gb: g.mem_bytes.map(|b| (b as f64) / (1024.0 * 1024.0 * 1024.0)),
        link: None,
    });

    InventoryNode {
        cpu,
        cache,
        memory,
        gpu,
    }
}

/// Merges `discovered` for `machine_name` into `existing` following spec §6.4:
/// - Never touches user keys (`tags`, `workdir`, `connect`, `modules`, `policy`, `reference`, `launcher`, `env`, `probe`).
/// - Keeps hand-written values for discovered keys, marks `override: true`, and emits a warning note.
/// - Marks `reference.measured.stale = true` when the fingerprint changes.
pub fn merge_inventory(
    existing: Option<&Inventory>,
    machine_name: &str,
    connect: MachineConnect,
    discovered: &Discovery,
    calo_version: &str,
) -> (Inventory, Vec<MergeNote>) {
    let mut inv = existing.cloned().unwrap_or_else(|| Inventory {
        schema: Inventory::SCHEMA.to_string(),
        machines: BTreeMap::new(),
    });
    let mut notes = Vec::new();
    let new_fp = fingerprint(discovered);
    let obs_node = discovery_to_inventory_node(discovered);

    let entry = inv
        .machines
        .entry(machine_name.to_string())
        .or_insert_with(|| MachineEntry {
            connect: connect.clone(),
            fingerprint: None,
            tags: Vec::new(),
            node_classes: BTreeMap::new(),
            node: None,
            network: None,
            reference: None,
            workdir: None,
            modules: Vec::new(),
            env: BTreeMap::new(),
            launcher: None,
            probe: None,
            policy: None,
            discovered: None,
        });

    // Check fingerprint change and mark stored `reference` stale (spec §6.4 item 3)
    if let Some(old_fp) = &entry.fingerprint {
        if old_fp != &new_fp {
            if let Some(ref_block) = &mut entry.reference {
                if let Some(measured) = &mut ref_block.measured {
                    measured.stale = Some(true);
                }
            }
            notes.push(MergeNote {
                machine: machine_name.to_string(),
                kind: MergeNoteKind::FingerprintChanged,
                field: "fingerprint".to_string(),
                is_override: false,
                message: format!(
                    "machine `{machine_name}` fingerprint changed from `{old_fp}` to `{new_fp}`"
                ),
            });
        }
    }
    entry.fingerprint = Some(new_fp);

    // Check if an existing hand-written `node` differs from observed and keep explicit overrides (spec §6.4 item 2)
    if entry.discovered.is_none() {
        if let Some(hand_node) = &entry.node {
            if let (Some(h_cpu), Some(o_cpu)) = (&hand_node.cpu, &obs_node.cpu) {
                if h_cpu.cores.is_some() && h_cpu.cores != o_cpu.cores {
                    notes.push(MergeNote {
                        machine: machine_name.to_string(),
                        kind: MergeNoteKind::OverrideKept,
                        field: "node.cpu.cores".to_string(),
                        is_override: true,
                        message: format!(
                            "override: true — keeping hand-written `node.cpu.cores` ({:?}) over observed ({:?})",
                            h_cpu.cores, o_cpu.cores
                        ),
                    });
                }
            }
        } else {
            entry.node = Some(obs_node);
        }
    } else {
        entry.node = Some(obs_node);
    }

    entry.discovered = Some(InventoryDiscoveredMeta {
        at: discovered.at.clone(),
        calo: calo_version.to_string(),
        fields: discovered.fields.clone(),
        unresolved: discovered.unresolved.clone(),
    });

    (inv, notes)
}

/// Groups `(node_name, Discovery)` pairs by stable fingerprint into `NodeClass` entries,
/// returning warnings if nodes share a class prefix yet differ in fingerprint (spec §6.2 R-SPC-6, §6.5).
pub fn node_classes(
    nodes: &[(String, Discovery)],
    partition: Option<&str>,
) -> (BTreeMap<String, NodeClass>, Vec<String>) {
    let mut by_fp: BTreeMap<String, Vec<(&str, &Discovery)>> = BTreeMap::new();
    for (name, disc) in nodes {
        let fp = fingerprint(disc);
        by_fp.entry(fp).or_default().push((name.as_str(), disc));
    }

    let mut classes = BTreeMap::new();
    let mut warnings = Vec::new();

    if by_fp.len() > 1 {
        warnings.push(format!(
            "sampled nodes have {} distinct hardware fingerprints within the same discovery set (R-SPC-6)",
            by_fp.len()
        ));
    }

    for (idx, (_fp, members)) in by_fp.into_iter().enumerate() {
        let (_, rep) = members[0];
        let inv_node = discovery_to_inventory_node(rep);
        let class_name = if let Some(gpu) = &inv_node.gpu {
            if let Some(arch) = &gpu.arch {
                format!("gpu-{arch}-c{idx}")
            } else {
                format!("gpu-c{idx}")
            }
        } else {
            format!("cpu-c{idx}")
        };
        classes.insert(
            class_name,
            NodeClass {
                count: Some(members.len() as u32),
                partition: partition.map(str::to_string),
                features: Vec::new(),
                node: inv_node,
            },
        );
    }

    (classes, warnings)
}
