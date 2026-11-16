//! Report pivot tables (CSV and Markdown) and regression gates (`calo-core::report`, spec §13).

pub mod error;
pub mod gate;
pub mod pivot;

pub use error::ReportError;
pub use gate::{evaluate_gate, evaluate_report_gates, GateOutcome};
pub use pivot::{build_pivot_table, select_records, PivotTable};
