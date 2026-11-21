#![forbid(unsafe_code)]
//! Command-line interface for Calorimeter (`calo`), implementing spec §14 and §17.

pub mod cli;
pub mod lint;
pub mod plan_cmd;
pub mod run_cmd;
pub mod spec_cmd;
// Analyze CMD implementation is hanging...
pub mod analyze_cmd;

use clap::Parser;
use cli::{Cli, Command};
use std::process::ExitCode;

/// Normative exit codes per spec §17.
pub mod exit_codes {
    /// Success.
    pub const SUCCESS: u8 = 0;
    /// Generic error.
    pub const GENERIC_ERROR: u8 = 1;
    /// Lint validation failure.
    pub const LINT_FAILURE: u8 = 2;
    /// Run failures present.
    pub const RUN_FAILURE: u8 = 3;
    /// Gate failure.
    pub const GATE_FAILURE: u8 = 4;
    /// Discovery or permission failure.
    pub const DISCOVERY_FAILURE: u8 = 5;
}

fn main() -> ExitCode {
    let args = Cli::parse();

    match run_command(&args) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            if !args.quiet {
                eprintln!("error: {err}");
            }
            ExitCode::from(exit_codes::GENERIC_ERROR)
        }
    }
}

fn run_command(args: &Cli) -> anyhow::Result<u8> {
    match &args.command {
        Command::Lint => {
            let diagnostics = lint::lint_files(&args.suite, &args.inventory, &args.set);
            if diagnostics.is_empty() {
                if args.verbose && !args.quiet {
                    println!("lint ok");
                }
                Ok(exit_codes::SUCCESS)
            } else {
                for diag in &diagnostics {
                    eprintln!("{diag}");
                }
                Ok(exit_codes::LINT_FAILURE)
            }
        }
        Command::Plan { format } => {
            let diagnostics = lint::lint_files(&args.suite, &args.inventory, &args.set);
            if !diagnostics.is_empty() {
                for diag in &diagnostics {
                    eprintln!("{diag}");
                }
                return Ok(exit_codes::LINT_FAILURE);
            }
            let output = plan_cmd::execute_plan(&args.suite, &args.inventory, &args.set, *format)?;
            if !args.quiet {
                print!("{output}");
            }
            Ok(exit_codes::SUCCESS)
        }
        Command::Run {
            select,
            force,
            dry_run,
            probe,
        } => {
            let diagnostics = lint::lint_files(&args.suite, &args.inventory, &args.set);
            if !diagnostics.is_empty() {
                for diag in &diagnostics {
                    eprintln!("{diag}");
                }
                return Ok(exit_codes::LINT_FAILURE);
            }
            run_cmd::execute_run(
                &args.suite,
                &args.inventory,
                &args.results,
                &args.set,
                select.as_deref(),
                *force,
                *dry_run,
                probe.as_deref(),
                args.quiet,
            )
        }
        Command::Spec {
            connect,
            machine,
            out,
            merge,
            list_partitions,
            partitions,
            yes,
            probe,
        } => {
            let out_path = out.as_deref().unwrap_or(&args.inventory);
            spec_cmd::execute_spec(
                connect,
                machine,
                out_path,
                *merge,
                *list_partitions,
                partitions,
                *yes,
                probe.as_deref(),
                args.quiet,
            )
        }
        Command::Verify { machine, probe } => {
            spec_cmd::execute_verify(&args.inventory, machine, probe.as_deref(), args.quiet)
        }
    }
}
