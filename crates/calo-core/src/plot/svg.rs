//! Shared monochrome/accent SVG primitives, scales, ticks, axes, and legends (spec §13 R-REP-6).

use crate::flame::xml_escape;
use crate::plot::error::PlotError;
use std::fmt::Write as _;

/// Axis scale transformation (spec §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleKind {
    /// Linear scale.
    #[default]
    Linear,
    /// Base-10 logarithmic scale.
    Log,
    /// Base-2 logarithmic scale.
    Log2,
}

impl ScaleKind {
    /// Parses `"linear"`, `"log"`, or `"log2"`.
    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s {
            Some("log") | Some("log10") => Self::Log,
            Some("log2") => Self::Log2,
            _ => Self::Linear,
        }
    }

    /// Transforms a domain value into linear plot space.
    pub fn transform(self, v: f64, series: &str, axis: &'static str) -> Result<f64, PlotError> {
        match self {
            Self::Linear => Ok(v),
            Self::Log => {
                if v <= 0.0 || !v.is_finite() {
                    return Err(PlotError::NonPositiveLogValue {
                        series: series.to_string(),
                        axis,
                        value: v,
                    });
                }
                Ok(v.log10())
            }
            Self::Log2 => {
                if v <= 0.0 || !v.is_finite() {
                    return Err(PlotError::NonPositiveLogValue {
                        series: series.to_string(),
                        axis,
                        value: v,
                    });
                }
                Ok(v.log2())
            }
        }
    }
}

/// Marker shape for distinguishing series without relying solely on color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerShape {
    /// Filled circle.
    Circle,
    /// Filled square.
    Square,
    /// Filled upward triangle.
    Triangle,
    /// Filled diamond.
    Diamond,
}

/// Styling tuple for a plotted series (color, stroke-dasharray, marker shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeriesStyle {
    /// Hex fill/stroke color.
    pub color: &'static str,
    /// SVG `stroke-dasharray` (`""` for solid).
    pub dash: &'static str,
    /// Point marker shape.
    pub marker: MarkerShape,
}

/// Colorblind-safe monochrome + `#7017FF` accent series palette (Task 19 figure style).
pub const SERIES_STYLES: &[SeriesStyle] = &[
    SeriesStyle {
        color: "#7017FF",
        dash: "",
        marker: MarkerShape::Circle,
    },
    SeriesStyle {
        color: "#161616",
        dash: "6,3",
        marker: MarkerShape::Square,
    },
    SeriesStyle {
        color: "#525252",
        dash: "2,2",
        marker: MarkerShape::Triangle,
    },
    SeriesStyle {
        color: "#8d8d8d",
        dash: "8,3,2,3",
        marker: MarkerShape::Diamond,
    },
];

/// Maps domain values `[d_min, d_max]` onto pixel coordinates `[px_min, px_max]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisScale {
    /// Scale kind.
    pub kind: ScaleKind,
    /// Minimum in transformed space.
    pub t_min: f64,
    /// Maximum in transformed space.
    pub t_max: f64,
    /// Pixel coordinate corresponding to `t_min`.
    pub px_min: f64,
    /// Pixel coordinate corresponding to `t_max`.
    pub px_max: f64,
}

impl AxisScale {
    /// Constructs an `AxisScale` from raw domain bounds `[d_min, d_max]`.
    pub fn new(
        kind: ScaleKind,
        d_min: f64,
        d_max: f64,
        px_min: f64,
        px_max: f64,
        series: &str,
        axis: &'static str,
    ) -> Result<Self, PlotError> {
        let mut t_min = kind.transform(d_min, series, axis)?;
        let mut t_max = kind.transform(d_max, series, axis)?;
        if (t_max - t_min).abs() < 1e-12 {
            let pad = if t_min.abs() > 1e-6 {
                t_min.abs() * 0.1
            } else {
                1.0
            };
            t_min -= pad;
            t_max += pad;
        }
        Ok(Self {
            kind,
            t_min,
            t_max,
            px_min,
            px_max,
        })
    }

    /// Maps a raw domain value `v` to its pixel coordinate.
    pub fn map(&self, v: f64, series: &str, axis: &'static str) -> Result<f64, PlotError> {
        let t = self.kind.transform(v, series, axis)?;
        let frac = (t - self.t_min) / (self.t_max - self.t_min);
        Ok(self.px_min + frac * (self.px_max - self.px_min))
    }

    /// Maps a clamped value (e.g. lower error bar bound on a log scale).
    pub fn map_clamped(&self, v: f64) -> f64 {
        let t = match self.kind {
            ScaleKind::Linear => v,
            ScaleKind::Log => {
                if v <= 0.0 {
                    self.t_min
                } else {
                    v.log10().max(self.t_min)
                }
            }
            ScaleKind::Log2 => {
                if v <= 0.0 {
                    self.t_min
                } else {
                    v.log2().max(self.t_min)
                }
            }
        };
        let frac = ((t - self.t_min) / (self.t_max - self.t_min)).clamp(0.0, 1.0);
        self.px_min + frac * (self.px_max - self.px_min)
    }
}

/// Generates clean tick values across `[min, max]`, handling single-value, tiny-range, and negative inputs.
pub fn nice_ticks(min: f64, max: f64, target_count: usize) -> Vec<f64> {
    if !min.is_finite() || !max.is_finite() {
        return vec![0.0];
    }
    if (max - min).abs() < 1e-15 {
        return vec![min];
    }
    let count = target_count.max(2) as f64;
    let range = nice_num(max - min, false);
    let step = nice_num(range / (count - 1.0), true);
    if step <= 0.0 || !step.is_finite() {
        return vec![min, max];
    }

    let start = (min / step).floor() * step;
    let end = (max / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut v = start;
    let mut guard = 0usize;
    while v <= end + 0.5 * step && guard < 64 {
        let clean = if v.abs() < step * 1e-9 { 0.0 } else { v };
        ticks.push(clean);
        v += step;
        guard += 1;
    }
    if ticks.is_empty() {
        ticks.push(min);
        ticks.push(max);
    }
    ticks
}

fn nice_num(range: f64, round: bool) -> f64 {
    let exp = range.abs().log10().floor();
    let frac = range.abs() / 10f64.powf(exp);
    let nice_frac = if round {
        if frac < 1.5 {
            1.0
        } else if frac < 3.0 {
            2.0
        } else if frac < 7.0 {
            5.0
        } else {
            10.0
        }
    } else if frac <= 1.0 {
        1.0
    } else if frac <= 2.0 {
        2.0
    } else if frac <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice_frac * 10f64.powf(exp)
}

/// Opens a static, script-free SVG root element with IBM Plex Sans styling.
pub fn begin_svg(width: f64, height: f64) -> String {
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}">"#
    );
    svg.push_str(
        r##"<style>text { font-family: 'IBM Plex Sans', sans-serif; fill: #161616; }</style>"##,
    );
    svg.push('\n');
    let _ = writeln!(
        svg,
        r##"<rect x="0" y="0" width="{width}" height="{height}" fill="#ffffff"/>"##
    );
    svg
}

/// Renders an explicit "no data" panel when a plot receives empty inputs (Task 19).
pub fn render_no_data_svg(title: Option<&str>, width: f64, height: f64) -> String {
    let mut svg = begin_svg(width, height);
    if let Some(t) = title {
        let _ = writeln!(
            svg,
            r##"<text x="20" y="26" font-size="14" font-weight="600">{}</text>"##,
            xml_escape(t)
        );
    }
    let _ = writeln!(
        svg,
        r##"<rect x="20" y="40" width="{}" height="{}" fill="none" stroke="#c6c6c6" stroke-width="1"/>"##,
        width - 40.0,
        height - 60.0
    );
    let _ = writeln!(
        svg,
        r##"<text x="{}" y="{}" text-anchor="middle" font-size="12" fill="#525252">no data</text>"##,
        width * 0.5,
        height * 0.5 + 10.0
    );
    svg.push_str("</svg>\n");
    svg
}

/// Draws a marker at `(cx, cy)` with size `r`.
pub fn draw_marker(svg: &mut String, shape: MarkerShape, cx: f64, cy: f64, r: f64, color: &str) {
    match shape {
        MarkerShape::Circle => {
            let _ = writeln!(
                svg,
                r#"<circle cx="{cx:.2}" cy="{cy:.2}" r="{r:.2}" fill="{color}"/>"#
            );
        }
        MarkerShape::Square => {
            let d = r * 1.8;
            let _ = writeln!(
                svg,
                r#"<rect x="{:.2}" y="{:.2}" width="{d:.2}" height="{d:.2}" fill="{color}"/>"#,
                cx - r * 0.9,
                cy - r * 0.9
            );
        }
        MarkerShape::Triangle => {
            let _ = writeln!(
                svg,
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{color}"/>"#,
                cx,
                cy - r * 1.1,
                cx - r,
                cy + r * 0.8,
                cx + r,
                cy + r * 0.8
            );
        }
        MarkerShape::Diamond => {
            let _ = writeln!(
                svg,
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{color}"/>"#,
                cx,
                cy - r * 1.1,
                cx + r * 1.1,
                cy,
                cx,
                cy + r * 1.1,
                cx - r * 1.1,
                cy
            );
        }
    }
}

/// Formats a numeric tick label compactly.
pub fn format_tick(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e9 {
        format!("{}", v.round() as i64)
    } else if v.abs() >= 1000.0 || (v.abs() > 0.0 && v.abs() < 0.01) {
        format!("{v:.2e}")
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
