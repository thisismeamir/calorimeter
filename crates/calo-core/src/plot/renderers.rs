//! Implementations of `line`, `bar`, `heatmap`, `scaling_plot`, `rank_heatmap`, `comm_matrix`,
//! `time_budget_bar`, and `region_tree` SVG renderers (spec §13 R-REP-3).

use crate::flame::xml_escape;
use crate::plot::error::PlotError;
use crate::plot::svg::{
    begin_svg, draw_marker, format_tick, nice_ticks, render_no_data_svg, AxisScale, ScaleKind,
    SERIES_STYLES,
};
use crate::services::Frame;
use calo_schema::record::RegionNode;
use calo_schema::ScalarValue;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Plot configuration passed to `render_plot` (spec §13).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlotSpec {
    /// Plot type (`line`, `bar`, `heatmap`, `scaling_plot`, `rank_heatmap`, `comm_matrix`, `time_budget_bar`, `region_tree`).
    pub plot_type: String,
    /// X-axis column name.
    pub x: Option<String>,
    /// Y-axis column name.
    pub y: Option<String>,
    /// Series grouping column (`line_by`).
    pub line_by: Option<String>,
    /// Facet grouping column (`facet_by`).
    pub facet_by: Option<String>,
    /// X-axis scale kind.
    pub x_scale: ScaleKind,
    /// Y-axis scale kind.
    pub y_scale: ScaleKind,
    /// Error bar column mode (e.g. `"ci95"` -> reads `ci_low` and `ci_high`).
    pub errorbars: Option<String>,
    /// Plot title.
    pub title: Option<String>,
    /// Optional color scale mode (e.g. `"log"` for `comm_matrix`).
    pub color_scale: ScaleKind,
}

/// Dispatches a `PlotSpec` and `Frame` to the corresponding SVG renderer (spec §13 R-REP-3).
pub fn render_plot(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    match spec.plot_type.as_str() {
        "line" => render_line(spec, frame),
        "bar" => render_bar(spec, frame),
        "heatmap" => render_heatmap(spec, frame),
        "scaling_plot" => render_scaling_plot(spec, frame),
        "rank_heatmap" => render_rank_heatmap(spec, frame),
        "comm_matrix" => render_comm_matrix(spec, frame),
        "time_budget_bar" => render_time_budget_bar(spec, frame),
        "roofline" | "timeline" | "gpu_timeline" => Err(PlotError::NotYetImplemented {
            plot_type: spec.plot_type.clone(),
        }),
        other => Err(PlotError::UnknownPlotType {
            plot_type: other.to_string(),
        }),
    }
}

fn axis_label_with_unit(col: &str, frame: &Frame) -> String {
    if let Some(ScalarValue::Str(u)) = frame.meta(&format!("{col}.unit")) {
        format!("{col} ({u})")
    } else {
        col.to_string()
    }
}

/// Renders a `line` plot with optional `line_by`, `facet_by` small multiples, log/log2 scales, and `ci95` error bars.
pub fn render_line(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    let x_col = spec.x.as_deref().unwrap_or("x");
    let y_col = spec.y.as_deref().unwrap_or("y");

    if frame.num_rows() == 0 {
        return Ok(render_no_data_svg(spec.title.as_deref(), 640.0, 400.0));
    }

    let xs = frame
        .column(x_col)
        .ok_or_else(|| PlotError::MissingColumn {
            plot_type: "line".to_string(),
            column: x_col.to_string(),
        })?
        .to_f64_vec(x_col)
        .map_err(|_| PlotError::MissingColumn {
            plot_type: "line".to_string(),
            column: x_col.to_string(),
        })?;
    let ys = frame
        .column(y_col)
        .ok_or_else(|| PlotError::MissingColumn {
            plot_type: "line".to_string(),
            column: y_col.to_string(),
        })?
        .to_f64_vec(y_col)
        .map_err(|_| PlotError::MissingColumn {
            plot_type: "line".to_string(),
            column: y_col.to_string(),
        })?;

    let ci_low = if spec.errorbars.as_deref() == Some("ci95") {
        frame
            .column("ci_low")
            .and_then(|c| c.to_f64_vec("ci_low").ok())
    } else {
        None
    };
    let ci_high = if spec.errorbars.as_deref() == Some("ci95") {
        frame
            .column("ci_high")
            .and_then(|c| c.to_f64_vec("ci_high").ok())
    } else {
        None
    };

    // Group rows into facets -> series
    #[derive(Clone)]
    struct Pt {
        x: f64,
        y: f64,
        lo: Option<f64>,
        hi: Option<f64>,
    }

    let mut facets: BTreeMap<String, BTreeMap<String, Vec<Pt>>> = BTreeMap::new();
    let mut all_series_names = BTreeSet::new();

    for i in 0..frame.num_rows() {
        let f_key = spec
            .facet_by
            .as_deref()
            .and_then(|fc| frame.column(fc))
            .and_then(|c| c.get_value(i))
            .map(|v| v.to_string())
            .unwrap_or_else(|| "_all".to_string());
        let s_key = spec
            .line_by
            .as_deref()
            .and_then(|lc| frame.column(lc))
            .and_then(|c| c.get_value(i))
            .map(|v| v.to_string())
            .unwrap_or_else(|| y_col.to_string());

        all_series_names.insert(s_key.clone());
        facets
            .entry(f_key)
            .or_default()
            .entry(s_key)
            .or_default()
            .push(Pt {
                x: xs[i],
                y: ys[i],
                lo: ci_low.as_ref().and_then(|v| v.get(i).copied()),
                hi: ci_high.as_ref().and_then(|v| v.get(i).copied()),
            });
    }

    // Shared domain bounds across all facets
    let x_min = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let x_max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let y_min = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let y_max = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    let n_facets = facets.len().max(1);
    let cols_grid = n_facets.min(3);
    let rows_grid = n_facets.div_ceil(cols_grid);

    let panel_w = 420.0;
    let panel_h = 280.0;
    let header_h = if spec.title.is_some() { 36.0 } else { 16.0 };
    let legend_h = 28.0;
    let total_w = (cols_grid as f64) * panel_w;
    let total_h = header_h + (rows_grid as f64) * panel_h + legend_h;

    let mut svg = begin_svg(total_w, total_h);
    if let Some(title) = &spec.title {
        let _ = writeln!(
            svg,
            r#"<text x="16" y="24" font-size="14" font-weight="600">{}</text>"#,
            xml_escape(title)
        );
    }

    let x_label = axis_label_with_unit(x_col, frame);
    let y_label = axis_label_with_unit(y_col, frame);
    let series_order: Vec<String> = all_series_names.into_iter().collect();

    for (f_idx, (f_name, series_map)) in facets.iter().enumerate() {
        let gx = (f_idx % cols_grid) as f64 * panel_w;
        let gy = header_h + (f_idx / cols_grid) as f64 * panel_h;

        let ml = 62.0;
        let mr = 18.0;
        let mt = 24.0;
        let mb = 44.0;
        let plot_x0 = gx + ml;
        let plot_x1 = gx + panel_w - mr;
        let plot_y0 = gy + mt;
        let plot_y1 = gy + panel_h - mb;

        let first_series = series_map
            .keys()
            .next()
            .map(String::as_str)
            .unwrap_or("series");
        let x_scale = AxisScale::new(
            spec.x_scale,
            x_min,
            x_max,
            plot_x0,
            plot_x1,
            first_series,
            "x",
        )?;
        let y_scale = AxisScale::new(
            spec.y_scale,
            y_min,
            y_max,
            plot_y1,
            plot_y0,
            first_series,
            "y",
        )?;

        if f_name != "_all" {
            let _ = writeln!(
                svg,
                r#"<text x="{:.2}" y="{:.2}" font-size="12" font-weight="600">{}</text>"#,
                plot_x0,
                gy + 16.0,
                xml_escape(f_name)
            );
        }

        // Gridlines & ticks
        for tx in nice_ticks(x_min, x_max, 5) {
            if let Ok(px) = x_scale.map(tx, first_series, "x") {
                if px >= plot_x0 - 1.0 && px <= plot_x1 + 1.0 {
                    let _ = writeln!(
                        svg,
                        r##"<line x1="{px:.2}" y1="{plot_y0:.2}" x2="{px:.2}" y2="{plot_y1:.2}" stroke="#e0e0e0" stroke-width="1"/>"##
                    );
                    let _ = writeln!(
                        svg,
                        r#"<text x="{px:.2}" y="{:.2}" text-anchor="middle" font-size="10">{}</text>"#,
                        plot_y1 + 14.0,
                        format_tick(tx)
                    );
                }
            }
        }
        for ty in nice_ticks(y_min, y_max, 5) {
            if let Ok(py) = y_scale.map(ty, first_series, "y") {
                if py >= plot_y0 - 1.0 && py <= plot_y1 + 1.0 {
                    let _ = writeln!(
                        svg,
                        r##"<line x1="{plot_x0:.2}" y1="{py:.2}" x2="{plot_x1:.2}" y2="{py:.2}" stroke="#e0e0e0" stroke-width="1"/>"##
                    );
                    let _ = writeln!(
                        svg,
                        r#"<text x="{:.2}" y="{:.2}" text-anchor="end" font-size="10">{}</text>"#,
                        plot_x0 - 6.0,
                        py + 3.5,
                        format_tick(ty)
                    );
                }
            }
        }

        // Axes
        let _ = writeln!(
            svg,
            r##"<polyline points="{plot_x0:.2},{plot_y0:.2} {plot_x0:.2},{plot_y1:.2} {plot_x1:.2},{plot_y1:.2}" fill="none" stroke="#161616" stroke-width="1"/>"##
        );
        let _ = writeln!(
            svg,
            r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" font-size="11">{}</text>"#,
            0.5 * (plot_x0 + plot_x1),
            plot_y1 + 32.0,
            xml_escape(&x_label)
        );
        let _ = writeln!(
            svg,
            r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" font-size="11" transform="rotate(-90 {:.2} {:.2})">{}</text>"#,
            gx + 15.0,
            0.5 * (plot_y0 + plot_y1),
            gx + 15.0,
            0.5 * (plot_y0 + plot_y1),
            xml_escape(&y_label)
        );

        // Plot each series
        for (s_name, pts_raw) in series_map {
            let s_idx = series_order.iter().position(|n| n == s_name).unwrap_or(0);
            let style = SERIES_STYLES[s_idx % SERIES_STYLES.len()];

            let mut pts = pts_raw.clone();
            pts.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));

            let mut poly_pts = Vec::with_capacity(pts.len());
            for pt in &pts {
                let px = x_scale.map(pt.x, s_name, "x")?;
                let py = y_scale.map(pt.y, s_name, "y")?;
                poly_pts.push((px, py, pt.lo, pt.hi));
            }

            let pts_attr: String = poly_pts
                .iter()
                .map(|(px, py, _, _)| format!("{px:.2},{py:.2}"))
                .collect::<Vec<_>>()
                .join(" ");
            let dash_attr = if style.dash.is_empty() {
                String::new()
            } else {
                format!(r#" stroke-dasharray="{}""#, style.dash)
            };
            let _ = writeln!(
                svg,
                r#"<polyline points="{pts_attr}" fill="none" stroke="{}" stroke-width="1.5"{dash_attr}/>"#,
                style.color
            );

            for (px, py, lo_opt, hi_opt) in poly_pts {
                if let (Some(lo), Some(hi)) = (lo_opt, hi_opt) {
                    let y_lo = y_scale.map_clamped(lo);
                    let y_hi = y_scale.map_clamped(hi);
                    let _ = writeln!(
                        svg,
                        r#"<line x1="{px:.2}" y1="{y_lo:.2}" x2="{px:.2}" y2="{y_hi:.2}" stroke="{}" stroke-width="1"/>"#,
                        style.color
                    );
                }
                draw_marker(&mut svg, style.marker, px, py, 3.2, style.color);
            }
        }
    }

    // Flat rectangular legend
    let ly = header_h + (rows_grid as f64) * panel_h + 6.0;
    let mut lx = 62.0;
    for (s_idx, s_name) in series_order.iter().enumerate() {
        let style = SERIES_STYLES[s_idx % SERIES_STYLES.len()];
        let _ = writeln!(
            svg,
            r##"<rect x="{lx:.2}" y="{ly:.2}" width="12" height="10" fill="{}" stroke="#161616" stroke-width="1"/>"##,
            style.color
        );
        let _ = writeln!(
            svg,
            r#"<text x="{:.2}" y="{:.2}" font-size="11">{}</text>"#,
            lx + 16.0,
            ly + 9.0,
            xml_escape(s_name)
        );
        lx += 28.0 + (s_name.len() as f64) * 6.5;
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

/// Renders a grouped categorical `bar` chart (`x`, `y`, optional `line_by`).
pub fn render_bar(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    let x_col = spec.x.as_deref().unwrap_or("x");
    let y_col = spec.y.as_deref().unwrap_or("y");
    if frame.num_rows() == 0 {
        return Ok(render_no_data_svg(spec.title.as_deref(), 600.0, 360.0));
    }

    let x_data = frame
        .column(x_col)
        .ok_or_else(|| PlotError::MissingColumn {
            plot_type: "bar".to_string(),
            column: x_col.to_string(),
        })?;
    let ys = frame
        .column(y_col)
        .ok_or_else(|| PlotError::MissingColumn {
            plot_type: "bar".to_string(),
            column: y_col.to_string(),
        })?
        .to_f64_vec(y_col)
        .map_err(|_| PlotError::MissingColumn {
            plot_type: "bar".to_string(),
            column: y_col.to_string(),
        })?;

    let mut cats = Vec::new();
    let mut vals: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut series_set = BTreeSet::new();

    for (i, item) in ys.iter().enumerate().take(frame.num_rows()) {
        let cat = x_data
            .get_value(i)
            .map(|v| v.to_string())
            .unwrap_or_default();
        if !cats.contains(&cat) {
            cats.push(cat.clone());
        }
        let s_name = spec
            .line_by
            .as_deref()
            .and_then(|lc| frame.column(lc))
            .and_then(|c| c.get_value(i))
            .map(|v| v.to_string())
            .unwrap_or_else(|| y_col.to_string());
        series_set.insert(s_name.clone());
        vals.insert((cat, s_name), *item);
    }

    let y_max = ys.iter().copied().fold(0.0f64, f64::max).max(1e-9);
    let series_list: Vec<String> = series_set.into_iter().collect();
    let n_series = series_list.len().max(1);

    let width = 620.0;
    let height = 380.0;
    let ml = 64.0;
    let mr = 20.0;
    let mt = if spec.title.is_some() { 44.0 } else { 20.0 };
    let mb = 60.0;
    let pw = width - ml - mr;
    let y_scale = AxisScale::new(ScaleKind::Linear, 0.0, y_max, height - mb, mt, "y", "y")?;

    let mut svg = begin_svg(width, height);
    if let Some(title) = &spec.title {
        let _ = writeln!(
            svg,
            r#"<text x="16" y="24" font-size="14" font-weight="600">{}</text>"#,
            xml_escape(title)
        );
    }

    let y_axis_px = height - mb;
    for ty in nice_ticks(0.0, y_max, 5) {
        if let Ok(py) = y_scale.map(ty, "y", "y") {
            let _ = writeln!(
                svg,
                r##"<line x1="{ml:.2}" y1="{py:.2}" x2="{:.2}" y2="{py:.2}" stroke="#e0e0e0" stroke-width="1"/>"##,
                width - mr
            );
            let _ = writeln!(
                svg,
                r#"<text x="{:.2}" y="{:.2}" text-anchor="end" font-size="10">{}</text>"#,
                ml - 6.0,
                py + 3.5,
                format_tick(ty)
            );
        }
    }

    let group_w = pw / (cats.len().max(1) as f64);
    let bar_w = ((group_w * 0.75) / (n_series as f64)).max(4.0);

    for (c_idx, cat) in cats.iter().enumerate() {
        let gx = ml + (c_idx as f64) * group_w + group_w * 0.125;
        for (s_idx, s_name) in series_list.iter().enumerate() {
            if let Some(&v) = vals.get(&(cat.clone(), s_name.clone())) {
                let bx = gx + (s_idx as f64) * bar_w;
                let by = y_scale.map(v.max(0.0), s_name, "y")?;
                let bh = (y_axis_px - by).max(0.0);
                let color = SERIES_STYLES[s_idx % SERIES_STYLES.len()].color;
                let _ = writeln!(
                    svg,
                    r##"<rect x="{bx:.2}" y="{by:.2}" width="{bar_w:.2}" height="{bh:.2}" fill="{color}" stroke="#161616" stroke-width="1"/>"##
                );
            }
        }
        let cx = ml + (c_idx as f64 + 0.5) * group_w;
        let _ = writeln!(
            svg,
            r#"<text x="{cx:.2}" y="{:.2}" text-anchor="middle" font-size="10">{}</text>"#,
            y_axis_px + 15.0,
            xml_escape(cat)
        );
    }

    let _ = writeln!(
        svg,
        r##"<polyline points="{ml:.2},{mt:.2} {ml:.2},{y_axis_px:.2} {:.2},{y_axis_px:.2}" fill="none" stroke="#161616" stroke-width="1"/>"##,
        width - mr
    );

    svg.push_str("</svg>\n");
    Ok(svg)
}

/// Renders a 2D numeric `heatmap` with monochrome-to-accent `#7017FF` ramp and cell values for small grids.
pub fn render_heatmap(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    let x_col = spec.x.as_deref().unwrap_or("x");
    let y_col = spec.y.as_deref().unwrap_or("y");
    let v_col = "value";
    render_grid_heatmap(
        spec.title.as_deref(),
        x_col,
        y_col,
        v_col,
        frame,
        spec.color_scale,
    )
}

/// Renders `rank_heatmap` (`rank` vs metric).
pub fn render_rank_heatmap(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    let x_col = spec.x.as_deref().unwrap_or("rank");
    let y_col = spec.y.as_deref().unwrap_or("metric");
    render_grid_heatmap(
        spec.title.as_deref(),
        x_col,
        y_col,
        "value",
        frame,
        spec.color_scale,
    )
}

/// Renders `comm_matrix` (`src_rank`, `dst_rank`, `bytes` or `msgs`, optional log color).
pub fn render_comm_matrix(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    let v_col = if frame.column("bytes").is_some() {
        "bytes"
    } else {
        "msgs"
    };
    render_grid_heatmap(
        spec.title.as_deref().or(Some("Communication Matrix")),
        "dst_rank",
        "src_rank",
        v_col,
        frame,
        spec.color_scale,
    )
}

fn render_grid_heatmap(
    title: Option<&str>,
    x_col: &str,
    y_col: &str,
    val_col: &str,
    frame: &Frame,
    color_scale: ScaleKind,
) -> Result<String, PlotError> {
    if frame.num_rows() == 0 {
        return Ok(render_no_data_svg(title, 520.0, 420.0));
    }
    let xc = frame
        .column(x_col)
        .ok_or_else(|| PlotError::MissingColumn {
            plot_type: "heatmap".to_string(),
            column: x_col.to_string(),
        })?;
    let yc = frame
        .column(y_col)
        .ok_or_else(|| PlotError::MissingColumn {
            plot_type: "heatmap".to_string(),
            column: y_col.to_string(),
        })?;
    let vc = frame
        .column(val_col)
        .ok_or_else(|| PlotError::MissingColumn {
            plot_type: "heatmap".to_string(),
            column: val_col.to_string(),
        })?
        .to_f64_vec(val_col)
        .map_err(|_| PlotError::MissingColumn {
            plot_type: "heatmap".to_string(),
            column: val_col.to_string(),
        })?;

    let mut x_keys = Vec::new();
    let mut y_keys = Vec::new();
    let mut grid: BTreeMap<(String, String), f64> = BTreeMap::new();

    for (i, item) in vc.iter().enumerate().take(frame.num_rows()) {
        let xk = xc.get_value(i).map(|v| v.to_string()).unwrap_or_default();
        let yk = yc.get_value(i).map(|v| v.to_string()).unwrap_or_default();
        if !x_keys.contains(&xk) {
            x_keys.push(xk.clone());
        }
        if !y_keys.contains(&yk) {
            y_keys.push(yk.clone());
        }
        grid.insert((yk, xk), *item);
    }

    let mut t_vals = Vec::with_capacity(vc.len());
    for &v in &vc {
        let tv = match color_scale {
            ScaleKind::Linear => v,
            ScaleKind::Log | ScaleKind::Log2 => {
                if v < 0.0 {
                    return Err(PlotError::NonPositiveLogValue {
                        series: val_col.to_string(),
                        axis: "color",
                        value: v,
                    });
                }
                (v + 1.0).log10()
            }
        };
        t_vals.push(tv);
    }

    let t_min = t_vals.iter().copied().fold(f64::INFINITY, f64::min);
    let t_max = t_vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (t_max - t_min).max(1e-12);

    let cell_w = 46.0;
    let cell_h = 34.0;
    let ml = 70.0;
    let mt = if title.is_some() { 44.0 } else { 20.0 };
    let width = ml + (x_keys.len() as f64) * cell_w + 24.0;
    let height = mt + (y_keys.len() as f64) * cell_h + 44.0;

    let mut svg = begin_svg(width, height);
    if let Some(t) = title {
        let _ = writeln!(
            svg,
            r#"<text x="16" y="24" font-size="14" font-weight="600">{}</text>"#,
            xml_escape(t)
        );
    }

    let show_text = x_keys.len() <= 12 && y_keys.len() <= 12;

    for (r, yk) in y_keys.iter().enumerate() {
        let cy = mt + (r as f64) * cell_h;
        let _ = writeln!(
            svg,
            r#"<text x="{:.2}" y="{:.2}" text-anchor="end" font-size="10">{}</text>"#,
            ml - 6.0,
            cy + cell_h * 0.62,
            xml_escape(yk)
        );
        for (c, xk) in x_keys.iter().enumerate() {
            let cx = ml + (c as f64) * cell_w;
            if let Some(&raw_v) = grid.get(&(yk.clone(), xk.clone())) {
                let tv = match color_scale {
                    ScaleKind::Linear => raw_v,
                    ScaleKind::Log | ScaleKind::Log2 => (raw_v + 1.0).log10(),
                };
                let norm = ((tv - t_min) / span).clamp(0.0, 1.0);
                let fill = ramp_grey_to_accent(norm);
                let txt_c = if norm > 0.65 { "#ffffff" } else { "#161616" };
                let _ = writeln!(
                    svg,
                    r##"<rect x="{cx:.2}" y="{cy:.2}" width="{cell_w:.2}" height="{cell_h:.2}" fill="{fill}" stroke="#161616" stroke-width="1"/>"##
                );
                if show_text {
                    let _ = writeln!(
                        svg,
                        r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" font-size="10" fill="{txt_c}">{}</text>"#,
                        cx + cell_w * 0.5,
                        cy + cell_h * 0.62,
                        format_tick(raw_v)
                    );
                }
            }
        }
    }

    for (c, xk) in x_keys.iter().enumerate() {
        let cx = ml + (c as f64 + 0.5) * cell_w;
        let _ = writeln!(
            svg,
            r#"<text x="{cx:.2}" y="{:.2}" text-anchor="middle" font-size="10">{}</text>"#,
            mt + (y_keys.len() as f64) * cell_h + 15.0,
            xml_escape(xk)
        );
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

fn ramp_grey_to_accent(norm: f64) -> String {
    // Interpolate #e0e0e0 (224,224,224) -> #7017FF (112,23,255)
    let r = (224.0 + norm * (112.0 - 224.0)).round() as u8;
    let g = (224.0 + norm * (23.0 - 224.0)).round() as u8;
    let b = (224.0 + norm * (255.0 - 224.0)).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Renders `scaling_plot` (efficiency vs P) with a dashed ideal `1.0` line and `#7017FF` accent on the measured series.
pub fn render_scaling_plot(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    let mut s_spec = spec.clone();
    if s_spec.x.is_none() {
        s_spec.x = Some("ranks".to_string());
    }
    if s_spec.y.is_none() {
        s_spec.y = Some("parallel_efficiency".to_string());
    }
    if s_spec.title.is_none() {
        s_spec.title = Some("Parallel Scaling Efficiency".to_string());
    }

    let mut base_svg = render_line(&s_spec, frame)?;
    // Inject horizontal dashed ideal efficiency=1.0 reference line before </svg>
    if frame.num_rows() > 0 {
        let marker = "</svg>";
        if let Some(pos) = base_svg.rfind(marker) {
            let ideal_line = r##"<line x1="62" y1="60" x2="402" y2="60" stroke="#525252" stroke-width="1" stroke-dasharray="4,4"/><text x="398" y="55" text-anchor="end" font-size="9" fill="#525252">ideal (1.0)</text>"##;
            base_svg.insert_str(pos, &format!("{ideal_line}\n"));
        }
    }
    Ok(base_svg)
}

/// Renders `time_budget_bar` with stacked categories and `#7017FF` accent on the largest non-compute category.
pub fn render_time_budget_bar(spec: &PlotSpec, frame: &Frame) -> Result<String, PlotError> {
    let categories = [
        "cpu_compute_s",
        "gpu_compute_s",
        "mpi_wait_s",
        "mpi_transfer_s",
        "host_device_s",
        "idle_s",
    ];
    let labels = [
        "CPU compute",
        "GPU compute",
        "MPI wait",
        "MPI transfer",
        "Host-device transfer",
        "Idle",
    ];

    if frame.num_rows() == 0 {
        return Ok(render_no_data_svg(spec.title.as_deref(), 600.0, 220.0));
    }

    let mut totals = [0.0f64; 6];
    for (idx, col_name) in categories.iter().enumerate() {
        if let Some(col) = frame.column(col_name) {
            if let Ok(vs) = col.to_f64_vec(col_name) {
                totals[idx] = vs.iter().sum();
            }
        }
    }
    let grand_total: f64 = totals.iter().sum();
    if grand_total <= 0.0 {
        return Ok(render_no_data_svg(spec.title.as_deref(), 600.0, 220.0));
    }

    // Largest non-compute category (indices 2..6) gets accent `#7017FF`
    let max_non_compute_idx = (2..6)
        .max_by(|&a, &b| {
            totals[a]
                .partial_cmp(&totals[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(2);
    let greys = ["#161616", "#525252", "#8d8d8d", "#c6c6c6", "#e0e0e0"];

    let width = 640.0;
    let height = 160.0;
    let mut svg = begin_svg(width, height);
    if let Some(title) = &spec.title {
        let _ = writeln!(
            svg,
            r#"<text x="20" y="24" font-size="14" font-weight="600">{}</text>"#,
            xml_escape(title)
        );
    }

    let bar_x = 20.0;
    let bar_y = 48.0;
    let bar_w = 600.0;
    let bar_h = 34.0;
    let mut cur_x = bar_x;

    for idx in 0..6 {
        if totals[idx] <= 0.0 {
            continue;
        }
        let seg_w = bar_w * (totals[idx] / grand_total);
        let fill = if idx == max_non_compute_idx {
            "#7017FF"
        } else {
            greys[idx % greys.len()]
        };
        let _ = writeln!(
            svg,
            r##"<g><title>{}: {:.3}s ({:.1}%)</title><rect x="{cur_x:.2}" y="{bar_y:.2}" width="{seg_w:.2}" height="{bar_h:.2}" fill="{fill}" stroke="#161616" stroke-width="1"/></g>"##,
            labels[idx],
            totals[idx],
            (totals[idx] / grand_total) * 100.0
        );
        cur_x += seg_w;
    }

    // Legend row
    let mut lx = 20.0;
    let ly = 104.0;
    for idx in 0..6 {
        if totals[idx] <= 0.0 {
            continue;
        }
        let fill = if idx == max_non_compute_idx {
            "#7017FF"
        } else {
            greys[idx % greys.len()]
        };
        let _ = writeln!(
            svg,
            r##"<rect x="{lx:.2}" y="{ly:.2}" width="10" height="10" fill="{fill}" stroke="#161616" stroke-width="1"/><text x="{:.2}" y="{:.2}" font-size="10">{}</text>"##,
            lx + 14.0,
            ly + 9.0,
            labels[idx]
        );
        lx += 24.0 + (labels[idx].len() as f64) * 5.8;
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

/// Renders a horizontal `region_tree` icicle diagram from `RegionNode`s (spec §13 R-REP-3).
pub fn render_region_tree(title: Option<&str>, tree: &[RegionNode]) -> String {
    if tree.is_empty() {
        return render_no_data_svg(title, 800.0, 200.0);
    }
    let total_s: f64 = tree.iter().map(|n| n.incl_s).sum();
    if total_s <= 0.0 {
        return render_no_data_svg(title, 800.0, 200.0);
    }

    fn max_d(nodes: &[RegionNode]) -> usize {
        nodes
            .iter()
            .map(|n| 1 + max_d(&n.children))
            .max()
            .unwrap_or(0)
    }

    let depth = max_d(tree).max(1);
    let width = 800.0;
    let row_h = 22.0;
    let top = if title.is_some() { 38.0 } else { 16.0 };
    let height = top + (depth as f64) * row_h + 20.0;

    let mut svg = begin_svg(width, height);
    if let Some(t) = title {
        let _ = writeln!(
            svg,
            r#"<text x="16" y="24" font-size="14" font-weight="600">{}</text>"#,
            xml_escape(t)
        );
    }
    #[allow(clippy::too_many_arguments)]
    fn draw_reg(
        svg: &mut String,
        nodes: &[RegionNode],
        d: usize,
        x0: f64,
        w_parent: f64,
        parent_s: f64,
        top: f64,
        row_h: f64,
    ) {
        let mut cx = x0;
        let greys = ["#e0e0e0", "#c6c6c6", "#8d8d8d", "#525252"];
        for n in nodes {
            let w = w_parent * (n.incl_s / parent_s.max(1e-12));
            if w < 1.0 {
                continue;
            }
            let y = top + (d as f64) * row_h;
            let fill = if d == 0 {
                "#7017FF"
            } else {
                greys[d % greys.len()]
            };
            let txt = if d == 0 || fill == "#525252" {
                "#ffffff"
            } else {
                "#161616"
            };
            let _ = writeln!(
                svg,
                r##"<g><title>{}: incl {:.4}s, excl {:.4}s ({} calls)</title><rect x="{cx:.2}" y="{y:.2}" width="{w:.2}" height="{:.2}" fill="{fill}" stroke="#161616" stroke-width="1"/>"##,
                xml_escape(&n.name),
                n.incl_s,
                n.excl_s,
                n.calls,
                row_h - 2.0
            );
            if w >= 36.0 {
                let _ = writeln!(
                    svg,
                    r#"<text x="{:.2}" y="{:.2}" font-size="10" fill="{txt}">{}</text>"#,
                    cx + 4.0,
                    y + 13.0,
                    xml_escape(&n.name)
                );
            }
            svg.push_str("</g>\n");
            if !n.children.is_empty() && n.incl_s > 0.0 {
                draw_reg(svg, &n.children, d + 1, cx, w, n.incl_s, top, row_h);
            }
            cx += w;
        }
    }

    draw_reg(&mut svg, tree, 0, 16.0, width - 32.0, total_s, top, row_h);
    svg.push_str("</svg>\n");
    svg
}
