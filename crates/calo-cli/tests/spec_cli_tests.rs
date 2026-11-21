#[cfg(unix)]
#[test]
fn calo_spec_and_verify_end_to_end_with_fake_probe() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("calo_spec_cli_{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();

    let fake_probe = dir.join("probe");
    let script = r#"#!/bin/sh
OUT=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--out" ]; then OUT="$2"; shift 2; else shift; fi
done
cat > "$OUT" <<'EOF'
schema: calo.discovery/1
probe_version: 0.1.0
at: "2026-11-16T00:00:00Z"
fingerprint: dummy
observed:
  kernel: 6.6.0
  cpu:
    model: TestCPU
    cores: 16
EOF
"#;
    std::fs::write(&fake_probe, script).unwrap();
    let mut perms = std::fs::metadata(&fake_probe).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_probe, perms).unwrap();

    let inv_out = dir.join("inventory.yaml");
    let calo_bin = env!("CARGO_BIN_EXE_calo");

    let spec_out = Command::new(calo_bin)
        .args([
            "--inventory",
            inv_out.to_str().unwrap(),
            "spec",
            "--connect",
            "local",
            "--probe",
            fake_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(spec_out.status.code(), Some(0));
    assert!(inv_out.exists());

    let verify_out = Command::new(calo_bin)
        .args([
            "--inventory",
            inv_out.to_str().unwrap(),
            "verify",
            "--probe",
            fake_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(verify_out.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&dir);
}
