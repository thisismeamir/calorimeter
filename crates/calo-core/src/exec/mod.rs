//! Run orchestration, `RunSpec` construction, template rendering, `Adapter` trait, status & cancel (spec §8.4, §9.9, §14, §15).

pub mod adapter;
pub mod error;
pub mod local;
pub mod runspec;
pub mod schedule;
pub mod status;

pub use adapter::{Adapter, JobHandle, JobState, StagedRun};
pub use error::ExecError;
pub use local::LocalAdapter;
pub use runspec::{
    build_launcher_argv, build_runspec, render_step_files, resolve_counter_aliases,
    resolve_probe_binary, subtract_isolated_cpus,
};
pub use schedule::{execute_suite_runs, order_runs, DryRunEntry};
pub use status::{
    cancel_runs, collect_status, format_status_table, CancelReport, StatusOutput, StatusRow,
    StatusSummary,
};
