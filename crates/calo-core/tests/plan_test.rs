use calo_core::plan::{
    estimate_cost, expand_suite, format_run_id, matches, PlanError, RunDescriptor, Selector,
};
use calo_schema::common::{CpuTier, GpuTier, Mode, MpiTier, ScalarValue, Tier, TiersActive};
use calo_schema::from_yaml_str;
use calo_schema::inventory::Inventory;
use calo_schema::suite::{SelectorClause, Suite};
use std::collections::BTreeMap;

fn test_inventory() -> Inventory {
    let yaml = r#"
schema: calo.inventory/1
machines:
  hpc-a:
    connect:
      type: slurm
      host: login.hpc
      partitions: [gpu]
    node_classes:
      gpu-a100:
        count: 32
        partition: gpu
        node:
          cpu:
            cores: 64
          gpu:
            vendor: nvidia
            model: A100
            arch: sm_80
            per_node: 4
            mem_gb: 80
"#;
    from_yaml_str(yaml).unwrap()
}

#[test]
fn example_1_baseline_pinned_no_axes() {
    let inv = test_inventory();
    let suite_yaml = r#"
schema: calo.suite/1
suite:
  name: ex1
steps:
  run:
    run: ./app
    measured: true
    timeout: "01:00:00"
benchmarks:
  - name: baseline
    machine: hpc-a/gpu-a100
    steps: [run]
    params:
      N: 128
      patch: none
    repeat: 2
    warmup: 1
"#;
    let suite: Suite = from_yaml_str(suite_yaml).unwrap();
    let runs = expand_suite(&suite, &inv).unwrap();
    // 1 warmup + 2 repeats = 3 runs
    assert_eq!(runs.len(), 3);
    assert!(runs[0].warmup);
    assert_eq!(runs[0].run_id, "baseline.N_128.patch_none.w0");
    assert!(!runs[1].warmup);
    assert_eq!(runs[1].run_id, "baseline.N_128.patch_none.r0");
    assert!(!runs[2].warmup);
    assert_eq!(runs[2].run_id, "baseline.N_128.patch_none.r1");
}

#[test]
fn example_2_3x4x2_all_with_include_and_exclude() {
    let inv = test_inventory();
    // 3 * 4 * 2 = 24 base combinations
    // + 1 extra include point (N=512, nodes=8, patch=fast) -> 25
    // - exclude subset pattern `{nodes: 1, patch: fast}` (removes 3 runs for N in [64, 128, 256]) -> 22
    let suite_yaml = r#"
schema: calo.suite/1
suite:
  name: ex2
benchmarks:
  - name: sweep
    machine: hpc-a/gpu-a100
    axes:
      N: [64, 128, 256]
      nodes: [1, 2, 4, 8]
      patch: [none, fast]
    expand: all
    include:
      - {N: 512, nodes: 8, patch: fast}
    exclude:
      - {nodes: 1, patch: fast}
"#;
    let suite: Suite = from_yaml_str(suite_yaml).unwrap();
    let runs = expand_suite(&suite, &inv).unwrap();
    assert_eq!(runs.len(), 22);

    // Ensure `{nodes: 1, patch: fast}` is completely excluded
    for r in &runs {
        let is_excluded = r.params.get("nodes") == Some(&ScalarValue::Int(1))
            && r.params.get("patch") == Some(&ScalarValue::Str("fast".to_string()));
        assert!(!is_excluded, "excluded pattern found in {}", r.run_id);
    }

    // Ensure `{N: 512, nodes: 8, patch: fast}` was included
    assert!(runs.iter().any(|r| {
        r.params.get("N") == Some(&ScalarValue::Int(512))
            && r.params.get("nodes") == Some(&ScalarValue::Int(8))
            && r.params.get("patch") == Some(&ScalarValue::Str("fast".to_string()))
    }));
}

#[test]
fn example_3_defaults_with_partial_include() {
    let inv = test_inventory();
    // `expand: defaults` starts with 1 run at defaults (N=128, nodes=1, patch=none)
    // Partial include `{patch: fast}` fills unnamed axes (`N`, `nodes`) with their defaults -> 2 runs total
    let suite_yaml = r#"
schema: calo.suite/1
suite:
  name: ex3
benchmarks:
  - name: ablate
    machine: hpc-a/gpu-a100
    axes:
      N: [64, 128, 256]
      nodes: [1, 2, 4]
      patch: [none, fast]
    defaults:
      N: 128
      nodes: 1
      patch: none
    expand: defaults
    include:
      - {patch: fast}
"#;
    let suite: Suite = from_yaml_str(suite_yaml).unwrap();
    let runs = expand_suite(&suite, &inv).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].run_id, "ablate.N_128.nodes_1.patch_none.r0");
    assert_eq!(runs[1].run_id, "ablate.N_128.nodes_1.patch_fast.r0");
}

#[test]
fn weak_scaling_derive_and_from_where_derivation() {
    let inv = test_inventory();
    let suite_yaml = r#"
schema: calo.suite/1
suite:
  name: weak_suite
steps:
  run:
    run: ./cosmo
    measured: true
    resources:
      nodes: "${{ params.nodes }}"
      ranks_per_node: 4
      gpus_per_rank: 1
    timeout: "00:30:00"
benchmarks:
  - name: weak
    machine: hpc-a/gpu-a100
    steps: [run]
    axes:
      nodes: [1, 8, 64]
    derive:
      ranks: "nodes * 4"
      N: "128 * cbrt(ranks / 4)"
  - name: weak_profile
    mode: profile
    machine: hpc-a/gpu-a100
    steps: [run]
    from: weak
    where: "params.nodes <= 8 and machine.node.gpu.per_node >= 4"
"#;
    let suite: Suite = from_yaml_str(suite_yaml).unwrap();
    let runs = expand_suite(&suite, &inv).unwrap();

    // 3 measure runs + 2 profile runs (for nodes=1 and nodes=8) = 5 runs
    assert_eq!(runs.len(), 5);

    // Check derived `N` for nodes=1 (ranks=4 -> N=128), nodes=8 (ranks=32 -> N=256), nodes=64 (ranks=256 -> N=512)
    assert_eq!(runs[0].params.get("N"), Some(&ScalarValue::Float(128.0)));
    assert_eq!(runs[1].params.get("N"), Some(&ScalarValue::Float(256.0)));
    assert_eq!(runs[2].params.get("N"), Some(&ScalarValue::Float(512.0)));

    // Check `weak_profile` linked runs and mode
    assert_eq!(runs[3].benchmark, "weak_profile");
    assert_eq!(runs[3].mode, Mode::Profile);
    assert_eq!(
        runs[3].linked_from.as_deref(),
        Some("weak.N_128.nodes_1.ranks_4.r0")
    );
    assert_eq!(
        runs[4].linked_from.as_deref(),
        Some("weak.N_256.nodes_8.ranks_32.r0")
    );

    // Check cost estimation:
    // Measure: (1 + 8 + 64 = 73 nodes) * 0.5h = 36.5 node-hours
    // Profile: (1 + 8 = 9 nodes) * 0.5h = 4.5 node-hours
    // Total node-hours = 41.0; GPUs = 4 per node -> 164.0 GPU-hours
    let cost = estimate_cost(&runs, &suite, &inv).unwrap();
    assert_eq!(cost.runs, 5);
    assert!((cost.node_hours - 41.0).abs() < 1e-9);
    assert!((cost.gpu_hours - 164.0).abs() < 1e-9);
}

#[test]
fn derive_cycle_returns_error() {
    let inv = test_inventory();
    let suite_yaml = r#"
schema: calo.suite/1
suite:
  name: cycle_suite
benchmarks:
  - name: bad
    machine: hpc-a/gpu-a100
    derive:
      a: "b + 1"
      b: "a * 2"
"#;
    let suite: Suite = from_yaml_str(suite_yaml).unwrap();
    let err = expand_suite(&suite, &inv).expect_err("cycle must fail");
    assert!(matches!(err, PlanError::DeriveCycle { .. }));
}

#[test]
fn expand_none_with_and_without_include_and_deterministic_ids() {
    let inv = test_inventory();
    let suite_empty_none = r#"
schema: calo.suite/1
suite:
  name: none_empty
benchmarks:
  - name: b
    machine: hpc-a/gpu-a100
    axes:
      z_axis: [1, 2]
      a_axis: [10, 20]
    expand: none
"#;
    let s1: Suite = from_yaml_str(suite_empty_none).unwrap();
    let r1 = expand_suite(&s1, &inv).unwrap();
    assert!(r1.is_empty());

    // Partial include under `expand: none` without default is an error
    let suite_bad_none = r#"
schema: calo.suite/1
suite:
  name: none_bad
benchmarks:
  - name: b
    machine: hpc-a/gpu-a100
    axes:
      z_axis: [1, 2]
      a_axis: [10, 20]
    expand: none
    include:
      - {z_axis: 1}
"#;
    let s2: Suite = from_yaml_str(suite_bad_none).unwrap();
    assert!(matches!(
        expand_suite(&s2, &inv),
        Err(PlanError::IncompleteIncludeForNone { .. })
    ));

    // Complete include under `expand: none` produces deterministic sorted IDs regardless of key order
    let suite_good_none = r#"
schema: calo.suite/1
suite:
  name: none_good
benchmarks:
  - name: b
    machine: hpc-a/gpu-a100
    axes:
      z_axis: [1, 2]
      a_axis: [10, 20]
    expand: none
    include:
      - {z_axis: 2, a_axis: 10}
"#;
    let s3: Suite = from_yaml_str(suite_good_none).unwrap();
    let r3 = expand_suite(&s3, &inv).unwrap();
    assert_eq!(r3.len(), 1);
    assert_eq!(r3[0].run_id, "b.a_axis_10.z_axis_2.r0");

    // Verify truncation beyond 120 chars
    let mut long_params = BTreeMap::new();
    long_params.insert(
        "very_long_parameter_name_alpha".to_string(),
        ScalarValue::Str("extremely_long_value_string_00000000000000000000000000".to_string()),
    );
    long_params.insert(
        "very_long_parameter_name_beta".to_string(),
        ScalarValue::Str("another_extremely_long_value_string_111111111111111111".to_string()),
    );
    let long_id = format_run_id("benchmark_with_long_name", &long_params, 0, false);
    assert_eq!(long_id.len(), 120);
}

#[test]
fn selector_intersection_union_and_glob() {
    let inv = test_inventory();
    let (mach, node) =
        calo_core::plan::selector::resolve_machine_node(&inv, "hpc-a/gpu-a100").unwrap();
    let params = BTreeMap::from([
        ("N".to_string(), ScalarValue::Int(256)),
        ("patch".to_string(), ScalarValue::Str("fast-fft".to_string())),
    ]);
    let tiers = TiersActive {
        cpu: Some(CpuTier::T1),
        mpi: Some(MpiTier::M1),
        gpu: Some(GpuTier::G0),
    };
    let desc = RunDescriptor {
        benchmark: "scaling_strong",
        group: Some("scaling"),
        mode: Mode::Profile,
        params: &params,
        tiers: &tiers,
        machine: Some(mach),
        node,
    };

    let sel = Selector::Clause(SelectorClause {
        benchmark: Some("scaling*".to_string()),
        group: Some("scaling".to_string()),
        params: BTreeMap::from([(
            "patch".to_string(),
            ScalarValue::Str("fast-fft".to_string()),
        )]),
        where_expr: Some("params.N >= 128 and machine.node.gpu.per_node >= 4".to_string()),
        mode: Some(Mode::Profile),
        tier: vec![Tier::T1, Tier::M1],
    });
    assert!(matches(&sel, &desc));

    // Requiring G1 fails because only G0 is active
    let sel_fail = Selector::Clause(SelectorClause {
        tier: vec![Tier::G1],
        ..SelectorClause::default()
    });
    assert!(!matches(&sel_fail, &desc));

    // Union of [sel_fail, sel] succeeds
    let union_sel = Selector::Union(vec![
        SelectorClause {
            tier: vec![Tier::G1],
            ..SelectorClause::default()
        },
        SelectorClause {
            benchmark: Some("scaling*".to_string()),
            ..SelectorClause::default()
        },
    ]);
    assert!(matches(&union_sel, &desc));
}
