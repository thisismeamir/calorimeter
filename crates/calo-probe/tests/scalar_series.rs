//! Integration tests for Task P3: live `HostSampler`, short-lived children, post-exit `None`, and notes.

#![forbid(unsafe_code)]

use std::{
    ffi::CString,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

use calo_probe::{
    scalars::merge_peak_rss,
    series::HostSampler,
    spawn::{spawn_gated, ChildSetup, NumaPolicy, Redirect},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("calo_p3_{}_{}_{label}", std::process::id(), id));
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("valid CString")
}

#[test]
fn host_sampler_tracks_short_lived_children_and_returns_none_after_wait4() {
    let setup = ChildSetup {
        argv: vec![
            cstr("sh"),
            cstr("-c"),
            cstr("for n in 1 2 3 4; do (i=0; while [ $i -lt 8000 ]; do i=$((i + 1)); done); done"),
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
    let mut sampler = HostSampler::new(child.pid());
    child.go().expect("go");
    sampler.rebase();

    let mut rows = Vec::new();
    for _ in 0..12 {
        if let Some(row) = sampler.sample() {
            rows.push(row);
        }
        thread::sleep(Duration::from_millis(10));
    }

    let (status, ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    // After wait4 has reaped the child, /proc/<pid> is gone and sample() must return None.
    assert!(
        sampler.sample().is_none(),
        "expected HostSampler::sample() after wait4 to return None"
    );

    assert!(!rows.is_empty(), "expected at least one sampled row");
    for win in rows.windows(2) {
        assert!(
            win[1].utime_ns >= win[0].utime_ns,
            "utime_ns decreased across short-lived children: {} -> {}",
            win[0].utime_ns,
            win[1].utime_ns
        );
        assert!(
            win[1].stime_ns >= win[0].stime_ns,
            "stime_ns decreased across short-lived children: {} -> {}",
            win[0].stime_ns,
            win[1].stime_ns
        );
    }

    let series_peak = sampler.peak_rss_bytes().expect("expected peak_rss_bytes");
    assert!(series_peak > 0);
    assert_eq!(
        merge_peak_rss(ru.maxrss, Some(series_peak)),
        ru.maxrss.max(series_peak)
    );
}

#[test]
fn host_sampler_notes_report_missing_rapl_and_absent_children_on_fake_roots() {
    let dir = temp_dir("fake_roots");
    let proc_root = dir.join("proc");
    let powercap_root = dir.join("missing_powercap");

    let pid_dir = proc_root.join("9999");
    fs::create_dir_all(pid_dir.join("task/9999")).unwrap();
    fs::write(
        pid_dir.join("stat"),
        "9999 (fake) S 1 9999 9999 0 -1 0 0 0 0 0 10 5 0 0 20 0 1 0 0 0 0",
    )
    .unwrap();
    fs::write(pid_dir.join("status"), "VmRSS:\t1024 kB\n").unwrap();
    // Intentionally omit `task/9999/children` and `powercap_root`.

    let mut sampler = HostSampler::new_with_roots(9999, proc_root, powercap_root);
    let row = sampler.sample().expect("fake proc sample");
    assert_eq!(row.rss_bytes, 1024 * 1024);
    assert!(row.power_pkg_w.is_nan());
    assert!(row.power_dram_w.is_nan());

    let notes = sampler.notes();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("RAPL") && n.contains("errno")),
        "expected RAPL note with errno, got: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("children absent")),
        "expected children absent note, got: {notes:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}
#[test]
fn host_sampler_carries_rss_forward_when_status_is_missing() {
    let dir = temp_dir("carry_rss");
    let proc_root = dir.join("proc");
    let powercap_root = dir.join("missing_powercap");

    let pid_dir = proc_root.join("8888");
    fs::create_dir_all(pid_dir.join("task/8888")).unwrap();
    fs::write(
        pid_dir.join("stat"),
        "8888 (fake) S 1 8888 8888 0 -1 0 0 0 0 0 10 5 0 0 20 0 1 0 0 0 0",
    )
    .unwrap();
    fs::write(pid_dir.join("status"), "VmRSS:\t2048 kB\n").unwrap();

    let mut sampler = HostSampler::new_with_roots(8888, proc_root.clone(), powercap_root);
    let first = sampler.sample().expect("first sample");
    assert_eq!(first.rss_bytes, 2048 * 1024);

    // Remove status while leaving stat intact: sampler must carry RSS forward and note it.
    fs::remove_file(pid_dir.join("status")).unwrap();
    let second = sampler.sample().expect("second sample with stat only");
    assert_eq!(second.rss_bytes, 2048 * 1024);
    assert!(
        sampler.notes().iter().any(|n| n == "rss_carried_forward"),
        "expected rss_carried_forward note, got: {:?}",
        sampler.notes()
    );

    let _ = fs::remove_dir_all(&dir);
}
