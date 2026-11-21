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

/// Supported CLI subcommands (spec §14).
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
    /// Execute expanded runs (resumable, spec §14, §15).
    Run {
        /// Filter runs by benchmark name or glob selector (spec §14).
        #[arg(long)]
        select: Option<String>,
        /// Re-run all selected runs even if valid records already exist (spec §14).
        #[arg(long)]
        force: bool,
        /// Print exact argv, runspec path, and env for every run without executing.
        #[arg(long)]
        dry_run: bool,
        /// Explicit path to the `probe` binary.
        #[arg(long)]
        probe: Option<PathBuf>,
    },
    /// Discover machine hardware and write/merge `inventory.yaml` (spec §6).
    Spec {
        /// Connection target (`local`, `ssh://[user@]host`, or `slurm://login`, spec §6.1).
        #[arg(long, default_value = "local")]
        connect: String,
        /// Machine name in `inventory.yaml`.
        #[arg(long, default_value = "local")]
        machine: String,
        /// Output inventory path (defaults to `--inventory`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Merge discovered hardware into existing inventory without touching user keys (spec §6.4).
        #[arg(long)]
        merge: bool,
        /// Passive read-only Slurm partition listing (spec §6.2 R-SPC-2).
        #[arg(long)]
        list_partitions: bool,
        /// Named Slurm partitions to probe (spec §6.2 R-SPC-1).
        #[arg(long = "partition")]
        partitions: Vec<String>,
        /// Confirm submission of Slurm discovery jobs non-interactively (spec §6.2 R-SPC-4).
        #[arg(long)]
        yes: bool,
        /// Explicit path to the `probe` binary.
        #[arg(long)]
        probe: Option<PathBuf>,
    },
    /// Verify machine fingerprint against `inventory.yaml` (spec §7 R-INV-5, §14).
    Verify {
        /// Machine name in `inventory.yaml` to verify.
        #[arg(long, default_value = "local")]
        machine: String,
        /// Explicit path to the `probe` binary.
        #[arg(long)]
        probe: Option<PathBuf>,
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
