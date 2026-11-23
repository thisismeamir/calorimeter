use calo_core::report::{diff_results_dirs, generate_report_artifacts, run_analysis_pipeline};
use calo_core::results::write_atomic_bytes;
use calo_schema::common::{CpuTier, Mode, ScalarValue, Status, TiersActive};
use calo_schema::discovery::ObservedHardware;
use calo_schema::record::{
    ExitInfo, Record, RecordEnvironment, RecordIdentity, RecordObservations, RecordProvenance,
    RecordScalars,
};
use calo_schema::suite::{
    ReportDef, ReportGate, ReportOutputFormat, ReportPlot, ReportTable, SelectorClause, Suite,
    SuiteMeta,
};
use calo_schema::{to_yaml_string, SchemaDoc};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("calo_pipe_{tag}_{nanos}"));
    fs::create_dir_all(&p).unwrap();
    p
}

fn mk_rec(run_id: &str, bench: &str, n: i64, wall_s: f64, probe_ver: &str) -> Record {
    Record {
        schema: Record::SCHEMA.to_string(),
        identity: RecordIdentity {
            run_id: run_id.to_string(),
            benchmark: bench.to_string(),
            group: None,
            mode: Mode::Measure,
            params: BTreeMap::from([("N".to_string(), ScalarValue::Int(n))]),
            repeat: Some(0),
            warmup: Some(false),
            rank: None,
            ranks: Some(1),
            node: Some("n0".to_string()),
            local_rank: None,
            suite_hash: Some("deadbeef".to_string()),
            linked_runs: vec![],
            status: Status::Ok,
        },
        environment: RecordEnvironment {
            machine: "local".to_string(),
            node_class: None,
            fingerprint: Some("fp1234".to_string()),
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
            ..RecordObservations::default()
        },
        provenance: RecordProvenance {
            probe_version: probe_ver.to_string(),
            shim_versions: None,
            counters: vec![],
            clock: None,
            callgraph: None,
            start: "2026-11-16T00:00:00Z".to_string(),
            end: "2026-11-16T00:00:01Z".to_string(),
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
fn end_to_end_analyze_report_gates_context_and_diff() {
    let dir = temp_dir("e2e");
    let dir_a = dir.join("res_a");
    let dir_b = dir.join("res_b");

    let recs_a = vec![
        mk_rec("b.N_64.r0", "b", 64, 10.0, "0.1.0"),
        mk_rec("b.N_64.r1", "b", 64, 10.0, "0.1.0"),
    ];
    let recs_b = vec![
        mk_rec("b.N_64.r0", "b", 64, 12.0, "0.1.0"),
        mk_rec("b.N_64.r1", "b", 64, 12.0, "0.1.0"),
    ];

    for r in &recs_a {
        let p = dir_a.join("records").join(&r.identity.run_id).join("run.yaml");
        write_atomic_bytes(&p, to_yaml_string(r).unwrap().as_bytes()).unwrap();
    }
    for r in &recs_b {
        let p = dir_b.join("records").join(&r.identity.run_id).join("run.yaml");
        write_atomic_bytes(&p, to_yaml_string(r).unwrap().as_bytes()).unwrap();
    }

    fs::write(dir.join("notes.txt"), "Experiment context verbatim").unwrap();

    let suite = Suite {
        schema: Suite::SCHEMA.to_string(),
        include: vec![],
        suite: SuiteMeta {
            name: "s".to_string(),
            version: None,
            description: None,
        },
        context: vec!["notes.txt".to_string()],
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

    let an_out = run_analysis_pipeline(&dir_a, &suite, &recs_a, None).unwrap();
    assert!(!an_out.written_tables.is_empty());
    assert!(an_out
        .notices
        .iter()
        .any(|n| n.contains("symbolization) skipped")));

    let rep_def = ReportDef {
        name: "summary".to_string(),
        requires: vec![],
        select: None,
        plots: vec![ReportPlot {
            plot_type: "line".to_string(),
            x: Some("params.N".to_string()),
            y: Some("wall_s".to_string()),
            line_by: None,
            facet_by: None,
            scale: None,
            errorbars: None,
            title: Some("Wall Time".to_string()),
        }],
        tables: vec![ReportTable {
            rows: "benchmark".to_string(),
            cols: "params.N".to_string(),
            value: "wall_s.median".to_string(),
        }],
        gates: vec![ReportGate {
            metric: "wall_s.median".to_string(),
            compare: Some(SelectorClause {
                benchmark: Some("b".to_string()),
                ..SelectorClause::default()
            }),
            fail_if: "> +5%".to_string(),
        }],
        output: vec![
            ReportOutputFormat::Md,
            ReportOutputFormat::Html,
            ReportOutputFormat::Csv,
            ReportOutputFormat::Svg,
        ],
    };

    // Compare dir_b (12.0s) against baseline dir_a (10.0s) -> +20% > +5% -> gate fails
    let gen = generate_report_artifacts(
        &dir_b,
        &dir,
        &suite,
        "deadbeef",
        &rep_def,
        &recs_b,
        Some(&recs_a),
    )
    .unwrap();
    assert!(!gen.gates[0].passed);
    let html = fs::read_to_string(dir_b.join("reports/summary/report.html")).unwrap();
    assert!(html.contains("Experiment context verbatim"));
    assert!(!html.contains("<script"));

    // Diff detects +20% change
    let diff_md = diff_results_dirs(&dir_a, &dir_b, "wall_s", false).unwrap();
    assert!(diff_md.contains("+20.00%"));

    // Mixed probe majors refused unless --allow-mixed
    let dir_c = dir.join("res_c");
    let rec_c = mk_rec("b.N_64.r0", "b", 64, 12.0, "1.0.0");
    write_atomic_bytes(
        &dir_c.join("records/b.N_64.r0/run.yaml"),
        to_yaml_string(&rec_c).unwrap().as_bytes(),
    )
    .unwrap();
    assert!(diff_results_dirs(&dir_a, &dir_c, "wall_s", false).is_err());
    assert!(diff_results_dirs(&dir_a, &dir_c, "wall_s", true).is_ok());

    let _ = fs::remove_dir_all(&dir);
}
