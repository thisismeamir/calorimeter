//! Folded call-stack ingestion, `FlameTree` construction, and static SVG flamegraph rendering (spec §13 R-REP-3, R-REP-6).

pub mod error;
pub mod render;
pub mod stack;

pub use error::FlameError;
pub use render::{render_diff_flamegraph, render_flamegraph, xml_escape, FlameOpts};
pub use stack::{from_folded, to_folded, FlameNode, FlameTree, StackRow, StackTable};
