//! Integration tests for Task P2: `CounterSet` degradation, scopes, and `enable_on_exec` gating.

#![forbid(unsafe_code)]

use std::{collections::BTreeMap, ffi::CString};

use calo_probe::{
    counters::{CounterAlias, CounterScope, CounterSet},
    spawn::{spawn_gated, ChildSetup, NumaPolicy, Redirect},
};
use calo_schema::WarningCode;

fn cstr(s: &str) -> CString {
    CString::new(s).expect("valid CString")
}

#[test]
fn unavailable_counter_degrades_without_aborting_others() {
    let setup = ChildSetup {
        argv: vec![cstr("sh"), cstr("-c"), cstr("true")],
        env: calo_probe::sys::spawn::current_env_cstrings(),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).expect("spawn_gated");
    let mut overrides = BTreeMap::new();
    // Deliberately nonexistent PMU type 4096 for `stalled-backend`.
    overrides.insert("stalled-backend".to_string(), "4096:0".to_string());

    let mut set = CounterSet::open_with_overrides(
        child.pid(),
        &[CounterAlias::TaskClock, CounterAlias::StalledBackend],
        &overrides,
        CounterScope::Auto,
    );

    if !set.any_open() {
        eprintln!(
            "SKIPPED unavailable_counter_degrades_without_aborting_others: perf_event_open denied ({:?})",
            set.warnings()
        );
        return;
    }

    assert!(
        set.open_names().contains(&"task-clock"),
        "expected task-clock to remain open when stalled-backend degraded"
    );
    assert!(
        set.warnings()
            .iter()
            .any(|w| w.code == WarningCode::TierDegraded && w.message.contains("stalled-backend")),
        "expected tier_degraded warning naming stalled-backend, got: {:?}",
        set.warnings()
    );

    child.go().expect("go");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    let counts = set.read_all();
    assert!(counts.contains_key("task-clock"));
}

#[test]
fn count_on_exec_only_is_zero_before_go_and_nonzero_after() {
    let setup = ChildSetup {
        argv: vec![
            cstr("sh"),
            cstr("-c"),
            cstr("i=0; while [ $i -lt 20000 ]; do i=$((i + 1)); done"),
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
    let mut set = CounterSet::open(
        child.pid(),
        &[CounterAlias::TaskClock, CounterAlias::Instructions],
    );

    if !set.any_open() {
        eprintln!(
            "SKIPPED count_on_exec_only_is_zero_before_go_and_nonzero_after: perf_event_open denied ({:?})",
            set.warnings()
        );
        return;
    }

    let pre_exec = set.read_all();
    for (name, count) in &pre_exec {
        assert_eq!(
            count.value, 0,
            "counter '{name}' must read 0 before go(), got {}",
            count.value
        );
    }

    child.go().expect("go");
    let (status, _ru) = child.wait4().expect("wait4");
    assert_eq!(status.code, Some(0));

    let post_exit = set.read_all();
    assert!(
        post_exit.values().any(|c| c.value > 0),
        "expected at least one counter > 0 after busy loop, got: {:?}",
        post_exit
    );
}

#[test]
fn scope_user_opens_and_reports_kernel_excluded() {
    let setup = ChildSetup {
        argv: vec![cstr("sh"), cstr("-c"), cstr("true")],
        env: calo_probe::sys::spawn::current_env_cstrings(),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let child = spawn_gated(&setup).expect("spawn_gated");
    let set = CounterSet::open_with_overrides(
        child.pid(),
        &[CounterAlias::TaskClock, CounterAlias::Instructions],
        &BTreeMap::new(),
        CounterScope::User,
    );

    if !set.any_open() {
        eprintln!(
            "SKIPPED scope_user_opens_and_reports_kernel_excluded: perf_event_open denied ({:?})",
            set.warnings()
        );
        return;
    }

    assert_eq!(set.open_names(), set.kernel_excluded());
}

#[test]
fn scope_auto_reports_kernel_excluded_consistently() {
    let setup = ChildSetup {
        argv: vec![cstr("sh"), cstr("-c"), cstr("true")],
        env: calo_probe::sys::spawn::current_env_cstrings(),
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let child = spawn_gated(&setup).expect("spawn_gated");
    let set = CounterSet::open_with_overrides(
        child.pid(),
        &[CounterAlias::TaskClock, CounterAlias::Instructions],
        &BTreeMap::new(),
        CounterScope::Auto,
    );

    if !set.any_open() {
        eprintln!(
            "SKIPPED scope_auto_reports_kernel_excluded_consistently: perf_event_open denied ({:?})",
            set.warnings()
        );
        return;
    }

    let open = set.open_names();
    for excluded in set.kernel_excluded() {
        assert!(
            open.contains(&excluded),
            "kernel_excluded entry '{excluded}' not in open_names {:?}",
            open
        );
    }
}
