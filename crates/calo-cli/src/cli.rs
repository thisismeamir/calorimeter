//! Clap command-line argument definitions (spec §14).

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Calorimeter (`calo`) benchmarking and profiling CLI (spec §14).
#[derive(Debug, Parser)]
#[command(name = "calo", version, about = "Calorimeter benchmarking and profiling system")]
pub struct Cli {
    /// Path to `suite.yaml` (spec §14).
    #[arg(long, global = true, default_value = "suite.yaml")]
    pub suite: PathBuf,

    /// Path to `inventory.yaml` (spec §14).
    #[arg(long, global = true, default_value = "inventory.yaml")]
    pub inventory: PathBuf,

    /// Override configuration values via dotted paths (`key.path=value`, spec §5.1, §14).
    #[arg(long = "set", global = true)]
    pub set: Vec<String>,

    /// Results output directory (spec §14).
    #[arg(long, global = true, default_value = "results")]
    pub results: PathBuf,

    /// Suppress non-error output (spec §14).
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,

    /// Enable verbose output (spec §14).
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Command,
}

/// Supported CLI subcommands in Phase 1 (spec §14, §20).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate `suite.yaml` and `inventory.yaml` against schemas and cross-references (spec §14).
    Lint,
    /// Print expanded runs, upper-bound node-hours, and GPU-hours (spec §8.5 R-BEN-2, §14).
    Plan {
        /// Output format (`table` or `yaml`, spec §14).
        #[arg(long, default_value = "table")]
        format: PlanFormat,
    },
}

/// Output format for `calo plan` (spec §14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PlanFormat {
    /// Human-readable ASCII table plus cost summary.
    Table,
    /// YAML serialization of `PinnedRun`s and cost summary.
    Yaml,
}
