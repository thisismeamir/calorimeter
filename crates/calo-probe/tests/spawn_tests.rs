//! Integration tests for Task P1: gated spawn, handshake, pre-exec setup, and `wait4` rusage.

use std::{
    ffi::CString,
    fs,
    mem::MaybeUninit,
    path::PathBuf,
    ptr,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use calo_probe::{
    clock::now_ns,
    spawn::{spawn_gated, ChildSetup, ExecError, NumaPolicy, Redirect},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn temp_path(label: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("calo_probe_p1_{pid}_{id}_{label}"));
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("valid CString")
}

fn default_env_with(extra: &[(&str, &str)]) -> Vec<CString> {
    let mut map = std::collections::BTreeMap::new();
    for (k, v) in std::env::vars() {
        if !k.contains('\0') && !v.contains('\0') {
            map.insert(k, v);
        }
    }
    for &(k, v) in extra {
        map.insert(k.to_string(), v.to_string());
    }
    map.into_iter()
        .filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok())
        .collect()
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
fn child_is_sleeping_and_probe_image_before_go() {
    let dir = temp_path("gated");
    let out_file = dir.join("out.txt");
    let setup = ChildSetup {
        argv: vec![cstr("sh"), cstr("-c"), cstr("printf 'ran' > out.txt")],
        env: default_env_with(&[]),
        cwd: Some(cstr(dir.to_str().unwrap())),
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    let pid = child.pid();

    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("read /proc/<pid>/stat");
    let rparen = stat.rfind(')').expect("closing paren in stat");
    let state = stat[rparen + 1..].trim_start().chars().next().unwrap();
    assert_eq!(state, 'S', "expected child state 'S' before go, got: {stat}");

    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).expect("read /proc/<pid>/cmdline");
    let cmdline_str = String::from_utf8_lossy(&cmdline);
    assert!(
        !cmdline_str.starts_with("sh\0"),
        "child should still have test/probe cmdline before go, got: {cmdline_str:?}"
    );
    assert!(
        !out_file.exists(),
        "workload side effect must not exist before go()"
    );

    child.go().expect("child.go()");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));
    assert_eq!(fs::read_to_string(&out_file).unwrap(), "ran");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn affinity_applied_in_child() {
    let dir = temp_path("affinity");
    let out_file = dir.join("affinity.txt");
    let target_cpu = first_allowed_cpu();

    let setup = ChildSetup {
        argv: vec![
            cstr("sh"),
            cstr("-c"),
            cstr("grep '^Cpus_allowed_list:' /proc/self/status"),
        ],
        env: default_env_with(&[]),
        cwd: None,
        cpu_list: Some(vec![target_cpu]),
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::File(cstr(out_file.to_str().unwrap())),
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    child.go().expect("go");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    let text = fs::read_to_string(&out_file).expect("read affinity output");
    let allowed = text
        .strip_prefix("Cpus_allowed_list:")
        .expect("Cpus_allowed_list prefix")
        .trim();
    assert_eq!(allowed, target_cpu.to_string());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn env_cwd_and_stdout_redirect_applied() {
    let dir = temp_path("env_cwd");
    let out_file = dir.join("stdout.txt");
    let canonical_dir = fs::canonicalize(&dir).expect("canonicalize temp dir");

    let setup = ChildSetup {
        argv: vec![
            cstr("sh"),
            cstr("-c"),
            cstr("printf '%s|%s' \"$CALO_P1_MARKER\" \"$(pwd -P)\""),
        ],
        env: default_env_with(&[("CALO_P1_MARKER", "expected_value_42")]),
        cwd: Some(cstr(canonical_dir.to_str().unwrap())),
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::File(cstr(out_file.to_str().unwrap())),
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    child.go().expect("go");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    let captured = fs::read_to_string(&out_file).expect("read redirected stdout");
    assert_eq!(
        captured,
        format!("expected_value_42|{}", canonical_dir.display())
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rlimit_applied_in_child() {
    let dir = temp_path("rlimit");
    let out_file = dir.join("ulimit.txt");

    let setup = ChildSetup {
        argv: vec![cstr("sh"), cstr("-c"), cstr("ulimit -c")],
        env: default_env_with(&[]),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: vec![(libc::RLIMIT_CORE as i32, 0, 0)],
        stdout: Redirect::File(cstr(out_file.to_str().unwrap())),
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    child.go().expect("go");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    let text = fs::read_to_string(&out_file).expect("read ulimit output");
    assert_eq!(text.trim(), "0");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn exec_failure_reports_enoent_not_exit_127() {
    let setup = ChildSetup {
        argv: vec![cstr("/nonexistent_calo_probe_binary_p1")],
        env: default_env_with(&[]),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    let res = child.go();
    match res {
        Err(ExecError::Exec(errno)) => assert_eq!(errno, libc::ENOENT),
        other => panic!("expected Err(ExecError::Exec(ENOENT)), got: {other:?}"),
    }
}

// Requires `cargo nextest` (one process per test): under `cargo test`, concurrent test
// threads open and close file descriptors in the same process.
#[test]
fn drop_on_ungoed_child_kills_and_reaps_without_leaking_fds() {
    let fd_count_before = fs::read_dir("/proc/self/fd").expect("read /proc/self/fd").count();

    let pid = {
        let setup = ChildSetup {
            argv: vec![cstr("sh"), cstr("-c"), cstr("sleep 60")],
            env: default_env_with(&[]),
            cwd: None,
            cpu_list: None,
            numa: NumaPolicy::None,
            rlimits: Vec::new(),
            stdout: Redirect::Null,
            stderr: Redirect::Null,
        };
        let child = spawn_gated(&setup).expect("spawn_gated");
        let pid = child.pid();
        assert!(PathBuf::from(format!("/proc/{pid}")).exists());
        pid
    };

    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "dropped child {pid} must be reaped and absent from /proc"
    );
    let fd_count_after = fs::read_dir("/proc/self/fd").expect("read /proc/self/fd").count();
    assert_eq!(
        fd_count_before, fd_count_after,
        "file descriptors leaked across spawn_gated + Drop"
    );
}

#[test]
fn parent_death_kills_child() {
    if let Ok(pid_file) = std::env::var("CALO_P1_HELPER_PID_FILE") {
        let setup = ChildSetup {
            argv: vec![cstr("sh"), cstr("-c"), cstr("exec sleep 60")],
            env: default_env_with(&[]),
            cwd: None,
            cpu_list: None,
            numa: NumaPolicy::None,
            rlimits: Vec::new(),
            stdout: Redirect::Null,
            stderr: Redirect::Null,
        };
        let mut grandchild = spawn_gated(&setup).expect("helper spawn_gated");
        let gpid = grandchild.pid();
        grandchild.go().expect("helper grandchild.go");
        fs::write(&pid_file, gpid.to_string()).expect("write grandchild pid");
        std::mem::forget(grandchild);
        std::process::exit(0);
    }

    let dir = temp_path("pdeathsig");
    let pid_file = dir.join("grandchild.pid");
    let current_exe = std::env::current_exe().expect("current_exe");

    let setup = ChildSetup {
        argv: vec![
            cstr(current_exe.to_str().unwrap()),
            cstr("--exact"),
            cstr("parent_death_kills_child"),
            cstr("--nocapture"),
        ],
        env: default_env_with(&[("CALO_P1_HELPER_PID_FILE", pid_file.to_str().unwrap())]),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut intermediate = spawn_gated(&setup).expect("spawn intermediate");
    intermediate.go().expect("intermediate.go");
    let (status, _ru) = intermediate.wait4().expect("wait4 intermediate");
    assert_eq!(status.code, Some(0));

    let gpid: i32 = fs::read_to_string(&pid_file)
        .expect("read grandchild.pid")
        .trim()
        .parse()
        .expect("parse grandchild pid");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut alive = true;
    while Instant::now() < deadline {
        if let Ok(stat) = fs::read_to_string(format!("/proc/{gpid}/stat")) {
            if let Some(idx) = stat.rfind(')') {
                let state = stat[idx + 1..].trim_start().chars().next();
                if matches!(state, Some('Z' | 'X')) {
                    alive = false;
                    break;
                }
            }
        } else {
            alive = false;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !alive,
        "grandchild {gpid} survived after intermediate parent exited"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rusage_values_plausible_for_sleep_and_busy_loop() {
    let sleep_setup = ChildSetup {
        argv: vec![cstr("sh"), cstr("-c"), cstr("sleep 0.2")],
        env: default_env_with(&[]),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };
    let mut sleep_child = spawn_gated(&sleep_setup).expect("spawn sleep");
    let t0 = now_ns();
    sleep_child.go().expect("sleep go");
    let (sleep_status, sleep_ru) = sleep_child.wait4().expect("sleep wait4");
    let sleep_wall_ns = now_ns() - t0;

    assert_eq!(sleep_status.code, Some(0));
    assert!(
        sleep_wall_ns >= 150_000_000,
        "expected >= 150ms wall time, got {sleep_wall_ns} ns"
    );
    assert!(
        sleep_ru.utime_ns + sleep_ru.stime_ns < 150_000_000,
        "sleep CPU time too high: {:?}",
        sleep_ru
    );
    assert!(
        sleep_ru.maxrss >= 65_536,
        "expected maxrss in bytes >= 64 KiB, got {}",
        sleep_ru.maxrss
    );
    assert!(sleep_ru.nvcsw >= 1, "expected >= 1 voluntary context switch");

    let busy_setup = ChildSetup {
        argv: vec![
            cstr("sh"),
            cstr("-c"),
            cstr("i=0; while [ $i -lt 40000 ]; do i=$((i + 1)); done"),
        ],
        env: default_env_with(&[]),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };
    let mut busy_child = spawn_gated(&busy_setup).expect("spawn busy");
    busy_child.go().expect("busy go");
    let (busy_status, busy_ru) = busy_child.wait4().expect("busy wait4");

    assert_eq!(busy_status.code, Some(0));
    assert!(
        busy_ru.utime_ns + busy_ru.stime_ns >= 5_000_000,
        "expected measurable CPU time for busy loop, got: {:?}",
        busy_ru
    );
    assert!(busy_ru.maxrss >= 65_536);
}

#[test]
fn child_resets_signals_and_clears_sigmask() {
    let dir = temp_path("signals");
    let out_file = dir.join("status_sigs.txt");

    // SAFETY: We save the prior SIGPIPE disposition and thread signal mask, set SIGPIPE to SIG_IGN
    // and block SIGUSR1 for the spawn, then restore both immediately after `spawn_gated`.
    let (prev_pipe, prev_mask) = unsafe {
        let mut ign = MaybeUninit::<libc::sigaction>::zeroed();
        (*ign.as_mut_ptr()).sa_sigaction = libc::SIG_IGN;
        libc::sigemptyset(&mut (*ign.as_mut_ptr()).sa_mask);
        let mut prev_pipe = MaybeUninit::<libc::sigaction>::zeroed();
        assert_eq!(
            libc::sigaction(libc::SIGPIPE, ign.as_ptr(), prev_pipe.as_mut_ptr()),
            0
        );

        let mut usr1 = MaybeUninit::<libc::sigset_t>::zeroed();
        libc::sigemptyset(usr1.as_mut_ptr());
        libc::sigaddset(usr1.as_mut_ptr(), libc::SIGUSR1);
        let mut prev_mask = MaybeUninit::<libc::sigset_t>::zeroed();
        assert_eq!(
            libc::sigprocmask(libc::SIG_BLOCK, usr1.as_ptr(), prev_mask.as_mut_ptr()),
            0
        );
        (prev_pipe.assume_init(), prev_mask.assume_init())
    };

    let setup = ChildSetup {
        argv: vec![
            cstr("sh"),
            cstr("-c"),
            cstr("grep -E '^(SigIgn|SigBlk):' /proc/self/status"),
        ],
        env: default_env_with(&[]),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::File(cstr(out_file.to_str().unwrap())),
        stderr: Redirect::Null,
    };

    let child_res = spawn_gated(&setup);

    // SAFETY: `prev_pipe` and `prev_mask` were initialized by `sigaction` and `sigprocmask`.
    unsafe {
        let _ = libc::sigaction(libc::SIGPIPE, &prev_pipe, ptr::null_mut());
        let _ = libc::sigprocmask(libc::SIG_SETMASK, &prev_mask, ptr::null_mut());
    }

    let mut child = child_res.expect("spawn_gated");
    child.go().expect("go");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    let text = fs::read_to_string(&out_file).expect("read status_sigs.txt");
    let mut sig_ign = None;
    let mut sig_blk = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("SigIgn:") {
            sig_ign = Some(u64::from_str_radix(rest.trim(), 16).expect("parse SigIgn hex"));
        } else if let Some(rest) = line.strip_prefix("SigBlk:") {
            sig_blk = Some(u64::from_str_radix(rest.trim(), 16).expect("parse SigBlk hex"));
        }
    }

    let sig_ign = sig_ign.expect("SigIgn line missing");
    let sig_blk = sig_blk.expect("SigBlk line missing");
    let sigpipe_bit = 1u64 << (libc::SIGPIPE as u32 - 1);
    assert_eq!(
        sig_ign & sigpipe_bit,
        0,
        "SIGPIPE (signal 13) must not be ignored in child: SigIgn={sig_ign:#x}"
    );
    assert_eq!(
        sig_blk, 0,
        "signal mask must be cleared in child: SigBlk={sig_blk:#x}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_spawn_go_wait4_eight_threads() {
    let start = Instant::now();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            thread::spawn(|| {
                let setup = ChildSetup {
                    argv: vec![cstr("sh"), cstr("-c"), cstr("true")],
                    env: default_env_with(&[]),
                    cwd: None,
                    cpu_list: None,
                    numa: NumaPolicy::None,
                    rlimits: Vec::new(),
                    stdout: Redirect::Null,
                    stderr: Redirect::Null,
                };
                let mut child = spawn_gated(&setup).expect("concurrent spawn_gated");
                child.go().expect("concurrent go");
                let (status, _ru) = child.wait4().expect("concurrent wait4");
                assert_eq!(status.code, Some(0));
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "8 concurrent spawn/go/wait4 took {:?}",
        start.elapsed()
    );
}

#[test]
fn kill_group_terminates_background_grandchild() {
    let dir = temp_path("kill_group");
    let gpid_file = dir.join("grandchild.pid");
    let cmd = format!("sleep 60 & echo $! > {}; wait", gpid_file.display());

    let setup = ChildSetup {
        argv: vec![cstr("sh"), cstr("-c"), cstr(&cmd)],
        env: default_env_with(&[]),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    child.go().expect("go");

    let wait_file_deadline = Instant::now() + Duration::from_secs(2);
    let gpid: i32 = loop {
        if let Ok(content) = fs::read_to_string(&gpid_file) {
            if let Ok(p) = content.trim().parse::<i32>() {
                break p;
            }
        }
        assert!(
            Instant::now() < wait_file_deadline,
            "timed out waiting for grandchild pid file"
        );
        thread::sleep(Duration::from_millis(10));
    };

    assert!(
        PathBuf::from(format!("/proc/{gpid}")).exists(),
        "grandchild {gpid} should be running before kill_group"
    );

    child.kill_group(libc::SIGKILL).expect("kill_group(SIGKILL)");
    let (_status, _ru) = child.wait4().expect("wait4");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut alive = true;
    while Instant::now() < deadline {
        if let Ok(stat) = fs::read_to_string(format!("/proc/{gpid}/stat")) {
            if let Some(idx) = stat.rfind(')') {
                let state = stat[idx + 1..].trim_start().chars().next();
                if matches!(state, Some('Z' | 'X')) {
                    alive = false;
                    break;
                }
            }
        } else {
            alive = false;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !alive,
        "grandchild {gpid} survived after kill_group(SIGKILL)"
    );
    let _ = fs::remove_dir_all(&dir);
}
