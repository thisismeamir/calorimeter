use calo_schema::common::{
    Confidence, CpuTier, GpuTier, Mode, MpiTier, ScalarValue, Status, Tier, TiersActive, Warning,
    WarningCode,
};
use calo_schema::discovery::{Discovery, FieldProvenance, ObservedHardware};
use calo_schema::inventory::{ConnectType, Inventory, MachineConnect, MachineEntry};
use calo_schema::manifest::{Manifest, ManifestRun, ManifestSuite};
use calo_schema::record::Record;
use calo_schema::runspec::{RunSpec, RunSpecOutputs};
use calo_schema::suite::{Suite, SuiteMeta};
use calo_schema::table::{ColumnDef, ColumnType, TableHeader};
use calo_schema::{from_yaml_str, to_yaml_string, SchemaDoc, SchemaError};
use std::collections::BTreeMap;

const GOLDEN_MINIMAL: &str = include_str!("golden/record_minimal.yaml");
const GOLDEN_FULL: &str = include_str!("golden/record_full.yaml");

#[test]
fn golden_record_minimal_round_trip() {
    let rec: Record = from_yaml_str(GOLDEN_MINIMAL).expect("minimal golden record must parse");
    assert_eq!(rec.identity.run_id, "baseline.r0");
    assert!(rec.observations.scalars.is_none());
    assert!(rec.observations.gpu.is_none());
    assert!(rec.observations.mpi.is_none());

    let serialized = to_yaml_string(&rec).expect("must serialize");
    let reparsed: Record = from_yaml_str(&serialized).expect("must reparse");
    assert_eq!(rec, reparsed);
}

#[test]
fn golden_record_full_round_trip_and_energy_distinction() {
    let rec: Record = from_yaml_str(GOLDEN_FULL).expect("full golden record must parse");

    // Verify 2-level region tree
    let regions = rec.observations.regions.as_ref().expect("regions present");
    assert_eq!(regions.threads.len(), 1);
    assert_eq!(regions.threads[0].tree.len(), 1);
    assert_eq!(regions.threads[0].tree[0].name, "step_evolve");
    assert_eq!(regions.threads[0].tree[0].children.len(), 2);
    assert_eq!(regions.threads[0].tree[0].children[0].name, "field_update");
    assert_eq!(regions.threads[0].tree[0].children[1].name, "fft");

    // Verify 0.0 vs omitted distinction on energy_j (spec §0.3, R-REC-6)
    let scalars = rec.observations.scalars.as_ref().expect("scalars present");
    let energy = scalars.energy_j.as_ref().expect("energy_j present");
    assert_eq!(energy.pkg, Some(2450.5));
    assert_eq!(energy.dram, None); // omitted in YAML -> None
    assert_eq!(energy.gpu, Some(0.0)); // measured zero in YAML -> Some(0.0)

    let serialized = to_yaml_string(&rec).expect("must serialize");
    assert!(!serialized.contains("dram:"));
    assert!(serialized.contains("gpu: 0.0"));

    let reparsed: Record = from_yaml_str(&serialized).expect("must reparse");
    assert_eq!(rec, reparsed);
    let reparsed_energy = reparsed
        .observations
        .scalars
        .as_ref()
        .unwrap()
        .energy_j
        .as_ref()
        .unwrap();
    assert_eq!(reparsed_energy.dram, None);
    assert_eq!(reparsed_energy.gpu, Some(0.0));
}

#[test]
fn version_mismatch_returns_typed_error() {
    let bad_yaml = "schema: calo.record/1\nidentity:\n  run_id: x\n  benchmark: b\n  mode: measure\n  status: ok\n";
    let err = from_yaml_str::<Record>(bad_yaml).expect_err("must reject wrong version");
    assert_eq!(
        err,
        SchemaError::VersionMismatch {
            expected: "calo.record/2".to_string(),
            found: "calo.record/1".to_string(),
        }
    );
}

#[test]
fn unknown_fields_are_ignored_on_record_runspec_discovery() {
    let record_with_unknown = format!(
        "{GOLDEN_MINIMAL}\nfuture_top_level_field: 42\n"
    );
    let rec: Record =
        from_yaml_str(&record_with_unknown).expect("unknown fields must be ignored on Record");
    assert_eq!(rec.identity.run_id, "baseline.r0");

    let runspec_yaml = r#"
schema: calo.runspec/1
run_id: test.r0
mode: measure
command: ["./app"]
cwd: /tmp
outputs:
  dir: /tmp/out
future_runspec_option: true
"#;
    let rs: RunSpec =
        from_yaml_str(runspec_yaml).expect("unknown fields must be ignored on RunSpec");
    assert_eq!(rs.run_id, "test.r0");

    let discovery_yaml = r#"
schema: calo.discovery/1
probe_version: 0.1.0
at: "2026-11-15T20:00:00Z"
fingerprint: deadbeef
observed:
  kernel: 6.6.0
  future_hw_sensor: "quantum_clock"
future_discovery_key: [1, 2, 3]
"#;
    let disc: Discovery =
        from_yaml_str(discovery_yaml).expect("unknown fields must be ignored on Discovery");
    assert_eq!(disc.fingerprint, "deadbeef");
}

#[test]
fn tier_authoritative_check_rejects_gpu_block_when_gpu_tier_absent() {
    let mut rec: Record = from_yaml_str(GOLDEN_FULL).expect("parse full");
    rec.provenance.tiers_active.gpu = None;

    let err = rec
        .tier_authoritative_check()
        .expect_err("must fail when gpu block is present without active gpu tier");
    assert_eq!(
        err,
        SchemaError::InactiveTierObservation {
            block: "observations.gpu",
            required_tier: "G0",
        }
    );
}

#[test]
fn round_trip_all_other_schema_types() {
    // 1. RunSpec
    let runspec = RunSpec {
        schema: RunSpec::SCHEMA.to_string(),
        run_id: "bench.N_64.r0".to_string(),
        mode: Mode::Measure,
        command: vec!["./model".to_string(), "N=64".to_string()],
        cwd: "/work".to_string(),
        env: BTreeMap::from([("OMP_NUM_THREADS".to_string(), "4".to_string())]),
        affinity: None,
        rlimits: None,
        counters: vec!["cycles".to_string()],
        sample_hz: Some(10),
        profile: None,
        mpi: None,
        gpu: None,
        shim_inject: None,
        regions: None,
        channel: None,
        rank: None,
        outputs: RunSpecOutputs {
            dir: "/work/out".to_string(),
        },
    };
    let s = to_yaml_string(&runspec).unwrap();
    assert_eq!(runspec, from_yaml_str::<RunSpec>(&s).unwrap());

    // 2. Discovery
    let discovery = Discovery {
        schema: Discovery::SCHEMA.to_string(),
        probe_version: "0.1.0".to_string(),
        at: "2026-11-15T20:00:00Z".to_string(),
        fingerprint: "abcdef0123456789".to_string(),
        tiers_supported: Some(TiersActive {
            cpu: Some(CpuTier::T2),
            mpi: Some(MpiTier::M1),
            gpu: Some(GpuTier::G0),
        }),
        observed: ObservedHardware::default(),
        toolchain: None,
        storage: vec![],
        fields: BTreeMap::from([(
            "cpu.model".to_string(),
            FieldProvenance {
                source: "/proc/cpuinfo".to_string(),
                confidence: Confidence::Queried,
            },
        )]),
        unresolved: vec![],
    };
    let s = to_yaml_string(&discovery).unwrap();
    assert_eq!(discovery, from_yaml_str::<Discovery>(&s).unwrap());

    // 3. Inventory
    let inventory = Inventory {
        schema: Inventory::SCHEMA.to_string(),
        machines: BTreeMap::from([(
            "local".to_string(),
            MachineEntry {
                connect: MachineConnect {
                    connect_type: ConnectType::Local,
                    host: None,
                    user: None,
                    partitions: vec![],
                    account: None,
                },
                fingerprint: Some("abcdef0123456789".to_string()),
                tags: vec!["dev".to_string()],
                node_classes: BTreeMap::new(),
                node: None,
                network: None,
                reference: None,
                workdir: Some("/tmp/calo".to_string()),
                modules: vec![],
                env: BTreeMap::new(),
                launcher: None,
                probe: None,
                policy: None,
                discovered: None,
            },
        )]),
    };
    let s = to_yaml_string(&inventory).unwrap();
    assert_eq!(inventory, from_yaml_str::<Inventory>(&s).unwrap());

    // 4. Suite
    let suite = Suite {
        schema: Suite::SCHEMA.to_string(),
        include: vec![],
        suite: SuiteMeta {
            name: "cosmolattice".to_string(),
            version: Some("0.1.0".to_string()),
            description: Some("CosmoLattice reference suite".to_string()),
        },
        context: vec![],
        sources: BTreeMap::new(),
        patches: None,
        builds: BTreeMap::new(),
        steps: BTreeMap::new(),
        benchmarks: vec![],
        services: BTreeMap::new(),
        symbols: None,
        reports: vec![],
        options: None,
    };
    let s = to_yaml_string(&suite).unwrap();
    assert_eq!(suite, from_yaml_str::<Suite>(&s).unwrap());

    // 5. Manifest
    let manifest = Manifest {
        schema: Manifest::SCHEMA.to_string(),
        suite: ManifestSuite {
            name: "cosmolattice".to_string(),
            version: Some("0.1.0".to_string()),
            hash: "1234567890abcdef".to_string(),
        },
        created: "2026-11-15T20:00:00Z".to_string(),
        calo: "0.1.0".to_string(),
        resolved: BTreeMap::new(),
        inventory_snapshot: inventory,
        runs: vec![ManifestRun {
            run_id: "b.r0".to_string(),
            benchmark: "b".to_string(),
            mode: Mode::Measure,
            machine: "local".to_string(),
            params: BTreeMap::from([("N".to_string(), ScalarValue::Int(128))]),
            status: Status::Ok,
            linked_runs: vec![],
            record: Some("records/b.r0/run.yaml".to_string()),
        }],
        sources: BTreeMap::new(),
    };
    let s = to_yaml_string(&manifest).unwrap();
    assert_eq!(manifest, from_yaml_str::<Manifest>(&s).unwrap());

    // 6. TableHeader
    let table_header = TableHeader {
        schema: TableHeader::SCHEMA.to_string(),
        table: "gpu.kernels".to_string(),
        columns: vec![
            ColumnDef {
                name: "start_ns".to_string(),
                col_type: ColumnType::Int64,
            },
            ColumnDef {
                name: "name_id".to_string(),
                col_type: ColumnType::StringId,
            },
        ],
        strings: BTreeMap::from([(
            "name_id".to_string(),
            vec!["void fft_kernel<double>()".to_string()],
        )]),
        meta: BTreeMap::from([("device".to_string(), ScalarValue::Int(0))]),
    };
    let s = to_yaml_string(&table_header).unwrap();
    assert_eq!(table_header, from_yaml_str::<TableHeader>(&s).unwrap());

    // Check WarningCode & Tier serialization
    let w = Warning {
        code: WarningCode::HardwareMismatch,
        message: "cores differed".to_string(),
    };
    let wy = calo_schema::yaml::to_string(&w).unwrap();
    assert!(wy.contains("hardware_mismatch"));
    assert_eq!(Tier::G2.to_string(), "G2");
}
