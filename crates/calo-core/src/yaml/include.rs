//! YAML loading with anchor/alias/merge-key expansion and recursive `include:` merging (spec §0.1, §5.1).

use crate::yaml::error::LoadError;
use calo_schema::yaml::{Mapping, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Reads a YAML file from `path`, expands anchors/aliases/merge keys (`<<`),
/// and recursively resolves top-level `include: [paths]` relative to the including file (spec §5.1).
///
/// Maps merge recursively and lists replace. Include cycles return `LoadError::IncludeCycle`.
pub fn load_with_includes(path: impl AsRef<Path>) -> Result<Value, LoadError> {
    let mut stack = Vec::new();
    load_recursive(path.as_ref(), &mut stack)
}

/// Parses a YAML string as if loaded from `virtual_path`, expanding anchors/merge keys
/// and resolving any relative `include:` paths on disk (spec §0.1, §5.1).
pub fn load_str_with_includes(
    src: &str,
    virtual_path: impl AsRef<Path>,
) -> Result<Value, LoadError> {
    let path = virtual_path.as_ref();
    let canonical = normalize_path(path);
    let mut stack = vec![canonical];
    parse_and_resolve(src, path, &mut stack)
}

fn load_recursive(path: &Path, stack: &mut Vec<PathBuf>) -> Result<Value, LoadError> {
    let canonical = normalize_path(path);
    if let Some(pos) = stack.iter().position(|p| p == &canonical) {
        let mut chain_parts: Vec<String> = stack[pos..]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        chain_parts.push(canonical.display().to_string());
        return Err(LoadError::IncludeCycle {
            chain: chain_parts.join(" -> "),
        });
    }

    let content = fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    stack.push(canonical);
    let result = parse_and_resolve(&content, path, stack);
    stack.pop();
    result
}

fn parse_and_resolve(
    content: &str,
    current_path: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<Value, LoadError> {
    let mut doc: Value = calo_schema::yaml::from_str(content)
        .map_err(|e| LoadError::from_yaml_err(current_path, e))?;

    // Expand YAML `<<` merge keys recursively (spec §0.1).
    doc.apply_merge()
        .map_err(|e| LoadError::from_yaml_err(current_path, e))?;

    let Value::Mapping(mut map) = doc else {
        return Ok(doc);
    };

    let include_key = Value::String("include".to_string());
    let Some(include_val) = map.remove(&include_key) else {
        return Ok(Value::Mapping(map));
    };

    let include_paths = extract_include_paths(include_val, current_path)?;
    let base_dir = current_path.parent().unwrap_or_else(|| Path::new("."));

    // Merge included files in declaration order, then merge the current file on top (spec §5.1).
    let mut merged = Value::Mapping(Mapping::new());
    for rel_path in include_paths {
        let target = base_dir.join(rel_path);
        let included_val = load_recursive(&target, stack)?;
        merge_values(&mut merged, included_val);
    }

    merge_values(&mut merged, Value::Mapping(map));
    Ok(merged)
}

fn extract_include_paths(val: Value, current_path: &Path) -> Result<Vec<PathBuf>, LoadError> {
    match val {
        Value::String(s) => Ok(vec![PathBuf::from(s)]),
        Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                match item {
                    Value::String(s) => out.push(PathBuf::from(s)),
                    _ => {
                        return Err(LoadError::InvalidIncludeFormat {
                            path: current_path.to_path_buf(),
                        })
                    }
                }
            }
            Ok(out)
        }
        _ => Err(LoadError::InvalidIncludeFormat {
            path: current_path.to_path_buf(),
        }),
    }
}

/// Merges `overlay` into `base` according to spec §5.1:
/// maps merge recursively and lists (and scalars) replace.
pub fn merge_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Mapping(base_map), Value::Mapping(overlay_map)) => {
            for (k, v) in overlay_map {
                if let Some(existing) = base_map.get_mut(&k) {
                    merge_values(existing, v);
                } else {
                    base_map.insert(k, v);
                }
            }
        }
        (base_slot, overlay_val) => {
            *base_slot = overlay_val;
        }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canon) = fs::canonicalize(path) {
        return canon;
    }
    // Lexical normalization fallback if file doesn't exist yet (so Io error triggers cleanly later).
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
