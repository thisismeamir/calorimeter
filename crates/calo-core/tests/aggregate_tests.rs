use calo_core::aggregate::aggregate_run;
use calo_schema::common::{CpuTier, GpuTier, Mode, MpiTier, Status, TiersActive, Warning, WarningCode};
use calo_schema::discovery::ObservedHardware;
use calo_schema::record::{
    ExitInfo, PhaseSpan, Record, RecordEnvironment, RecordIdentity, RecordObservations,
    RecordProvenance, RecordScalars, RegionNode, Regions, ThreadRegions,
};
use calo_schema::{to_yaml_string, SchemaDoc};
use std::collections::BTreeMap;

fn rank_record(rank: u32, wall_s: f64, cpu_tier: CpuTier, status: Status) -> Record {
    Record {
        schema: Record::SCHEMA.to_string(),
        identity: RecordIdentity {
            run_id: "strong.r0".to_string(),
            benchmark: "strong".to_string(),
            group: None,
            mode: Mode::Measure,
            params: BTreeMap::new(),
            repeat: Some(0),
            warmup: Some(false),
            rank: Some(rank),
            ranks: Some(4),
            node: Some(format!("node{}", rank / 2)),
            local_rank: Some(rank % 2),
            suite_hash: Some("abcd".to_string()),
            linked_runs: vec![],
            status,
        },
        environment: RecordEnvironment {
            machine: "hpc".to_string(),
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
            phases: Some(BTreeMap::from([(
                "zero_phase".to_string(),
                PhaseSpan {
                    start_ns: 100,
                    end_ns: 100,
                },
            )])),
            regions: if rank == 2 {
                // Missing region block on rank 2
                None
            } else {
                Some(Regions {
                    threads: vec![ThreadRegions {
                        tid: 1,
                        tree: vec![RegionNode {
                            name: "solve".to_string(),
                            calls: 10,
                            incl_s: wall_s * 0.5,
                            excl_s: wall_s * 0.5,
                            children: vec![],
                        }],
                    }],
                })
            },
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
                cpu: Some(cpu_tier),
                mpi: Some(MpiTier::M0),
                gpu: Some(GpuTier::G0),
            },
            warnings: if rank == 0 {
                vec![Warning {
                    code: WarningCode::MuxDetected,
                    message: "mux".to_string(),
                }]
            } else {
                vec![]
            },
            errors: vec![],
        },
        aggregate: None,
    }
}

#[test]
fn aggregate_4_ranks_with_straggler_missing_region_mixed_tiers_and_zero_mean_guard() {
    // Ranks: wall_s = [8.0, 8.0, 8.0, 16.0] -> mean = 10.0, min = 8.0, max = 16.0, imbalance = 1.6
    // Rank 3 has degraded tier T0 (others T2) -> min tier is T0 + TierDegraded warning
    // Rank 2 is missing `regions` -> region "solve" aggregated over ranks 0, 1, 3 ([4.0, 4.0, 8.0])
    // `phase.zero_phase` has duration 0.0 -> mean = 0.0 -> imbalance is None (omitted in YAML)
    let recs = vec![
        rank_record(0, 8.0, CpuTier::T2, Status::Ok),
        rank_record(1, 8.0, CpuTier::T2, Status::Ok),
        rank_record(2, 8.0, CpuTier::T2, Status::Ok),
        rank_record(3, 16.0, CpuTier::T0, Status::Ok),
    ];

    let agg = aggregate_run(&recs).unwrap();
    assert_eq!(agg.identity.rank, None);
    assert_eq!(agg.identity.ranks, Some(4));
    assert_eq!(agg.identity.status, Status::Ok);
    assert_eq!(agg.provenance.tiers_active.cpu, Some(CpuTier::T0));
    assert!(agg
        .provenance
        .warnings
        .iter()
        .any(|w| w.code == WarningCode::TierDegraded));

    let block = agg.aggregate.as_ref().unwrap();
    assert_eq!(
        block.rank_records,
        vec!["rank0.yaml", "rank1.yaml", "rank2.yaml", "rank3.yaml"]
    );

    let wall = &block.scalars["wall_s"];
    assert!((wall.min - 8.0).abs() < 1e-9);
    assert!((wall.mean - 10.0).abs() < 1e-9);
    assert!((wall.max - 16.0).abs() < 1e-9);
    assert!((wall.imbalance.unwrap() - 1.6).abs() < 1e-9);

    let solve = &block.regions["solve"];
    assert!((solve.min - 4.0).abs() < 1e-9);
    assert!((solve.max - 8.0).abs() < 1e-9);

    let zero_p = &block.scalars["phase.zero_phase"];
    assert_eq!(zero_p.mean, 0.0);
    assert_eq!(zero_p.imbalance, None);
    let yaml_out = to_yaml_string(&agg).unwrap();
    assert!(!yaml_out.contains("NaN"));
    assert!(!yaml_out.contains("inf"));

    // Failed rank propagation
    let mut failed_recs = recs;
    failed_recs[1].identity.status = Status::Failed;
    let failed_agg = aggregate_run(&failed_recs).unwrap();
    assert_eq!(failed_agg.identity.status, Status::Failed);
}
