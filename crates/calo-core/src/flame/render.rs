//! Static monochrome/accent SVG flamegraph and differential flamegraph renderers (spec §13 R-REP-3, R-REP-6).

use crate::flame::error::FlameError;
use crate::flame::stack::{FlameNode, FlameTree};
use regex::Regex;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Rendering configuration for `render_flamegraph` and `render_diff_flamegraph` (spec §13).
#[derive(Debug, Clone, PartialEq)]
pub struct FlameOpts {
    /// SVG canvas width in pixels (default `1200`).
    pub width: u32,
    /// Height of each frame row in pixels (default `18`).
    pub row_height: u32,
    /// Minimum percentage threshold for pruning (default `0.05`).
    pub min_pct: f64,
    /// Render top-down icicle graph when `true`, bottom-up flamegraph when `false`.
    pub inverted: bool,
    /// Optional regex highlighting matching frames in accent `#7017FF`.
    pub highlight: Option<String>,
    /// Optional chart title.
    pub title: Option<String>,
}

impl Default for FlameOpts {
    fn default() -> Self {
        Self {
            width: 1200,
            row_height: 18,
            min_pct: 0.05,
            inverted: false,
            highlight: None,
            title: None,
        }
    }
}

/// Escapes `&`, `<`, `>`, `"`, and `'` for safe XML/SVG embedding.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push('&'),
            '<' => out.push('<'),
            '>' => out.push('>'),
            '"' => out.push('\"'),
            '\u{0027}' => out.push('\''),
            other => out.push(other),
        }
    }
    out
}

/// Renders a static, script-free SVG flamegraph (or icicle graph if `opts.inverted`) (spec §13 R-REP-6).
pub fn render_flamegraph(tree: &FlameTree, opts: &FlameOpts) -> Result<String, FlameError> {
    let highlight_re = match &opts.highlight {
        Some(pat) => Some(
            Regex::new(pat).map_err(|e| FlameError::InvalidHighlightRegex {
                pattern: pat.clone(),
                message: e.to_string(),
            })?,
        ),
        None => None,
    };

    let width = opts.width.max(200) as f64;
    let row_h = opts.row_height.max(10) as f64;
    let pad_x = 12.0;
    let header_h = if opts.title.is_some() { 34.0 } else { 14.0 };
    let footer_h = 16.0;
    let usable_w = (width - 2.0 * pad_x).max(10.0);

    let max_depth = tree.root.max_depth().max(1);
    let total_h = header_h + (max_depth as f64) * row_h + footer_h;

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {total_h}" width="{width}" height="{total_h}">"#,
    );
    svg.push_str(
        r#"<style>text { font-family: 'IBM Plex Sans', sans-serif; font-size: 11px; }</style>"#,
    );
    svg.push('\n');
    let _ = writeln!(
        svg,
        r##"<rect x="0" y="0" width="{width}" height="{total_h}" fill="#ffffff"/>"##
    );

    if let Some(title) = &opts.title {
        let _ = writeln!(
            svg,
            r##"<text x="{pad_x}" y="22" font-size="14" font-weight="600" fill="#161616">{}</text>"##,
            xml_escape(title)
        );
    }

    if tree.root.total_weight == 0 {
        let _ = writeln!(
            svg,
            r##"<text x="{pad_x}" y="{}" fill="#525252">no samples</text>"##,
            header_h + 14.0
        );
        svg.push_str("</svg>\n");
        return Ok(svg);
    }

    let root_total = tree.root.total_weight as f64;
    let greys = ["#e0e0e0", "#c6c6c6", "#8d8d8d", "#525252"];

    let mut ctx = RenderCtx {
        svg: &mut svg,
        root_total,
        row_h,
        header_h,
        max_depth,
        inverted: opts.inverted,
        greys: &greys,
        highlight_re: highlight_re.as_ref(),
        before_totals: None,
        before_root_total: None,
    };

    draw_node(&tree.root, 0, pad_x, usable_w, &mut ctx);

    svg.push_str("</svg>\n");
    Ok(svg)
}

/// Renders a differential flamegraph comparing `before` against `after` (spec §13 R-REP-3, R-REP-6).
///
/// Layout and widths come from `after`. Frames are colored on a grey-to-accent scale (`#7017FF` for
/// growth, `#e0e0e0` for shrinkage, `#8d8d8d` for unchanged within ±1%). Frames present only in
/// `before` are listed in a flat legend line.
pub fn render_diff_flamegraph(
    before: &FlameTree,
    after: &FlameTree,
    opts: &FlameOpts,
) -> Result<String, FlameError> {
    let before_totals = before.totals_by_name();
    let after_totals = after.totals_by_name();
    let removed: Vec<String> = before_totals
        .keys()
        .filter(|k| !after_totals.contains_key(*k))
        .cloned()
        .collect();

    let width = opts.width.max(200) as f64;
    let row_h = opts.row_height.max(10) as f64;
    let pad_x = 12.0;
    let header_h = if opts.title.is_some() { 34.0 } else { 14.0 };
    let legend_h = if removed.is_empty() { 28.0 } else { 44.0 };
    let usable_w = (width - 2.0 * pad_x).max(10.0);

    let max_depth = after.root.max_depth().max(1);
    let total_h = header_h + (max_depth as f64) * row_h + legend_h;

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {total_h}" width="{width}" height="{total_h}">"#,
    );
    svg.push_str(
        r#"<style>text { font-family: 'IBM Plex Sans', sans-serif; font-size: 11px; }</style>"#,
    );
    svg.push('\n');
    let _ = writeln!(
        svg,
        r##"<rect x="0" y="0" width="{width}" height="{total_h}" fill="#ffffff"/>"##
    );

    if let Some(title) = &opts.title {
        let _ = writeln!(
            svg,
            r##"<text x="{pad_x}" y="22" font-size="14" font-weight="600" fill="#161616">{}</text>"##,
            xml_escape(title)
        );
    }

    if after.root.total_weight == 0 {
        let _ = writeln!(
            svg,
            r##"<text x="{pad_x}" y="{}" fill="#525252">no samples</text>"##,
            header_h + 14.0
        );
        svg.push_str("</svg>\n");
        return Ok(svg);
    }

    let root_total = after.root.total_weight as f64;
    let greys = ["#e0e0e0", "#c6c6c6", "#8d8d8d", "#525252"];

    let mut ctx = RenderCtx {
        svg: &mut svg,
        root_total,
        row_h,
        header_h,
        max_depth,
        inverted: opts.inverted,
        greys: &greys,
        highlight_re: None,
        before_totals: Some(&before_totals),
        before_root_total: Some(before.root.total_weight.max(1) as f64),
    };

    draw_node(&after.root, 0, pad_x, usable_w, &mut ctx);

    // Flat rectangular legend & removed-only-in-before summary
    let ly = header_h + (max_depth as f64) * row_h + 8.0;
    let _ = writeln!(
        svg,
        r##"<rect x="{pad_x}" y="{ly}" width="10" height="10" fill="#7017FF" stroke="#161616" stroke-width="1"/><text x="{}" y="{}" fill="#161616">grew (>+1%)</text>"##,
        pad_x + 14.0,
        ly + 9.0
    );
    let _ = writeln!(
        svg,
        r##"<rect x="{}" y="{ly}" width="10" height="10" fill="#8d8d8d" stroke="#161616" stroke-width="1"/><text x="{}" y="{}" fill="#161616">unchanged (±1%)</text>"##,
        pad_x + 100.0,
        pad_x + 114.0,
        ly + 9.0
    );
    let _ = writeln!(
        svg,
        r##"<rect x="{}" y="{ly}" width="10" height="10" fill="#e0e0e0" stroke="#161616" stroke-width="1"/><text x="{}" y="{}" fill="#161616">shrank (<-1%)</text>"##,
        pad_x + 225.0,
        pad_x + 239.0,
        ly + 9.0
    );

    if !removed.is_empty() {
        let list_str = removed.join(", ");
        let _ = writeln!(
            svg,
            r##"<text x="{pad_x}" y="{}" fill="#525252">Removed (only in before): {}</text>"##,
            ly + 25.0,
            xml_escape(&list_str)
        );
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

struct RenderCtx<'a> {
    svg: &'a mut String,
    root_total: f64,
    row_h: f64,
    header_h: f64,
    max_depth: usize,
    inverted: bool,
    greys: &'a [&'a str; 4],
    highlight_re: Option<&'a Regex>,
    before_totals: Option<&'a BTreeMap<String, u64>>,
    before_root_total: Option<f64>,
}

fn draw_node(node: &FlameNode, depth: usize, x: f64, w: f64, ctx: &mut RenderCtx<'_>) {
    if w < 0.5 || node.total_weight == 0 {
        return;
    }

    let y = if ctx.inverted {
        ctx.header_h + (depth as f64) * ctx.row_h
    } else {
        ctx.header_h + ((ctx.max_depth - 1 - depth) as f64) * ctx.row_h
    };

    let pct = (node.total_weight as f64 / ctx.root_total) * 100.0;

    let (fill, text_fill, tooltip) =
        if let (Some(b_map), Some(b_root)) = (ctx.before_totals, ctx.before_root_total) {
            if depth == 0 {
                (
                    "#e0e0e0",
                    "#161616",
                    format!(
                        "{} (before: {}, after: {})",
                        node.name, b_root as u64, node.total_weight
                    ),
                )
            } else {
                let b_val = b_map.get(&node.name).copied().unwrap_or(0);
                let b_pct = (b_val as f64 / b_root) * 100.0;
                let rel_delta = if b_pct > 0.0 {
                    ((pct - b_pct) / b_pct) * 100.0
                } else {
                    100.0
                };
                let (fill_c, txt_c) = if rel_delta > 1.0 {
                    ("#7017FF", "#ffffff")
                } else if rel_delta < -1.0 {
                    ("#e0e0e0", "#161616")
                } else {
                    ("#8d8d8d", "#161616")
                };
                let tip = format!(
                    "{} (before: {} [{:.2}%], after: {} [{:.2}%], delta: {:+.2}%)",
                    node.name, b_val, b_pct, node.total_weight, pct, rel_delta
                );
                (fill_c, txt_c, tip)
            }
        } else {
            let is_hl = depth > 0 && ctx.highlight_re.is_some_and(|re| re.is_match(&node.name));
            let (fill_c, txt_c) = if is_hl {
                ("#7017FF", "#ffffff")
            } else {
                let g = ctx.greys[depth % ctx.greys.len()];
                let tf = if g == "#525252" { "#ffffff" } else { "#161616" };
                (g, tf)
            };
            let tip = format!("{} ({} samples, {:.2}%)", node.name, node.total_weight, pct);
            (fill_c, txt_c, tip)
        };

    let rect_h = (ctx.row_h - 1.0).max(1.0);
    let _ = writeln!(
        ctx.svg,
        r##"<g><title>{}</title><rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{rect_h:.2}" fill="{fill}" stroke="#161616" stroke-width="0.5"/>"##,
        xml_escape(&tooltip)
    );

    if w >= 24.0 {
        let max_chars = ((w - 6.0) / 6.5).floor() as usize;
        if max_chars >= 2 {
            let label = truncate_label(&node.name, max_chars);
            let ty = y + rect_h * 0.72;
            let tx = x + 3.0;
            let _ = writeln!(
                ctx.svg,
                r#"<text x="{tx:.2}" y="{ty:.2}" fill="{text_fill}">{}</text>"#,
                xml_escape(&label)
            );
        }
    }
    ctx.svg.push_str("</g>\n");

    let mut child_x = x;
    for child in node.children.values() {
        let child_w = w * (child.total_weight as f64 / node.total_weight as f64);
        draw_node(child, depth + 1, child_x, child_w, ctx);
        child_x += child_w;
    }
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    if max_chars <= 3 {
        return s.chars().take(max_chars).collect();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}
