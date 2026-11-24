//! String interpolation (`${{ expr }}`) and input-file template rendering (`{{ var }}`) (spec §5.2, §8.4 R-STP-8).

use crate::expr::ast::Span;
use crate::expr::error::ExprError;
use crate::expr::eval::eval;
use crate::expr::parser::parse;
use crate::expr::value::Env;
use calo_schema::ScalarValue;
use std::collections::BTreeMap;

/// Interpolates all `${{ expr }}` blocks inside `template` by parsing and evaluating each expression
/// against `env` (spec §5.2, §8.4 R-STP-1).
pub fn interpolate(template: &str, env: &dyn Env) -> Result<String, ExprError> {
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0;
    let bytes = template.as_bytes();
    let len = bytes.len();

    while cursor < len {
        if let Some(rel_start) = template[cursor..].find("${{") {
            let open_pos = cursor + rel_start;
            out.push_str(&template[cursor..open_pos]);

            let expr_start = open_pos + 3;
            let Some(rel_end) = template[expr_start..].find("}}") else {
                return Err(ExprError::TemplateError {
                    span: Span::new(open_pos, len),
                    message: "unclosed `${{` in string interpolation".to_string(),
                });
            };
            let expr_end = expr_start + rel_end;
            let raw_expr = &template[expr_start..expr_end];

            let ast = parse(raw_expr).map_err(|e| e.with_offset(expr_start))?;
            let val = eval(&ast, env).map_err(|e| e.with_offset(expr_start))?;
            out.push_str(&val.to_string());

            cursor = expr_end + 2;
        } else {
            out.push_str(&template[cursor..]);
            break;
        }
    }

    Ok(out)
}

/// Renders an input-file template using strict `{{ var }}` variable substitution only (spec §5.2, §8.4 R-STP-8).
///
/// Logic blocks (`{% ... %}`) or complex expressions inside `{{ ... }}` are rejected, and any missing
/// variable returns `ExprError::UnresolvedReference`.
pub fn render_template(
    template: &str,
    vars: &BTreeMap<String, ScalarValue>,
) -> Result<String, ExprError> {
    if let Some(logic_pos) = template.find("{%") {
        return Err(ExprError::TemplateError {
            span: Span::new(logic_pos, (logic_pos + 2).min(template.len())),
            message:
                "logic blocks (`{% ... %}`) are forbidden in input file templates (spec R-STP-8)"
                    .to_string(),
        });
    }

    let mut out = String::with_capacity(template.len());
    let mut cursor = 0;
    let len = template.len();

    while cursor < len {
        if let Some(rel_start) = template[cursor..].find("{{") {
            let open_pos = cursor + rel_start;
            out.push_str(&template[cursor..open_pos]);

            let var_start = open_pos + 2;
            let Some(rel_end) = template[var_start..].find("}}") else {
                return Err(ExprError::TemplateError {
                    span: Span::new(open_pos, len),
                    message: "unclosed `{{` in template".to_string(),
                });
            };
            let var_end = var_start + rel_end;
            let raw_inner = &template[var_start..var_end];
            let trimmed = raw_inner.trim();

            if trimmed.is_empty() {
                return Err(ExprError::TemplateError {
                    span: Span::new(open_pos, var_end + 2),
                    message: "empty `{{ }}` placeholder in template".to_string(),
                });
            }

            if !is_valid_template_var_name(trimmed) {
                return Err(ExprError::TemplateError {
                    span: Span::new(var_start, var_end),
                    message: format!(
                        "invalid template variable `{trimmed}`: only simple variable names are permitted in `{{{{ var }}}}` (spec R-STP-8)"
                    ),
                });
            }

            let val = vars.get(trimmed).ok_or_else(|| {
                let trim_offset = raw_inner.find(trimmed).unwrap_or(0);
                ExprError::UnresolvedReference {
                    name: trimmed.to_string(),
                    span: Span::new(
                        var_start + trim_offset,
                        var_start + trim_offset + trimmed.len(),
                    ),
                }
            })?;

            out.push_str(&val.to_string());
            cursor = var_end + 2;
        } else {
            out.push_str(&template[cursor..]);
            break;
        }
    }

    Ok(out)
}

fn is_valid_template_var_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}
