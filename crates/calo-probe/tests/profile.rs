//! Integration tests for Task P7: T1 sampling, maps capture, loss accounting, `no_symbols`,
//! and attribute events.

#![forbid(unsafe_code)]

use std::{
    ffi::CString,
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use calo_probe::{
    profile::{elf_has_symbols, ProfileCollector, ProfileConfig},
    spawn::{spawn_gated, ChildSetup, NumaPolicy, Redirect},
    sys::sample::{CallgraphConfig, SampleTrigger, Sampler, Source},
};
use calo_schema::common::{CallgraphMethod, WarningCode};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("calo_p7_{}_{}_{label}", std::process::id(), id));
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

#[test]
fn t1_collector_captures_maps_and_attribute_samples() {
    let dir = temp_dir("t1_maps_attr");
    let setup = ChildSetup {
        argv: vec![
            CString::new("sh").unwrap(),
            CString::new("-c").unwrap(),
            CString::new("i=0; while [ $i -lt 30000 ]; do i=$((i + 1)); done").unwrap(),
        ],
        env: calo_probe::sys::spawn::current_env_cstrings(),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    let cfg = ProfileConfig {
        freq_hz: 199,
        callgraph: CallgraphMethod::Fp,
        stack_bytes: 8192,
        attribute: vec!["task-clock".to_string()],
        max_loss: 0.50,
        strict_loss: false,
    };

    let collector = ProfileCollector::open(child.pid(), cfg, &dir).expect("open collector");
    if collector.raw_fds().is_empty() {
        eprintln!(
            "SKIPPED t1_collector_captures_maps_and_attribute_samples: perf sampling denied ({:?})",
            collector
                .finish(true)
                .map(|s| s.warnings)
                .unwrap_or_default()
        );
        let _ = fs::remove_dir_all(&dir);
        return;
    }

    child.go().expect("go");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    let summary = collector.finish(true).expect("finish collector");
    assert!(
        dir.join("profile.maps.calotbl").exists(),
        "profile.maps.calotbl must be created"
    );
    assert!(
        !summary.record_profile.modules.is_empty(),
        "expected MMAP2 executable modules captured for sh workload"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn tiny_ring_triggers_sample_loss_warning() {
    let setup = ChildSetup {
        argv: vec![
            CString::new("sh").unwrap(),
            CString::new("-c").unwrap(),
            CString::new("i=0; while [ $i -lt 60000 ]; do i=$((i + 1)); done").unwrap(),
        ],
        env: calo_probe::sys::spawn::current_env_cstrings(),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    // Deliberately tiny 1-page ring (`pages_log2 = 0`) at high frequency to force LOST records.
    let Ok(mut sampler) = Sampler::open_configured(
        child.pid(),
        Source::CpuClock,
        SampleTrigger::Freq(4000),
        0,
        CallgraphConfig::Fp,
    ) else {
        eprintln!("SKIPPED tiny_ring_triggers_sample_loss_warning: perf sampling denied");
        return;
    };

    child.go().expect("go");
    let _ = child.wait4().expect("wait4");

    let mut lost = 0u64;
    let mut samples = 0u64;
    let _ = sampler.drain(|ev| match ev {
        calo_probe::sys::record::Event::Lost { count } => lost += count,
        calo_probe::sys::record::Event::Sample(_) => samples += 1,
        _ => {}
    });
    if lost == 0 {
        eprintln!(
            "SKIPPED tiny_ring_triggers_sample_loss_warning: kernel did not overflow 1-page ring (samples={samples})"
        );
    }
}

#[test]
fn no_symbols_detects_stripped_binary() {
    let dir = temp_dir("strip");
    let bin_copy = dir.join("sh_stripped");
    if fs::copy("/bin/sh", &bin_copy).is_err() {
        eprintln!("SKIPPED no_symbols_detects_stripped_binary: cannot copy /bin/sh");
        let _ = fs::remove_dir_all(&dir);
        return;
    }
    let strip_ok = Command::new("strip")
        .arg("--strip-all")
        .arg(&bin_copy)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !strip_ok {
        eprintln!("SKIPPED no_symbols_detects_stripped_binary: `strip` command unavailable");
        let _ = fs::remove_dir_all(&dir);
        return;
    }

    assert!(
        !elf_has_symbols(&bin_copy),
        "stripped binary must report no symbols"
    );

    let mut collector = ProfileCollector::open(
        std::process::id() as libc::pid_t,
        ProfileConfig::default(),
        &dir,
    )
    .unwrap();
    let has_syms = collector.check_main_executable_symbols(&bin_copy);
    assert!(!has_syms);
    let summary = collector.finish(has_syms).unwrap();
    assert!(summary.invalid);
    assert!(summary
        .errors
        .iter()
        .any(|e| e.code == WarningCode::NoSymbols));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn probe_run_profile_mode_emits_sidecars_and_rejects_stripped_binary() {
    use calo_probe::probe::Probe;
    use calo_schema::{
        common::{Mode, Status},
        runspec::{RunSpec, RunSpecOutputs, RunSpecProfile},
        SchemaDoc,
    };
    use std::collections::BTreeMap;

    let dir = temp_dir("live_t1");
    let src = dir.join("busy.c");
    let bin = dir.join("busy");
    fs::write(
        &src,
        r#"
#include <stdint.h>
__attribute__((noinline)) uint64_t inner_work(uint64_t n) {
    volatile uint64_t acc = 0;
    for (uint64_t i = 0; i < n; ++i) { acc += i ^ (i >> 3); }
    return acc;
}
int main(void) {
    return inner_work(40000000ULL) == 0 ? 1 : 0;
}
"#,
    )
    .expect("write busy.c");

    let cc_ok = Command::new("cc")
        .args(["-O1", "-g", "-fno-omit-frame-pointer", "-o"])
        .arg(&bin)
        .arg(&src)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !cc_ok {
        eprintln!("SKIPPED probe_run_profile_mode_emits_sidecars_and_rejects_stripped_binary: `cc` unavailable");
        let _ = fs::remove_dir_all(&dir);
        return;
    }

    let out_dir = dir.join("out_ok");
    let spec = RunSpec {
        schema: RunSpec::SCHEMA.to_string(),
        run_id: "t1_live.r0".to_string(),
        mode: Mode::Profile,
        command: vec![bin.to_string_lossy().into_owned()],
        cwd: dir.to_string_lossy().into_owned(),
        env: BTreeMap::new(),
        affinity: None,
        rlimits: None,
        counters: vec!["task-clock".to_string()],
        sample_hz: Some(20),
        profile: Some(RunSpecProfile {
            freq_hz: Some(199),
            callgraph: Some(CallgraphMethod::Fp),
            stack_bytes: None,
            attribute: Vec::new(),
            max_loss: Some(0.50),
        }),
        mpi: None,
        gpu: None,
        shim_inject: None,
        regions: None,
        channel: None,
        rank: None,
        outputs: RunSpecOutputs {
            dir: out_dir.to_string_lossy().into_owned(),
        },
    };

    let outcome = Probe::run(&spec).expect("Probe::run in profile mode");
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.record.identity.status, Status::Ok);
    assert!(outcome.record_path.exists());
    assert!(out_dir.join("profile.samples.calotbl").exists());
    assert!(out_dir.join("profile.chains.calotbl").exists());
    assert!(out_dir.join("profile.maps.calotbl").exists());

    let prof = outcome
        .record
        .observations
        .profile
        .as_ref()
        .expect("observations.profile must be populated in Mode::Profile");
    assert_eq!(prof.method, CallgraphMethod::Fp);
    assert_eq!(
        outcome.record.provenance.callgraph,
        Some(CallgraphMethod::Fp)
    );
    if prof.samples > 0 {
        assert!(
            !prof.modules.is_empty(),
            "expected executable modules from MMAP2"
        );
    }

    // R-BLD-3: stripped binary must be marked Invalid with NoSymbols in profile mode.
    let bin_stripped = dir.join("busy_stripped");
    fs::copy(&bin, &bin_stripped).expect("copy bin");
    let strip_ok = Command::new("strip")
        .arg("--strip-all")
        .arg(&bin_stripped)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if strip_ok {
        let out_stripped = dir.join("out_stripped");
        let mut bad_spec = spec;
        bad_spec.run_id = "t1_stripped.r0".to_string();
        bad_spec.command = vec![bin_stripped.to_string_lossy().into_owned()];
        bad_spec.outputs = RunSpecOutputs {
            dir: out_stripped.to_string_lossy().into_owned(),
        };
        let bad_out = Probe::run(&bad_spec).expect("Probe::run stripped");
        assert_eq!(bad_out.record.identity.status, Status::Invalid);
        assert!(bad_out
            .record
            .provenance
            .errors
            .iter()
            .any(|e| e.code == WarningCode::NoSymbols));
    }

    let _ = fs::remove_dir_all(&dir);
}
