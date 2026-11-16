//! Selectors, benchmark matrix expansion, and resource cost estimation (spec §5.3, §8.5).

pub mod cost;
pub mod error;
pub mod expand;
pub mod selector;

pub use cost::{estimate_cost, parse_timeout_hours, Cost};
pub use error::PlanError;
pub use expand::{expand_suite, format_run_id, PinnedRun};
pub use selector::{matches, RunDescriptor, Selector};
