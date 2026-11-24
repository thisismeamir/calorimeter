//! Execution of `calo spec` and `calo verify` (spec §6, §7 R-INV-5, §14, §17).

use crate::exit_codes;
use calo_core::adapters::{assert_partition_named, list_partitions_report, parse_sinfo_partitions};
use calo_core::discover::{merge_inventory, verify_fingerprint_diff};
use calo_core::exec::resolve_probe_binary;
use calo_core::proc::{CommandRunner, CommandSpec, RealCommandRunner};
use calo_core::results::write_atomic_bytes;
use calo_schema::discovery::Discovery;
use calo_schema::inventory::{ConnectType, Inventory, MachineConnect};
use calo_schema::{from_yaml_str, to_yaml_string};
use std::fs;
use std::path::Path;

/// Runs `calo spec` (local discovery, Slurm `--list-partitions`, or dry-run validation, spec §6).
#[allow(clippy::too_many_arguments)]
pub fn execute_spec(
    connect: &str,
    machine_name: &str,
    out_path: &Path,
    merge: bool,
    list_partitions: bool,
    partitions: &[String],
    yes: bool,
    probe_opt: Option<&Path>,
    quiet: bool,
) -> anyhow::Result<u8> {
    let runner = RealCommandRunner;

    if let Some(login) = connect.strip_prefix("slurm://") {
        if list_partitions {
            let out = runner.run(&CommandSpec::new(
                "ssh",
                vec![
                    login.to_string(),
                    "sinfo -h -o '%P|%D|%l|%c|%m|%f|%G'".to_string(),
                ],
            ))?;
            if !out.success() {
                if !quiet {
                    eprintln!("discovery failed: {}", out.stderr);
                }
                return Ok(exit_codes::DISCOVERY_FAILURE);
            }
            let parsed = parse_sinfo_partitions(&out.stdout);
            if !quiet {
                print!("{}", list_partitions_report(&parsed, login));
            }
            return Ok(exit_codes::SUCCESS);
        }

        if partitions.is_empty() {
            if !quiet {
                eprintln!("error: slurm:// discovery requires --partition <NAME> or --list-partitions (R-SPC-1)");
            }
            return Ok(exit_codes::DISCOVERY_FAILURE);
        }
        for p in partitions {
            assert_partition_named(partitions, None, None, p)?;
        }
        if !yes {
            if !quiet {
                eprintln!("Refusing to submit Slurm discovery jobs without --yes (R-SPC-4)");
            }
            return Ok(exit_codes::DISCOVERY_FAILURE);
        }
        return Ok(exit_codes::SUCCESS);
    }

    if connect != "local" {
        if !quiet {
            eprintln!("error: unsupported --connect `{connect}` in local spec mode");
        }
        return Ok(exit_codes::DISCOVERY_FAILURE);
    }

    let Ok(probe_bin) = resolve_probe_binary(probe_opt) else {
        if !quiet {
            eprintln!("error: could not locate `probe` binary");
        }
        return Ok(exit_codes::DISCOVERY_FAILURE);
    };

    let tmp_disc = std::env::temp_dir().join(format!("calo_discover_{}.yaml", std::process::id()));
    let out = runner.run(&CommandSpec::new(
        probe_bin.display().to_string(),
        vec![
            "--discover".to_string(),
            "--out".to_string(),
            tmp_disc.display().to_string(),
        ],
    ))?;

    if !out.success() {
        let _ = fs::remove_file(&tmp_disc);
        if !quiet {
            eprintln!("probe --discover failed: {}", out.stderr);
        }
        return Ok(exit_codes::DISCOVERY_FAILURE);
    }

    let disc_raw = fs::read_to_string(&tmp_disc)?;
    let _ = fs::remove_file(&tmp_disc);
    let discovered: Discovery = from_yaml_str(&disc_raw)?;

    let existing = if merge && out_path.exists() {
        let raw = fs::read_to_string(out_path)?;
        Some(from_yaml_str::<Inventory>(&raw)?)
    } else {
        None
    };

    let mc = MachineConnect {
        connect_type: ConnectType::Local,
        host: None,
        user: None,
        partitions: vec![],
        account: None,
    };

    let (merged, notes) = merge_inventory(
        existing.as_ref(),
        machine_name,
        mc,
        &discovered,
        env!("CARGO_PKG_VERSION"),
    );

    for n in notes {
        if !quiet {
            eprintln!("note: {}", n.message);
        }
    }

    let inv_yaml = to_yaml_string(&merged)?;
    write_atomic_bytes(out_path, inv_yaml.as_bytes())?;

    if !quiet {
        println!("wrote {}", out_path.display());
    }
    Ok(exit_codes::SUCCESS)
}

/// Runs `calo verify` comparing `inventory.yaml` fingerprint to a fresh local `Discovery` (spec §7 R-INV-5).
pub fn execute_verify(
    inventory_path: &Path,
    machine_name: &str,
    probe_opt: Option<&Path>,
    quiet: bool,
) -> anyhow::Result<u8> {
    let inv_raw = fs::read_to_string(inventory_path)?;
    let inv: Inventory = from_yaml_str(&inv_raw)?;

    let Some(machine) = inv.machines.get(machine_name) else {
        if !quiet {
            eprintln!(
                "machine `{machine_name}` not found in {}",
                inventory_path.display()
            );
        }
        return Ok(exit_codes::DISCOVERY_FAILURE);
    };

    let Ok(probe_bin) = resolve_probe_binary(probe_opt) else {
        if !quiet {
            eprintln!("error: could not locate `probe` binary");
        }
        return Ok(exit_codes::DISCOVERY_FAILURE);
    };

    let tmp_disc = std::env::temp_dir().join(format!("calo_verify_{}.yaml", std::process::id()));
    let runner = RealCommandRunner;
    let out = runner.run(&CommandSpec::new(
        probe_bin.display().to_string(),
        vec![
            "--discover".to_string(),
            "--out".to_string(),
            tmp_disc.display().to_string(),
        ],
    ))?;

    if !out.success() {
        let _ = fs::remove_file(&tmp_disc);
        if !quiet {
            eprintln!("probe --discover failed: {}", out.stderr);
        }
        return Ok(exit_codes::DISCOVERY_FAILURE);
    }

    let disc_raw = fs::read_to_string(&tmp_disc)?;
    let _ = fs::remove_file(&tmp_disc);
    let discovered: Discovery = from_yaml_str(&disc_raw)?;

    let diff = verify_fingerprint_diff(machine.fingerprint.as_deref(), &discovered);
    if diff.matches {
        if !quiet {
            println!("verify ok: `{machine_name}` fingerprint {}", diff.observed);
        }
        Ok(exit_codes::SUCCESS)
    } else {
        if !quiet {
            eprintln!(
                "fingerprint mismatch for `{machine_name}`: expected {:?}, observed {}",
                diff.expected, diff.observed
            );
        }
        Ok(exit_codes::DISCOVERY_FAILURE)
    }
}
