use calo_core::discover::{fingerprint, merge_inventory, node_classes, MergeNoteKind};
use calo_schema::discovery::{
    Discovery, ObservedCpu, ObservedGpu, ObservedGpuClocks, ObservedHardware, ObservedMemory,
    ObservedMpi, ObservedPolicy,
};
use calo_schema::inventory::{
    ConnectType, InventoryReference, MachineConnect, ReferenceMeasured,
};
use calo_schema::SchemaDoc;
use std::collections::BTreeMap;

fn base_discovery() -> Discovery {
    Discovery {
        schema: Discovery::SCHEMA.to_string(),
        probe_version: "0.1.0".to_string(),
        at: "2026-11-16T00:00:00Z".to_string(),
        fingerprint: "placeholder".to_string(),
        tiers_supported: None,
        observed: ObservedHardware {
            cpu: Some(ObservedCpu {
                model: Some("EPYC 7763".to_string()),
                microarch: Some("zen3".to_string()),
                sockets: Some(1),
                cores: Some(64),
                threads: Some(128),
                smt: Some(true),
                isa: vec!["avx2".to_string()],
                cache: None,
            }),
            memory: Some(ObservedMemory {
                total_bytes: Some(256 * 1024 * 1024 * 1024),
                numa_nodes: Some(4),
                ..ObservedMemory::default()
            }),
            kernel: Some("6.6.0".to_string()),
            policy: Some(ObservedPolicy {
                governor: Some("performance".to_string()),
                ..ObservedPolicy::default()
            }),
            gpus: vec![ObservedGpu {
                index: 0,
                vendor: Some("nvidia".to_string()),
                model: Some("A100".to_string()),
                arch: Some("sm_80".to_string()),
                sms: Some(108),
                mem_bytes: Some(80 * 1024 * 1024 * 1024),
                driver: Some("550.54".to_string()),
                runtime: None,
                ecc: Some(true),
                clocks: Some(ObservedGpuClocks {
                    sm_mhz: Some(1410),
                    mem_mhz: Some(1593),
                }),
                link: vec![],
                numa: Some(0),
                profiling_permitted: Some(true),
            }],
            mpi: Some(ObservedMpi {
                library: Some("openmpi".to_string()),
                version: Some("5.0.2".to_string()),
                ..ObservedMpi::default()
            }),
            ..ObservedHardware::default()
        },
        toolchain: None,
        storage: vec![],
        fields: BTreeMap::new(),
        unresolved: vec![],
    }
}

#[test]
fn fingerprint_stable_under_volatile_changes_and_sensitive_to_stable_fields() {
    let d1 = base_discovery();
    let fp1 = fingerprint(&d1);

    // Mutate volatile state (governor, GPU clocks, hostname, timestamp)
    let mut d_volatile = d1.clone();
    d_volatile.at = "2026-11-17T12:00:00Z".to_string();
    d_volatile.observed.hostname = Some("node-99".to_string());
    d_volatile.observed.policy.as_mut().unwrap().governor = Some("powersave".to_string());
    d_volatile.observed.gpus[0].clocks.as_mut().unwrap().sm_mhz = Some(900);
    assert_eq!(fp1, fingerprint(&d_volatile));

    // Mutate stable fields (kernel, CPU cores, GPU driver) -> fingerprint MUST change
    let mut d_kern = d1.clone();
    d_kern.observed.kernel = Some("6.8.0".to_string());
    assert_ne!(fp1, fingerprint(&d_kern));

    let mut d_cpu = d1.clone();
    d_cpu.observed.cpu.as_mut().unwrap().cores = Some(32);
    assert_ne!(fp1, fingerprint(&d_cpu));

    let mut d_gpu = d1;
    d_gpu.observed.gpus[0].driver = Some("560.00".to_string());
    assert_ne!(fp1, fingerprint(&d_gpu));
}

#[test]
fn merge_inventory_preserves_user_keys_marks_overrides_and_stale_reference() {
    let mut disc = base_discovery();
    let fp_initial = fingerprint(&disc);
    let mc = MachineConnect {
        connect_type: ConnectType::Local,
        host: None,
        user: None,
        partitions: vec![],
        account: None,
    };

    let (mut inv, notes) = merge_inventory(None, "local", mc.clone(), &disc, "0.1.0");
    assert!(notes.is_empty());
    assert_eq!(inv.machines["local"].fingerprint.as_deref(), Some(fp_initial.as_str()));

    // User curates tags, workdir, and calibrated reference
    let entry = inv.machines.get_mut("local").unwrap();
    entry.tags = vec!["my-tag".to_string()];
    entry.workdir = Some("/scratch/calo".to_string());
    entry.reference = Some(InventoryReference {
        measured: Some(ReferenceMeasured {
            probe: "0.1.0".to_string(),
            fingerprint: fp_initial,
            at: "2026-11-16T00:00:00Z".to_string(),
            stale: Some(false),
        }),
        ..InventoryReference::default()
    });

    // Kernel upgrade changes fingerprint -> reference marked stale, user keys untouched
    disc.observed.kernel = Some("6.9.0".to_string());
    let (inv2, notes2) = merge_inventory(Some(&inv), "local", mc, &disc, "0.1.0");
    assert_eq!(inv2.machines["local"].tags, vec!["my-tag".to_string()]);
    assert_eq!(
        inv2.machines["local"].workdir.as_deref(),
        Some("/scratch/calo")
    );
    assert_eq!(
        inv2.machines["local"]
            .reference
            .as_ref()
            .unwrap()
            .measured
            .as_ref()
            .unwrap()
            .stale,
        Some(true)
    );
    assert!(notes2
        .iter()
        .any(|n| n.kind == MergeNoteKind::FingerprintChanged));
}

#[test]
fn node_classes_grouping_warns_on_divergent_nodes() {
    let d1 = base_discovery();
    let mut d2 = base_discovery();
    d2.observed.cpu.as_mut().unwrap().cores = Some(32);

    let (classes, warnings) = node_classes(
        &[("n1".to_string(), d1), ("n2".to_string(), d2)],
        Some("gpu"),
    );
    assert_eq!(classes.len(), 2);
    assert_eq!(warnings.len(), 1);
}
