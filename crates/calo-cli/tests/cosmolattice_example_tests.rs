use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn cosmolattice_example_passes_lint_and_plan_with_weak_scaling_derive() {
    let root = repo_root();
    let suite = root.join("examples/cosmolattice/suite.yaml");
    let inv = root.join("examples/cosmolattice/inventory.yaml");
    let calo = env!("CARGO_BIN_EXE_calo");

    let lint_out = Command::new(calo)
        .args([
            "--suite",
            suite.to_str().unwrap(),
            "--inventory",
            inv.to_str().unwrap(),
            "lint",
        ])
        .output()
        .expect("run calo lint on cosmolattice example");
    assert_eq!(
        lint_out.status.code(),
        Some(0),
        "lint stderr: {}",
        String::from_utf8_lossy(&lint_out.stderr)
    );

    let plan_out = Command::new(calo)
        .args([
            "--suite",
            suite.to_str().unwrap(),
            "--inventory",
            inv.to_str().unwrap(),
            "plan",
            "--format",
            "yaml",
        ])
        .output()
        .expect("run calo plan on cosmolattice example");
    assert_eq!(plan_out.status.code(), Some(0));

    let stdout = String::from_utf8_lossy(&plan_out.stdout);
    // Expected expanded run count:
    // - baseline: 1 warmup + 3 repeats = 4
    // - strong-scaling: 4 nodes * 3 repeats = 12
    // - weak-scaling: 3 ranks * 2 repeats = 6 (N = 128, 256, 512)
    // - patch-effect: 2 patches * 3 repeats = 6
    // - strong-scaling-profile: 3 nodes (1, 2, 4) * 1 repeat = 3
    // Total = 31 runs
    assert!(stdout.contains("runs: 31"), "plan stdout:\n{stdout}");
    assert!(stdout.contains("weak-scaling.N_128."));
    assert!(stdout.contains("weak-scaling.N_256."));
    assert!(stdout.contains("weak-scaling.N_512."));
    assert!(stdout.contains("ranks_1"));
    assert!(stdout.contains("ranks_8"));
    assert!(stdout.contains("ranks_64"));
}
