use calo_core::exec::{
    build_launcher_argv, build_runspec, execute_suite_runs, order_runs, resolve_counter_aliases,
    subtract_isolated_cpus, ExecError, LocalAdapter,
};
use calo_core::plan::{expand_suite, PinnedRun};
use calo_core::proc::RealCommandRunner;
use calo_core::results::ResultsDir;
use calo_core::yaml::{Layer, LayeredBuilder};
use calo_schema::common::{Mode, Status};
use calo_schema::from_yaml_str;
use calo_schema::inventory::Inventory;
use calo_schema::manifest::Manifest;
use calo_schema::runspec::MpiTraceMode;
use calo_schema::suite::{RunOrder, Suite};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("calo_exec_{tag}_{nanos}"));
    fs::create_dir_all(&p).unwrap();
    p
}

fn test_inv() -> Inventory {
    from_yaml_str(
        r#"
schema: calo.inventory/1
machines:
  hpc:
    connect: { type: local }
    node:
      cpu: { cores: 64, threads: 64 }
    launcher:
      mpi: "srun -n {ranks} --ntasks-per-node {ranks_per_node}"
    probe:
      counters:
        cycles: "perf::cycles"
    policy:
      isolate: { cpu_list: "0-3" }
    env:
      FOO: bar
"#,
    )
    .unwrap()
}

#[test]
fn runspec_affinity_counters_launcher_and_measure_mode_rejection() {
    let inv = test_inv();
    let suite: Suite = from_yaml_str(
        r#"
schema: calo.suite/1
suite: { name: s }
steps:
  mpi_step:
    run: "./app"
    measured: true
    resources:
      nodes: 2
      ranks_per_node: 4
      threads_per_rank: 8
  serial_step:
    run: "./serial"
    measured: true
benchmarks:
  - name: mpi_bench
    mode: measure
    machine: hpc
    steps: [mpi_step]
    probe:
      counters: [cycles, instructions]
  - name: serial_bench
    mode: measure
    machine: hpc
    steps: [serial_step]
  - name: bad_measure
    mode: measure
    machine: hpc
    steps: [serial_step]
    probe:
      mpi: summary
"#,
    )
    .unwrap();

    // Affinity subtraction: 0-3 removed from 0..64 -> "4-63"
    assert_eq!(subtract_isolated_cpus(64, "0-3"), "4-63");

    // Counter alias resolution with inventory override
    let resolved_ctrs = resolve_counter_aliases(
        &["cycles".to_string(), "instructions".to_string()],
        &inv.machines["hpc"].probe.as_ref().unwrap().counters,
    );
    assert_eq!(resolved_ctrs, vec!["perf::cycles", "instructions"]);

    let runs = expand_suite(&suite, &inv).unwrap();

    // MPI runspec & launcher argv
    let rs_mpi = build_runspec(
        &runs[0],
        &suite,
        &inv,
        &BTreeMap::new(),
        Path::new("/tmp/out"),
        Path::new("/tmp"),
        None,
    )
    .unwrap();
    assert_eq!(
        rs_mpi.env.get("OMP_NUM_THREADS").map(String::as_str),
        Some("8")
    );
    assert_eq!(rs_mpi.env.get("FOO").map(String::as_str), Some("bar"));
    assert_eq!(
        rs_mpi.affinity.as_ref().unwrap().cpu_list.as_deref(),
        Some("4-63")
    );
    assert_eq!(rs_mpi.mpi.as_ref().unwrap().mode, MpiTraceMode::Off);

    let argv_mpi = build_launcher_argv(
        Path::new("/usr/bin/probe"),
        Path::new("/tmp/runspec.yaml"),
        &suite.steps["mpi_step"],
        &inv,
        "hpc",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        argv_mpi,
        vec![
            "srun",
            "-n",
            "8",
            "--ntasks-per-node",
            "4",
            "/usr/bin/probe",
            "--rank-aware",
            "--runspec",
            "/tmp/runspec.yaml"
        ]
    );

    // Serial argv (no resources)
    let argv_serial = build_launcher_argv(
        Path::new("/usr/bin/probe"),
        Path::new("/tmp/runspec.yaml"),
        &suite.steps["serial_step"],
        &inv,
        "hpc",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        argv_serial,
        vec!["/usr/bin/probe", "--runspec", "/tmp/runspec.yaml"]
    );

    // Measure mode rejects explicit shim/sampling
    let bad_err = build_runspec(
        &runs[2],
        &suite,
        &inv,
        &BTreeMap::new(),
        Path::new("/tmp/out"),
        Path::new("/tmp"),
        None,
    )
    .expect_err("measure mode must reject mpi: summary");
    assert!(matches!(bad_err, ExecError::InvalidModeConfig { .. }));
}

#[test]
fn interleaved_and_random_ordering_deterministic() {
    let runs: Vec<PinnedRun> = (0..2)
        .flat_map(|n| {
            (0..2).map(move |r| PinnedRun {
                run_id: format!("b.N_{n}.r{r}"),
                benchmark: "b".to_string(),
                group: None,
                mode: Mode::Measure,
                machine: "hpc".to_string(),
                params: BTreeMap::from([("N".to_string(), calo_schema::ScalarValue::Int(n))]),
                repeat: r,
                warmup: false,
                linked_from: None,
            })
        })
        .collect();

    let inter = order_runs(&runs, RunOrder::Interleaved, 1);
    let ids: Vec<&str> = inter.iter().map(|r| r.run_id.as_str()).collect();
    assert_eq!(ids, vec!["b.N_0.r0", "b.N_1.r0", "b.N_0.r1", "b.N_1.r1"]);

    let rand1 = order_runs(&runs, RunOrder::Random, 99);
    let rand2 = order_runs(&runs, RunOrder::Random, 99);
    assert_eq!(rand1, rand2);
}

#[test]
#[cfg(unix)]
fn fake_probe_resume_and_on_failure_stop() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("fake_probe");
    let inv = test_inv();
    let suite_val = calo_schema::yaml::from_str(
        r#"
schema: calo.suite/1
suite: { name: fake_run }
steps:
  s:
    run: "./noop"
    measured: true
benchmarks:
  - name: b1
    mode: measure
    machine: hpc
    steps: [s]
    axes:
      N: [1, 2, 3]
options:
  order: sequential
  on_failure: stop
"#,
    )
    .unwrap();
    let mut lb = LayeredBuilder::new();
    lb.add_layer(Layer::Suite, suite_val);
    let layered = lb.resolve::<Suite>().unwrap();

    let results = ResultsDir::create(&dir, &layered.value, &inv).unwrap();
    let runs = expand_suite(&layered.value, &inv).unwrap();
    results
        .write_manifest(&layered, &inv, &runs, BTreeMap::new(), "0.1.0")
        .unwrap();

    // Fake `probe` script: succeeds on N_1, fails on N_2 (which triggers `on_failure: stop` before N_3)
    let fake_probe = dir.join("fake_probe.sh");
    let suite_hash = results.suite_hash.clone();
    let script = format!(
        r#"#!/bin/sh
RUNSPEC=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--runspec" ]; then RUNSPEC="$2"; shift 2; else shift; fi
done
RUN_ID=$(grep "^run_id:" "$RUNSPEC" | awk '{{print $2}}')
OUTDIR=$(grep "dir:" "$RUNSPEC" | awk '{{print $2}}')
mkdir -p "$OUTDIR"
if echo "$RUN_ID" | grep -q "N_2"; then
  exit 1
fi
cat > "$OUTDIR/run.yaml" <<EOF
schema: calo.record/2
identity:
  run_id: $RUN_ID
  benchmark: b1
  mode: measure
  params:
    N: 1
  suite_hash: {suite_hash}
  status: ok
environment:
  machine: hpc
  observed:
    kernel: 6.6.0
provenance:
  probe_version: 0.1.0
  start: "2026-11-16T00:00:00Z"
  end: "2026-11-16T00:00:01Z"
  exit:
    code: 0
  tiers_active:
    cpu: T0
EOF
exit 0
"#
    );
    fs::write(&fake_probe, script).unwrap();
    let mut perms = fs::metadata(&fake_probe).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_probe, perms).unwrap();

    let adapter = LocalAdapter::new(Arc::new(RealCommandRunner));
    let sum = execute_suite_runs(
        &runs,
        &layered.value,
        &inv,
        &results,
        &dir,
        &fake_probe,
        &adapter,
        false,
        false,
        1,
    )
    .unwrap();

    assert_eq!(sum.succeeded, 1);
    assert_eq!(sum.failed, 1);

    let m: Manifest = from_yaml_str(&fs::read_to_string(results.manifest_path()).unwrap()).unwrap();
    assert_eq!(m.runs[1].status, Status::Failed);

    let _ = fs::remove_dir_all(&dir);
}
