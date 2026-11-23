//! Results directory layout, atomic manifest writer, and resume planning (spec §4, §14, §15, §18).

pub mod error;
pub mod store;

pub use error::ResultsError;
pub use store::{
    compute_suite_hash, is_valid_record, plan_resume, update_manifest_run, write_atomic_bytes,
    RecordValidity, ResultsDir, SidecarKind,
};
