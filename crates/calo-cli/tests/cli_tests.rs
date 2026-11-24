use std::path::PathBuf;
use std::process::Command;

fn calo_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_calo"))
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn valid_suite_passes_lint_and_plan() {
    let inv = fixture_path("inventory.yaml");
    let suite = fixture_path("valid_suite.yaml");

    let out = Command::new(calo_bin())
        .args(["--inventory", inv.to_str().unwrap(), "--suite", suite.to_str().unwrap(), "lint"])
        .output()
        .expect("run calo lint");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    // Check `calo plan` table output
    let plan_out = Command::new(calo_bin())
        .args(["--inventory", inv.to_str().unwrap(), "--suite", suite.to_str().unwrap(), "plan"])
        .output()
        .expect("run calo plan");
    assert_eq!(plan_out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&plan_out.stdout);
    assert!(stdout.contains("scaling.N_128.nodes_1.patch_none.r0"));
    assert!(stdout.contains("scaling_prof.N_256.nodes_2.patch_none.r0"));
    assert!(stdout.contains("Total runs: 6"));

    // Check `calo plan --format yaml`
    let yaml_out = Command::new(calo_bin())
        .args([
            "--inventory",
            inv.to_str().unwrap(),
            "--suite",
            suite.to_str().unwrap(),
            "plan",
            "--format",
            "yaml",
        ])
        .output()
        .expect("run calo plan --format yaml");
    assert_eq!(yaml_out.status.code(), Some(0));
    let yaml_stdout = String::from_utf8_lossy(&yaml_out.stdout);
    assert!(yaml_stdout.contains("run_id: scaling.N_128.nodes_1.patch_none.r0"));
    assert!(yaml_stdout.contains("node_hours:"));
}

fn assert_lint_fails_with(fixture_name: &str, expected_substr: &str) {
    let inv = fixture_path("inventory.yaml");
    let suite = fixture_path(fixture_name);

    let out = Command::new(calo_bin())
        .args(["--inventory", inv.to_str().unwrap(), "--suite", suite.to_str().unwrap(), "lint"])
        .output()
        .expect("run calo lint");

    // Spec §17: exit code 2 for lint failure
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(expected_substr),
        "expected stderr to contain `{expected_substr}`, got:\n{stderr}"
    );
    // Output format must be `file:line: message`
    assert!(stderr.contains(&format!("{fixture_name}:")));
}

#[test]
fn lint_rejects_bad_schema_header() {
    assert_lint_fails_with("bad_schema.yaml", "schema version mismatch");
}

#[test]
fn lint_rejects_unknown_expression_reference() {
    assert_lint_fails_with("bad_unknown_ref.yaml", "unknown reference `params.nonexistent_axis`");
}

#[test]
fn lint_rejects_unknown_machine_build_step_patchset() {
    assert_lint_fails_with("bad_unknown_names.yaml", "unknown machine `unknown-cluster`");
    assert_lint_fails_with("bad_unknown_names.yaml", "unknown build `missing_build`");
    assert_lint_fails_with("bad_unknown_names.yaml", "unknown step `missing_step`");
    assert_lint_fails_with("bad_unknown_names.yaml", "unknown patch set `missing_patchset`");
}

#[test]
fn lint_rejects_service_dependency_cycle() {
    assert_lint_fails_with("bad_service_cycle.yaml", "service dependency cycle detected");
}

#[test]
fn lint_rejects_report_requiring_tiers_not_enabled_by_mode() {
    assert_lint_fails_with("bad_report_tiers.yaml", "requires tier `T1`");
}

#[test]
fn lint_rejects_profile_benchmark_without_debug_info_required() {
    assert_lint_fails_with("bad_profile_debug_info.yaml", "R-BLD-3");
}

#[test]
fn lint_rejects_benchmark_without_exactly_one_measured_step() {
    assert_lint_fails_with("bad_measured_steps.yaml", "R-STP-2");
}
