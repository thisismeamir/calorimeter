use calo_core::plan::Selector;
use calo_core::report::{
    build_pivot_table, evaluate_gate, evaluate_report_gates, select_records,
};
use calo_schema::common::{CpuTier, Mode, ScalarValue, Status, TiersActive};
use calo_schema::discovery::ObservedHardware;
use calo_schema::record::{
    ExitInfo, Record, RecordEnvironment, RecordIdentity, RecordObservations, RecordProvenance,
    RecordScalars, ReportedMetric,
};
use calo_schema::suite::{ReportGate, ReportTable, SelectorClause};
use calo_schema::SchemaDoc;
use std::collections::BTreeMap;

fn make_record(
    benchmark: &str,
    patch: &str,
    n: i64,
    wall_s: f64,
    throughput: f64,
) -> Record {
    Record {
        schema: Record::SCHEMA.to_string(),
        identity: RecordIdentity {
            run_id: format!("{benchmark}.N_{n}.patch_{patch}.r0"),
            benchmark: benchmark.to_string(),
            group: Some("scaling".to_string()),
            mode: Mode::Measure,
            params: BTreeMap::from([
                ("N".to_string(), ScalarValue::Int(n)),
                ("patch".to_string(), ScalarValue::Str(patch.to_string())),
            ]),
            repeat: Some(0),
            warmup: Some(false),
            rank: Some(0),
            ranks: Some(1),
            node: Some("node0".to_string()),
            local_rank: Some(0),
            suite_hash: None,
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
        observations: RecordObservations {
            scalars: Some(RecordScalars {
                wall_s: Some(wall_s),
                ..RecordScalars::default()
            }),
            reported: Some(BTreeMap::from([(
                "throughput".to_string(),
                ReportedMetric {
                    value: throughput,
                    unit: Some("sites/s".to_string()),
                },
            )])),
            ..RecordObservations::default()
        },
        provenance: RecordProvenance {
            probe_version: "0.1.0".to_string(),
            shim_versions: None,
            counters: vec![],
            clock: None,
            callgraph: None,
            start: "2026-11-16T00:00:00Z".to_string(),
            end: "2026-11-16T00:00:10Z".to_string(),
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
fn pivot_correctness_and_missing_cell_handling() {
    // Grid:
    // patch="none": N=64 (wall_s=10.0), N=128 (wall_s=20.0), N=256 (wall_s=40.0)
    // patch="fast": N=64 (wall_s=5.0),  N=128 (missing!),    N=256 (wall_s=10.0)
    let records = vec![
        make_record("sweep", "none", 64, 10.0, 100.0),
        make_record("sweep", "none", 128, 20.0, 90.0),
        make_record("sweep", "none", 256, 40.0, 80.0),
        make_record("sweep", "fast", 64, 5.0, 200.0),
        make_record("sweep", "fast", 256, 10.0, 160.0),
    ];

    let selected = select_records(&records, &Selector::All);
    assert_eq!(selected.len(), 5);

    let spec = ReportTable {
        rows: "params.patch".to_string(),
        cols: "params.N".to_string(),
        value: "wall_s.median".to_string(),
    };

    let pivot = build_pivot_table(&selected, &spec);

    // Numeric sorting on `params.N` -> ["64", "128", "256"]
    assert_eq!(pivot.col_keys, vec!["64", "128", "256"]);
    assert_eq!(pivot.row_keys, vec!["fast", "none"]);

    assert_eq!(pivot.get("none", "64"), Some(10.0));
    assert_eq!(pivot.get("none", "128"), Some(20.0));
    assert_eq!(pivot.get("none", "256"), Some(40.0));
    assert_eq!(pivot.get("fast", "64"), Some(5.0));
    assert_eq!(pivot.get("fast", "128"), None); // Missing cell!
    assert_eq!(pivot.get("fast", "256"), Some(10.0));

    let csv = pivot.to_csv();
    assert!(csv.contains("params.patch \\ params.N,64,128,256"));
    // Missing cell in fast row is empty between commas: `fast,5.00,,10.00`
    assert!(csv.contains("fast,5.00,,10.00"));
    assert!(csv.contains("none,10.00,20.00,40.00"));

    let md = pivot.to_markdown();
    assert!(md.contains("| params.patch \\ params.N | 64 | 128 | 256 |"));
    // Missing cell in Markdown is `-`
    assert!(md.contains("| fast | 5.00 | - | 10.00 |"));
    assert!(md.contains("| none | 10.00 | 20.00 | 40.00 |"));
}

#[test]
fn gate_evaluation_all_fail_if_forms() {
    let baseline_recs = vec![
        make_record("baseline", "none", 128, 10.0, 1000.0),
        make_record("baseline", "none", 128, 10.0, 1000.0),
    ];
    let candidate_recs = vec![
        // wall_s = 10.8 (+8% slower than 10.0), throughput = 850.0 (-15% lower than 1000.0)
        make_record("candidate", "fast", 128, 10.8, 850.0),
    ];

    let mut all_recs = baseline_recs.clone();
    all_recs.extend(candidate_recs.clone());

    let obs_refs: Vec<&Record> = candidate_recs.iter().collect();
    let base_refs: Vec<&Record> = baseline_recs.iter().collect();

    // Form 1: `"> +5%"` on `wall_s.median` (observed 10.8 vs baseline 10.0 is +8% -> fails!)
    let g_slow = ReportGate {
        metric: "wall_s.median".to_string(),
        compare: Some(SelectorClause {
            benchmark: Some("baseline".to_string()),
            ..SelectorClause::default()
        }),
        fail_if: "> +5%".to_string(),
    };
    let out1 = evaluate_gate(&g_slow, &obs_refs, &base_refs).unwrap();
    assert!(!out1.passed);
    assert!((out1.observed - 10.8).abs() < 1e-9);
    assert!((out1.baseline - 10.0).abs() < 1e-9);

    // Same form `"> +10%"` passes because +8% <= +10%
    let g_ok = ReportGate {
        metric: "wall_s.median".to_string(),
        compare: None,
        fail_if: "> +10%".to_string(),
    };
    let out2 = evaluate_gate(&g_ok, &obs_refs, &base_refs).unwrap();
    assert!(out2.passed);

    // Form 2: `"< -10%"` on `reported.throughput` (observed 850 vs baseline 1000 is -15% < -10% -> fails!)
    let g_tput = ReportGate {
        metric: "reported.throughput".to_string(),
        compare: None,
        fail_if: "< -10%".to_string(),
    };
    let out3 = evaluate_gate(&g_tput, &obs_refs, &base_refs).unwrap();
    assert!(!out3.passed);

    // Form 3: Expression `fail_if` (`"observed > baseline * 1.05"`)
    let g_expr = ReportGate {
        metric: "wall_s.median".to_string(),
        compare: Some(SelectorClause {
            benchmark: Some("baseline".to_string()),
            ..SelectorClause::default()
        }),
        fail_if: "observed > baseline * 1.05".to_string(),
    };
    let report_sel = Selector::Clause(SelectorClause {
        benchmark: Some("candidate".to_string()),
        ..SelectorClause::default()
    });
    let outcomes =
        evaluate_report_gates(&report_sel, &[g_expr], &all_recs, None).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].passed);
}
