//! Static monochrome/accent SVG plot renderers (spec §13 R-REP-3, R-REP-6, R-REP-7).

pub mod error;
pub mod renderers;
pub mod svg;

pub use error::PlotError;
pub use renderers::{
    render_bar, render_comm_matrix, render_heatmap, render_line, render_plot, render_rank_heatmap,
    render_region_tree, render_scaling_plot, render_time_budget_bar, PlotSpec,
};
pub use svg::{nice_ticks, AxisScale, MarkerShape, ScaleKind, SERIES_STYLES};
