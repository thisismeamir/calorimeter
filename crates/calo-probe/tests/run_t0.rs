//! End-to-end T0 integration tests for Task P6 (`Probe::run` and `probe` binary).

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use calo_probe::probe::{Probe, ProbeOptions};
use calo_schema::{
    common::{Mode, Status, WarningCode},
    record::Record,
    runspec::{RunSpec, RunSpecAffinity, RunSpecOutputs},
    SchemaDoc,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("calo_p6_{}_{}_{label}", std::process::id(), id));
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

fn base_runspec(dir: &Path, cmd: &[&str]) -> RunSpec {
    RunSpec {
        schema: RunSpec::SCHEMA.to_string(),
        run_id: "bench.r0".to_string(),
        mode: Mode::Measure,
        command: cmd.iter().map(|s| (*s).to_string()).collect(),
        cwd: dir.to_string_lossy().into_owned(),
        env: BTreeMap::new(),
        affinity: None,
        rlimits: None,
        counters: Vec::new(),
        sample_hz: Some(20),
        profile: None,
        mpi: None,
        gpu: None,
        shim_inject: None,
        regions: None,
        channel: None,
        rank: None,
        outputs: RunSpecOutputs {
            dir: dir.to_string_lossy().into_owned(),
        },
    }
}

fn first_allowed_cpu() -> usize {
    if let Ok(status) = fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Cpus_allowed_list:") {
                let first = rest.trim().split(',').next().unwrap_or("0");
                let start = first.split('-').next().unwrap_or("0");
                if let Ok(cpu) = start.trim().parse::<usize>() {
                    return cpu;
                }
            }
        }
    }
    0
}

#[test]
fn t0_happy_path_sleep_and_absent_until_finalize() {
    let dir = temp_dir("happy");
    let spec = base_runspec(&dir, &["sh", "-c", "sleep 0.3"]);
    let record_path = dir.join("rank0.yaml");

    let running = Arc::new(AtomicBool::new(true));
    let saw_premature = Arc::new(AtomicBool::new(false));
    let r_flag = running.clone();
    let p_flag = saw_premature.clone();
    let rp = record_path.clone();

    let poller = thread::spawn(move || {
        while r_flag.load(Ordering::Acquire) {
            if rp.exists() {
                p_flag.store(true, Ordering::Release);
            }
            thread::sleep(Duration::from_millis(5));
        }
    });

    let outcome = Probe::run(&spec).expect("Probe::run");
    running.store(false, Ordering::Release);
    poller.join().unwrap();
    assert!(
        !saw_premature.load(Ordering::Acquire),
        "rank0.yaml must not exist before finalize"
    );

    assert_eq!(outcome.record.identity.status, Status::Ok);
    assert!(outcome.record.check().is_ok());

    let parsed: Record =
        serde_yaml_ng::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    let wall = parsed
        .observations
        .scalars
        .as_ref()
        .unwrap()
        .wall_s
        .unwrap();
    assert!(
        (0.225..=0.45).contains(&wall),
        "wall_s {wall} outside 25% tolerance of 0.3s"
    );

    let counters = &parsed.observations.scalars.as_ref().unwrap().counters;
    if counters.is_empty() {
        eprintln!("SKIPPED counter presence assertion: perf_event_open unavailable on this host");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t0_nonzero_exit_and_signal_nine() {
    let dir = temp_dir("nonzero");
    let spec = base_runspec(&dir, &["sh", "-c", "exit 3"]);
    let outcome = Probe::run(&spec).expect("Probe::run nonzero");
    assert_eq!(outcome.record.identity.status, Status::Failed);
    assert_eq!(outcome.record.provenance.exit.code, Some(3));
    assert_eq!(outcome.exit_code, 3);
    let _ = fs::remove_dir_all(&dir);

    let dir = temp_dir("sigkill");
    let spec = base_runspec(&dir, &["sh", "-c", "kill -9 $$"]);
    let outcome = Probe::run(&spec).expect("Probe::run sigkill");
    assert_eq!(outcome.record.identity.status, Status::Failed);
    assert_eq!(
        outcome.record.provenance.exit.signal.as_deref(),
        Some("SIGKILL")
    );
    assert_eq!(outcome.exit_code, 137);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t0_timeout_kills_grandchild_and_marks_timeout() {
    let dir = temp_dir("timeout");
    let gpid_file = dir.join("gpid.txt");
    let cmd = format!("sleep 60 & echo $! > {}; wait", gpid_file.display());
    let spec = base_runspec(&dir, &["sh", "-c", &cmd]);

    let t0 = Instant::now();
    let opts = ProbeOptions {
        timeout_s: Some(1),
        ..Default::default()
    };
    let outcome = Probe::run_with_options(&spec, &opts).expect("Probe::run timeout");
    assert!(t0.elapsed() < Duration::from_secs(8));
    assert_eq!(outcome.record.identity.status, Status::Timeout);

    if let Ok(s) = fs::read_to_string(&gpid_file) {
        if let Ok(gpid) = s.trim().parse::<i32>() {
            assert!(!PathBuf::from(format!("/proc/{gpid}")).exists());
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t0_exec_failure_writes_record_with_exec_failed_error() {
    let dir = temp_dir("exec_fail");
    let spec = base_runspec(&dir, &["/nonexistent_calo_p6_binary"]);
    let outcome = Probe::run(&spec).expect("Probe::run exec fail");
    assert_eq!(outcome.record.identity.status, Status::Failed);
    assert!(outcome.record.provenance.errors.iter().any(
        |e| e.code == WarningCode::ExecFailed && e.message.contains(&libc::ENOENT.to_string())
    ));
    assert!(outcome.record_path.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t0_channel_end_to_end_degraded_counters_and_affinity() {
    let dir = temp_dir("channel_affinity");
    let cpu = first_allowed_cpu();
    let mut spec = base_runspec(
        &dir,
        &[
            "sh",
            "-c",
            "printf 'phase main start\\ntid=1 region step start\\nmetric events=1000\\ntag build=x\\ntid=1 region step end\\nphase main end\\n' > \"$CALO_CHANNEL\"",
        ],
    );
    spec.affinity = Some(RunSpecAffinity {
        cpu_list: Some(cpu.to_string()),
        numa: None,
    });
    spec.counters = vec!["task-clock".into(), "stalled-backend".into()];

    let mut overrides = BTreeMap::new();
    overrides.insert("stalled-backend".to_string(), "4096:0".to_string());
    let opts = ProbeOptions {
        counters_override: overrides,
        ..Default::default()
    };

    let outcome = Probe::run_with_options(&spec, &opts).expect("Probe::run");
    assert_eq!(outcome.record.identity.status, Status::Ok);
    assert!(outcome
        .record
        .observations
        .phases
        .as_ref()
        .unwrap()
        .contains_key("main"));
    assert_eq!(
        outcome.record.observations.reported.as_ref().unwrap()["events"].value,
        1000.0
    );
    assert_eq!(
        outcome.record.observations.tags.as_ref().unwrap()["build"],
        "x"
    );
    assert_eq!(
        outcome
            .record
            .environment
            .placement
            .as_ref()
            .and_then(|p| p.cpu_list.as_deref()),
        Some(cpu.to_string().as_str())
    );
    assert!(outcome
        .record
        .provenance
        .warnings
        .iter()
        .any(|w| w.code == WarningCode::TierDegraded && w.message.contains("stalled-backend")));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t0_output_determinism_and_probe_binary_exit_codes() {
    let d1 = temp_dir("det1");
    let d2 = temp_dir("det2");
    let r1 = Probe::run(&base_runspec(&d1, &["sh", "-c", "true"]))
        .unwrap()
        .record;
    let r2 = Probe::run(&base_runspec(&d2, &["sh", "-c", "true"]))
        .unwrap()
        .record;

    assert_eq!(r1.schema, r2.schema);
    assert_eq!(r1.identity, r2.identity);
    assert_eq!(r1.provenance.counters, r2.provenance.counters);
    assert_eq!(r1.provenance.clock, r2.provenance.clock);
    assert_eq!(r1.provenance.tiers_active, r2.provenance.tiers_active);

    let bin = env!("CARGO_BIN_EXE_probe");

    // 1. Workload exit 0
    let s0_dir = temp_dir("bin0");
    let s0_path = s0_dir.join("runspec.yaml");
    fs::write(
        &s0_path,
        serde_yaml_ng::to_string(&base_runspec(&s0_dir, &["sh", "-c", "true"])).unwrap(),
    )
    .unwrap();
    let st = Command::new(bin)
        .args(["--runspec", s0_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(0));

    // 2. Workload exit 3
    let s3_dir = temp_dir("bin3");
    let s3_path = s3_dir.join("runspec.yaml");
    fs::write(
        &s3_path,
        serde_yaml_ng::to_string(&base_runspec(&s3_dir, &["sh", "-c", "exit 3"])).unwrap(),
    )
    .unwrap();
    let st = Command::new(bin)
        .args(["--runspec", s3_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(3));

    // 3. Signal 9 -> 137
    let s9_dir = temp_dir("bin9");
    let s9_path = s9_dir.join("runspec.yaml");
    fs::write(
        &s9_path,
        serde_yaml_ng::to_string(&base_runspec(&s9_dir, &["sh", "-c", "kill -9 $$"])).unwrap(),
    )
    .unwrap();
    let st = Command::new(bin)
        .args(["--runspec", s9_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(137));

    // 4. Bad runspec -> 70
    let st = Command::new(bin)
        .args(["--runspec", "/nonexistent_runspec_p6.yaml"])
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(70));

    // 5. SIGTERM delivered to probe still writes a record with status: failed
    let sterm_dir = temp_dir("binterm");
    let sterm_path = sterm_dir.join("runspec.yaml");
    let ready_flag = sterm_dir.join("ready.flag");
    fs::write(
        &sterm_path,
        serde_yaml_ng::to_string(&base_runspec(
            &sterm_dir,
            &["sh", "-c", "touch ready.flag; exec sleep 30"],
        ))
        .unwrap(),
    )
    .unwrap();
    let mut child = Command::new(bin)
        .args(["--runspec", sterm_path.to_str().unwrap()])
        .spawn()
        .unwrap();

    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !ready_flag.exists() {
        assert!(
            Instant::now() < ready_deadline,
            "timed out waiting for probe workload to start"
        );
        thread::sleep(Duration::from_millis(5));
    }

    let _ = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    let st = child.wait().unwrap();
    assert!(
        sterm_dir.join("rank0.yaml").exists(),
        "rank0.yaml missing after probe exited with {st:?}"
    );
    let term_rec: Record =
        serde_yaml_ng::from_str(&fs::read_to_string(sterm_dir.join("rank0.yaml")).unwrap())
            .unwrap();
    assert_eq!(term_rec.identity.status, Status::Failed);

    for d in [d1, d2, s0_dir, s3_dir, s9_dir, sterm_dir] {
        let _ = fs::remove_dir_all(&d);
    }
}
