//! Layered configuration resolution and origin tracking for `manifest.yaml` (spec §5.1, §15).

use crate::yaml::error::LoadError;
use calo_schema::yaml::{Mapping, Value};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Configuration precedence layers from lowest to highest (spec §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// Built-in default values (lowest precedence).
    Default,
    /// Machine inventory (`inventory.yaml`).
    Inventory,
    /// Suite configuration (`suite.yaml` + includes).
    Suite,
    /// Per-environment overlay (`overlays/*.yaml`).
    Overlay,
    /// Benchmark-level overrides.
    Benchmark,
    /// Command-line `--set key=value` overrides (highest precedence).
    CliSet,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Default => "default",
            Self::Inventory => "inventory",
            Self::Suite => "suite",
            Self::Overlay => "overlay",
            Self::Benchmark => "benchmark",
            Self::CliSet => "cli_set",
        };
        f.write_str(s)
    }
}

/// Provenance record for a single resolved configuration key in `manifest.yaml` (spec §5.1, §15).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    /// Winning layer that provided the value.
    pub layer: Layer,
    /// Resolved value at this key path.
    pub value: Value,
}

/// Resolved configuration value paired with per-key origin provenance (spec §5.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Layered<T> {
    /// Typed deserialized configuration value.
    pub value: T,
    /// Merged raw YAML value after all layers and `--set` overrides.
    pub raw: Value,
    /// Dotted-path map recording the winning `Origin` for each leaf/replaced key.
    pub origins: BTreeMap<String, Origin>,
}

/// Builder for accumulating configuration layers in precedence order (spec §5.1).
#[derive(Debug, Clone, Default)]
pub struct LayeredBuilder {
    entries: Vec<(Layer, Value)>,
    cli_sets: Vec<String>,
}

impl LayeredBuilder {
    /// Creates an empty `LayeredBuilder`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a YAML `Value` at the specified `Layer` (spec §5.1).
    pub fn add_layer(&mut self, layer: Layer, value: Value) -> &mut Self {
        self.entries.push((layer, value));
        self
    }

    /// Adds a CLI `--set key=value` override (e.g. `"benchmarks.0.repeat=3"`, spec §5.1).
    pub fn add_cli_set(&mut self, spec: impl Into<String>) -> &mut Self {
        self.cli_sets.push(spec.into());
        self
    }

    /// Adds multiple CLI `--set key=value` overrides (spec §5.1).
    pub fn add_cli_sets<I, S>(&mut self, specs: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for s in specs {
            self.cli_sets.push(s.into());
        }
        self
    }

    /// Resolves all added layers in ascending precedence order (`Default` -> ... -> `CliSet`)
    /// and deserializes the merged YAML into `T` (spec §5.1).
    pub fn resolve<T: DeserializeOwned>(&self) -> Result<Layered<T>, LoadError> {
        // Sort a working list by `Layer` enum order while preserving insertion order within the same layer.
        let mut ordered: Vec<(Layer, Value)> = self.entries.clone();
        ordered.sort_by_key(|(layer, _)| *layer);

        let mut merged = Value::Mapping(Mapping::new());
        let mut origins = BTreeMap::new();

        for (layer, val) in ordered {
            merge_with_origin(&mut merged, val, layer, "", &mut origins);
        }

        for raw_set in &self.cli_sets {
            apply_cli_set(&mut merged, raw_set, &mut origins)?;
        }

        let value: T = calo_schema::yaml::from_value(merged.clone())
            .map_err(|e| LoadError::Deserialization(e.to_string()))?;

        Ok(Layered {
            value,
            raw: merged,
            origins,
        })
    }
}

/// Converts the origin map of a `Layered<T>` for `manifest.yaml` (spec §5.1, §15).
pub fn to_manifest_resolved<T>(layered: &Layered<T>) -> BTreeMap<String, Origin> {
    layered.origins.clone()
}

fn merge_with_origin(
    base: &mut Value,
    overlay: Value,
    layer: Layer,
    prefix: &str,
    origins: &mut BTreeMap<String, Origin>,
) {
    match (base, overlay) {
        (Value::Mapping(base_map), Value::Mapping(overlay_map)) => {
            for (k, v) in overlay_map {
                let key_str = scalar_key_to_string(&k);
                let path = if prefix.is_empty() {
                    key_str
                } else {
                    format!("{prefix}.{key_str}")
                };

                if let Some(existing) = base_map.get_mut(&k) {
                    merge_with_origin(existing, v, layer, &path, origins);
                } else {
                    record_subtree_origins(&v, layer, &path, origins);
                    base_map.insert(k, v);
                }
            }
        }
        (base_slot, overlay_val) => {
            // Lists and scalars replace any prior subtree Origins under `prefix` (spec §5.1).
            if !prefix.is_empty() {
                let sub_prefix = format!("{prefix}.");
                origins.retain(|k, _| k != prefix && !k.starts_with(&sub_prefix));
                record_subtree_origins(&overlay_val, layer, prefix, origins);
            }
            *base_slot = overlay_val;
        }
    }
}

fn record_subtree_origins(
    val: &Value,
    layer: Layer,
    prefix: &str,
    origins: &mut BTreeMap<String, Origin>,
) {
    match val {
        Value::Mapping(map) => {
            if map.is_empty() && !prefix.is_empty() {
                origins.insert(
                    prefix.to_string(),
                    Origin {
                        layer,
                        value: val.clone(),
                    },
                );
                return;
            }
            for (k, v) in map {
                let key_str = scalar_key_to_string(k);
                let path = if prefix.is_empty() {
                    key_str
                } else {
                    format!("{prefix}.{key_str}")
                };
                record_subtree_origins(v, layer, &path, origins);
            }
        }
        Value::Sequence(seq) => {
            if !prefix.is_empty() {
                origins.insert(
                    prefix.to_string(),
                    Origin {
                        layer,
                        value: val.clone(),
                    },
                );
            }
            for (idx, item) in seq.iter().enumerate() {
                let path = if prefix.is_empty() {
                    idx.to_string()
                } else {
                    format!("{prefix}.{idx}")
                };
                record_subtree_origins(item, layer, &path, origins);
            }
        }
        _ => {
            if !prefix.is_empty() {
                origins.insert(
                    prefix.to_string(),
                    Origin {
                        layer,
                        value: val.clone(),
                    },
                );
            }
        }
    }
}

fn apply_cli_set(
    root: &mut Value,
    raw_spec: &str,
    origins: &mut BTreeMap<String, Origin>,
) -> Result<(), LoadError> {
    let Some((path_str, val_str)) = raw_spec.split_once('=') else {
        return Err(LoadError::InvalidCliSet {
            raw: raw_spec.to_string(),
            reason: "missing `=` separator (expected `key.path=value`)".to_string(),
        });
    };

    let path_str = path_str.trim();
    if path_str.is_empty() {
        return Err(LoadError::InvalidCliSet {
            raw: raw_spec.to_string(),
            reason: "key path cannot be empty".to_string(),
        });
    }

    let segments: Vec<&str> = path_str.split('.').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(LoadError::InvalidCliSet {
            raw: raw_spec.to_string(),
            reason: "key path contains empty segment".to_string(),
        });
    }

    // Parse the RHS as a YAML value (so `3` -> Int, `true` -> Bool, `foo` -> String).
    let parsed_val: Value = if val_str.is_empty() {
        Value::String(String::new())
    } else {
        calo_schema::yaml::from_str(val_str).unwrap_or_else(|_| Value::String(val_str.to_string()))
    };

    set_dotted_value(root, &segments, parsed_val.clone(), raw_spec)?;

    let sub_prefix = format!("{path_str}.");
    origins.retain(|k, _| k != path_str && !k.starts_with(&sub_prefix));
    record_subtree_origins(&parsed_val, Layer::CliSet, path_str, origins);
    Ok(())
}

fn set_dotted_value(
    current: &mut Value,
    segments: &[&str],
    new_val: Value,
    raw_spec: &str,
) -> Result<(), LoadError> {
    let Some((&first, rest)) = segments.split_first() else {
        *current = new_val;
        return Ok(());
    };

    if rest.is_empty() {
        match current {
            Value::Mapping(map) => {
                map.insert(Value::String(first.to_string()), new_val);
                return Ok(());
            }
            Value::Sequence(seq) => {
                let idx: usize = first.parse().map_err(|_| LoadError::InvalidCliSet {
                    raw: raw_spec.to_string(),
                    reason: format!("expected sequence index, found `{first}`"),
                })?;
                if idx >= seq.len() {
                    return Err(LoadError::InvalidCliSet {
                        raw: raw_spec.to_string(),
                        reason: format!(
                            "sequence index {idx} out of bounds (len {})",
                            seq.len()
                        ),
                    });
                }
                seq[idx] = new_val;
                return Ok(());
            }
            _ => {
                return Err(LoadError::InvalidCliSet {
                    raw: raw_spec.to_string(),
                    reason: format!("cannot index into scalar with `{first}`"),
                });
            }
        }
    }

    // Intermediate step
    match current {
        Value::Mapping(map) => {
            let key = Value::String(first.to_string());
            if !map.contains_key(&key) {
                map.insert(key.clone(), Value::Mapping(Mapping::new()));
            }
            let child = map.get_mut(&key).ok_or_else(|| LoadError::InvalidCliSet {
                raw: raw_spec.to_string(),
                reason: "failed to traverse mapping".to_string(),
            })?;
            set_dotted_value(child, rest, new_val, raw_spec)
        }
        Value::Sequence(seq) => {
            let idx: usize = first.parse().map_err(|_| LoadError::InvalidCliSet {
                raw: raw_spec.to_string(),
                reason: format!("expected sequence index, found `{first}`"),
            })?;
            let child = seq.get_mut(idx).ok_or_else(|| LoadError::InvalidCliSet {
                raw: raw_spec.to_string(),
                reason: format!("sequence index {idx} out of bounds"),
            })?;
            set_dotted_value(child, rest, new_val, raw_spec)
        }
        _ => Err(LoadError::InvalidCliSet {
            raw: raw_spec.to_string(),
            reason: format!("cannot traverse through scalar at `{first}`"),
        }),
    }
}

fn scalar_key_to_string(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => format!("{k:?}"),
    }
}
