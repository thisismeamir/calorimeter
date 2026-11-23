//! Report pivot tables (CSV/Markdown), regression gates, analysis pipeline, HTML/MD rendering, and diff (spec §12, §13, §14).

pub mod error;
pub mod gate;
pub mod pipeline;
pub mod pivot;

pub use error::ReportError;
pub use gate::{evaluate_gate, evaluate_report_gates, GateOutcome};
pub use pipeline::{
    build_frame_from_records, diff_results_dirs, generate_report_artifacts, run_analysis_pipeline,
    AnalysisReport, DiffRow, GeneratedReport, ProvenanceFooter,
};
pub use pivot::{build_pivot_table, select_records, PivotTable};
