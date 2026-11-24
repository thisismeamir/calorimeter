use calo_core::plan::PinnedRun;
use calo_core::results::{
    compute_suite_hash, is_valid_record, plan_resume, update_manifest_run, write_atomic_bytes,
    RecordValidity, ResultsDir, SidecarKind,
};
use calo_core::yaml::{Layer, LayeredBuilder};
use calo_schema::common::{CpuTier, Mode, ScalarValue, Status, TiersActive};
use calo_schema::discovery::ObservedHardware;
use calo_schema::inventory::{ConnectType, Inventory, MachineConnect, MachineEntry};
use calo_schema::manifest::Manifest;
use calo_schema::record::{
    ExitInfo, Record, RecordEnvironment, RecordIdentity, RecordObservations, RecordProvenance,
};
use calo_schema::suite::Suite;
use calo_schema::{from_yaml_str, to_yaml_string, SchemaDoc};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("calo_res_{tag}_{nanos}"));
    fs::create_dir_all(&p).unwrap();
    p
}

fn minimal_inventory() -> Inventory {
    Inventory {
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
                fingerprint: None,
                tags: vec![],
                node_classes: BTreeMap::new(),
                node: None,
                network: None,
                reference: None,
                workdir: None,
                modules: vec![],
                env: BTreeMap::new(),
                launcher: None,
                probe: None,
                policy: None,
                discovered: None,
            },
        )]),
    }
}

fn make_record(run_id: &str, benchmark: &str, suite_hash: &str, n: i64, schema: &str) -> Record {
    Record {
        schema: schema.to_string(),
        identity: RecordIdentity {
            run_id: run_id.to_string(),
            benchmark: benchmark.to_string(),
            group: None,
            mode: Mode::Measure,
            params: BTreeMap::from([("N".to_string(), ScalarValue::Int(n))]),
            repeat: Some(0),
            warmup: Some(false),
            rank: None,
            ranks: Some(1),
            node: Some("node0".to_string()),
            local_rank: None,
            suite_hash: Some(suite_hash.to_string()),
            linked_runs: vec![],
            status: Status::Ok,
        },
        environment: RecordEnvironment {
            machine: "local".to_string(),
            node_class: None,
            fingerprint: None,
            declared: BTreeMap::new(),
            observed: ObservedHardware::default(),
            placement: None,
            devices: vec![],
            revisions: BTreeMap::new(),
            patchset: None,
            build: None,
            artifact_hashes: BTreeMap::new(),
            rendered_files: BTreeMap::new(),
            mpi_lib: None,
            modules: vec![],
        },
        observations: RecordObservations::default(),
        provenance: RecordProvenance {
            probe_version: "0.1.0".to_string(),
            shim_versions: None,
            counters: vec![],
            clock: None,
            callgraph: None,
            start: "2026-11-16T00:00:00Z".to_string(),
            end: "2026-11-16T00:00:05Z".to_string(),
            start_ns: None,
            exit: ExitInfo {
                code: Some(0),
                signal: None,
            },
            sample_loss: None,
            trace_overflow: None,
            tiers_active: TiersActive {
                cpu: Some(CpuTier::T0),
                mpi: None,
                gpu: None,
            },
            warnings: vec![],
            errors: vec![],
        },
        aggregate: None,
    }
}

#[test]
fn stable_suite_hash_across_key_reordering() {
    let yaml_a = r#"
schema: calo.suite/1
suite:
  name: cosmo
  version: "0.1"
steps:
  b_step:
    run: ./b
  a_step:
    run: ./a
"#;
    let yaml_b = r#"
steps:
  a_step:
    run: ./a
  b_step:
    run: ./b
suite:
  version: "0.1"
  name: cosmo
schema: calo.suite/1
"#;
    let s1: Suite = from_yaml_str(yaml_a).unwrap();
    let s2: Suite = from_yaml_str(yaml_b).unwrap();
    assert_eq!(
        compute_suite_hash(&s1).unwrap(),
        compute_suite_hash(&s2).unwrap()
    );
}

#[test]
fn atomic_write_leaves_no_partial_file_on_simulated_failure() {
    let dir = temp_dir("atomic");
    let target = dir.join("manifest.yaml");
    write_atomic_bytes(&target, b"initial_ok").unwrap();

    // Attempting atomic write to a path whose target is a directory fails rename,
    // leaving no leftover `.tmp` files in the parent directory.
    let dir_target = dir.join("subdir");
    fs::create_dir_all(&dir_target).unwrap();
    let err = write_atomic_bytes(&dir_target, b"should_fail");
    assert!(err.is_err());

    let leftover_tmps: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftover_tmps.is_empty());
    assert_eq!(fs::read_to_string(&target).unwrap(), "initial_ok");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn record_validity_and_resume_planning() {
    let dir = temp_dir("resume");
    let inv = minimal_inventory();
    let suite: Suite = from_yaml_str("schema: calo.suite/1\nsuite:\n  name: s\n").unwrap();
    let res = ResultsDir::create(&dir, &suite, &inv).unwrap();

    // Verify subdirectories and path helpers
    assert!(res.root.join("records").is_dir());
    assert!(res.root.join("analysis").is_dir());
    assert_eq!(
        res.sidecar_dir(SidecarKind::Gpu, "run1"),
        res.root.join("gpu").join("run1")
    );

    let run_valid = PinnedRun {
        run_id: "b.N_64.r0".to_string(),
        benchmark: "b".to_string(),
        group: None,
        mode: Mode::Measure,
        machine: "local".to_string(),
        params: BTreeMap::from([("N".to_string(), ScalarValue::Int(64))]),
        repeat: 0,
        warmup: false,
        linked_from: None,
    };
    let run_stale = PinnedRun {
        run_id: "b.N_128.r0".to_string(),
        benchmark: "b".to_string(),
        group: None,
        mode: Mode::Measure,
        machine: "local".to_string(),
        params: BTreeMap::from([("N".to_string(), ScalarValue::Int(128))]),
        repeat: 0,
        warmup: false,
        linked_from: None,
    };
    let run_unsup = PinnedRun {
        run_id: "b.N_256.r0".to_string(),
        benchmark: "b".to_string(),
        group: None,
        mode: Mode::Measure,
        machine: "local".to_string(),
        params: BTreeMap::from([("N".to_string(), ScalarValue::Int(256))]),
        repeat: 0,
        warmup: false,
        linked_from: None,
    };
    let run_missing = PinnedRun {
        run_id: "b.N_512.r0".to_string(),
        benchmark: "b".to_string(),
        group: None,
        mode: Mode::Measure,
        machine: "local".to_string(),
        params: BTreeMap::from([("N".to_string(), ScalarValue::Int(512))]),
        repeat: 0,
        warmup: false,
        linked_from: None,
    };

    // Write valid record
    let p_valid = res.record_path(&run_valid.run_id, None);
    let rec_valid = make_record(&run_valid.run_id, "b", &res.suite_hash, 64, Record::SCHEMA);
    write_atomic_bytes(&p_valid, to_yaml_string(&rec_valid).unwrap().as_bytes()).unwrap();
    assert_eq!(
        is_valid_record(&p_valid, &run_valid, &res.suite_hash),
        RecordValidity::Valid
    );

    // Write stale record (mismatched suite_hash)
    let p_stale = res.record_path(&run_stale.run_id, None);
    let rec_stale = make_record(&run_stale.run_id, "b", "old_hash", 128, Record::SCHEMA);
    write_atomic_bytes(&p_stale, to_yaml_string(&rec_stale).unwrap().as_bytes()).unwrap();
    assert_eq!(
        is_valid_record(&p_stale, &run_stale, &res.suite_hash),
        RecordValidity::Stale
    );

    // Write unsupported schema record
    let p_unsup = res.record_path(&run_unsup.run_id, None);
    let rec_unsup_yaml = "schema: calo.record/99\nidentity:\n  run_id: b.N_256.r0\n";
    write_atomic_bytes(&p_unsup, rec_unsup_yaml.as_bytes()).unwrap();
    assert_eq!(
        is_valid_record(&p_unsup, &run_unsup, &res.suite_hash),
        RecordValidity::SchemaUnsupported
    );

    // Missing record
    let p_missing = res.record_path(&run_missing.run_id, None);
    assert_eq!(
        is_valid_record(&p_missing, &run_missing, &res.suite_hash),
        RecordValidity::Missing
    );

    let all_runs = vec![run_valid, run_stale, run_unsup, run_missing];
    let to_run = plan_resume(&all_runs, &res, false);
    assert_eq!(to_run.len(), 3); // skips `run_valid`
    assert!(!to_run.iter().any(|r| r.run_id == "b.N_64.r0"));

    // `--force` re-runs all 4
    let forced = plan_resume(&all_runs, &res, true);
    assert_eq!(forced.len(), 4);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn manifest_write_and_concurrent_updates() {
    let dir = temp_dir("manifest_conc");
    let inv = minimal_inventory();
    let suite_val =
        calo_schema::yaml::from_str("schema: calo.suite/1\nsuite:\n  name: s\n").unwrap();
    let mut lb = LayeredBuilder::new();
    lb.add_layer(Layer::Suite, suite_val);
    let layered = lb.resolve::<Suite>().unwrap();

    let res = ResultsDir::create(&dir, &layered.value, &inv).unwrap();
    let runs: Vec<PinnedRun> = (0..4)
        .map(|i| PinnedRun {
            run_id: format!("b.r{i}"),
            benchmark: "b".to_string(),
            group: None,
            mode: Mode::Measure,
            machine: "local".to_string(),
            params: BTreeMap::new(),
            repeat: i,
            warmup: false,
            linked_from: None,
        })
        .collect();

    res.write_manifest(&layered, &inv, &runs, BTreeMap::new(), "0.1.0")
        .unwrap();

    let shared_res = Arc::new(res.clone());
    let mut handles = Vec::new();
    for i in 0..4u32 {
        let r_clone = Arc::clone(&shared_res);
        handles.push(std::thread::spawn(move || {
            let status = if i == 2 { Status::Failed } else { Status::Ok };
            let rec = format!("records/b.r{i}/run.yaml");
            update_manifest_run(&r_clone, &format!("b.r{i}"), status, Some(&rec), &[]).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let raw_manifest = fs::read_to_string(res.manifest_path()).unwrap();
    let parsed: Manifest = from_yaml_str(&raw_manifest).unwrap();
    assert_eq!(parsed.runs.len(), 4);
    assert_eq!(parsed.runs[2].status, Status::Failed);
    assert_eq!(parsed.runs[0].status, Status::Ok);

    let _ = fs::remove_dir_all(&dir);
}
