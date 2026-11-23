//! Schema dumping (`calo schema dump`) and JSON-pointer schema validation for `calo lint` (spec §0.2).

use crate::export::error::ExportError;
use calo_schema::yaml::Value;
use std::fs;
use std::path::Path;

/// Writes all generated `*.schema.yaml` files into `out_dir` (spec §0.2).
pub fn dump_schemas_to_dir(out_dir: &Path) -> Result<Vec<std::path::PathBuf>, ExportError> {
    fs::create_dir_all(out_dir).map_err(|source| ExportError::Io {
        path: out_dir.to_path_buf(),
        source,
    })?;

    let mut written = Vec::new();
    for (filename, content) in calo_schema::schema_gen::generate_all_schemas() {
        let path = out_dir.join(filename);
        fs::write(&path, content.as_bytes()).map_err(|source| ExportError::Io {
            path: path.clone(),
            source,
        })?;
        written.push(path);
    }
    Ok(written)
}

/// Validates a YAML `Value` against a JSON Schema YAML `Value`, reporting exact JSON-pointer paths (spec §0.2).
pub fn validate_against_schema(
    doc: &Value,
    schema: &Value,
) -> Result<(), Vec<ExportError>> {
    let mut errs = Vec::new();
    validate_node(doc, schema, "", &mut errs);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn validate_node(
    doc: &Value,
    schema: &Value,
    ptr: &str,
    errs: &mut Vec<ExportError>,
) {
    let Value::Mapping(s_map) = schema else {
        return;
    };
    let display_ptr = if ptr.is_empty() { "/" } else { ptr };

    // `const:` check
    if let Some(expected_const) = s_map.get(Value::String("const".to_string())) {
        if doc != expected_const {
            errs.push(ExportError::SchemaValidation {
                path: display_ptr.to_string(),
                message: format!("expected constant {expected_const:?}, found {doc:?}"),
            });
            return;
        }
    }

    // `type:` check
    if let Some(ty_val) = s_map.get(Value::String("type".to_string())) {
        let ok = match ty_val {
            Value::String(t) => matches_type(doc, t),
            Value::Sequence(ts) => ts.iter().any(|t| match t {
                Value::String(s) => matches_type(doc, s),
                _ => false,
            }),
            _ => true,
        };
        if !ok {
            errs.push(ExportError::SchemaValidation {
                path: display_ptr.to_string(),
                message: format!("expected type {ty_val:?}, found {:?}", doc_type_name(doc)),
            });
            return;
        }
    }

    // `required:` and `properties:` for mappings
    if let Value::Mapping(d_map) = doc {
        if let Some(Value::Sequence(req_list)) = s_map.get(Value::String("required".to_string())) {
            for r in req_list {
                if let Value::String(rk) = r {
                    if !d_map.contains_key(Value::String(rk.clone())) {
                        errs.push(ExportError::SchemaValidation {
                            path: format!("{ptr}/{rk}"),
                            message: format!("missing required property `{rk}`"),
                        });
                    }
                }
            }
        }
        if let Some(Value::Mapping(props)) = s_map.get(Value::String("properties".to_string())) {
            for (k, v) in d_map {
                if let Some(sub_schema) = props.get(k) {
                    let k_str = match k {
                        Value::String(s) => s.as_str(),
                        _ => "?",
                    };
                    validate_node(v, sub_schema, &format!("{ptr}/{k_str}"), errs);
                }
            }
        }
    }

    // `items:` for sequences
    if let Value::Sequence(d_seq) = doc {
        if let Some(item_schema) = s_map.get(Value::String("items".to_string())) {
            for (idx, item) in d_seq.iter().enumerate() {
                validate_node(item, item_schema, &format!("{ptr}/{idx}"), errs);
            }
        }
    }
}

fn matches_type(doc: &Value, expected: &str) -> bool {
    match expected {
        "object" => matches!(doc, Value::Mapping(_)),
        "array" => matches!(doc, Value::Sequence(_)),
        "string" => matches!(doc, Value::String(_)),
        "boolean" => matches!(doc, Value::Bool(_)),
        "integer" => matches!(doc, Value::Number(n) if n.is_i64() || n.is_u64()),
        "number" => matches!(doc, Value::Number(_)),
        "null" => matches!(doc, Value::Null),
        _ => true,
    }
}

const fn doc_type_name(doc: &Value) -> &'static str {
    match doc {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "array",
        Value::Mapping(_) => "object",
        Value::Tagged(_) => "tagged",
    }
}
