//! Suite execution manifest schema (`calo.manifest/1`, spec §15).

use crate::common::{Mode, ScalarValue, Status};
use crate::error::SchemaError;
use crate::inventory::Inventory;
use crate::SchemaDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Suite identity and hash inside `manifest.yaml` (spec §15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSuite {
    /// Suite name.
    pub name: String,
    /// Suite version string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Hex-encoded suite hash.
    pub hash: String,
}

/// Pinned run entry inside `manifest.yaml` (spec §15).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestRun {
    /// Deterministic run identifier.
    pub run_id: String,
    /// Benchmark name.
    pub benchmark: String,
    /// Execution mode.
    pub mode: Mode,
    /// Target machine (or `machine/class`).
    pub machine: String,
    /// Pinned scalar parameters for this run.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ScalarValue>,
    /// Current or final run status.
    pub status: Status,
    /// Linked measure/profile/kernel run IDs (spec §8.5 R-BEN-4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_runs: Vec<String>,
    /// Relative path to the run record YAML file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
}

/// Resolved configuration manifest (`calo.manifest/1`, spec §15).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema header (`"calo.manifest/1"`).
    pub schema: String,
    /// Suite name, version, and hash.
    pub suite: ManifestSuite,
    /// ISO 8601 creation timestamp.
    pub created: String,
    /// `calo` version string.
    pub calo: String,
    /// Full resolved configuration with origin layer metadata per key (spec §5.1, §15).
    pub resolved: BTreeMap<String, crate::yaml::Value>,
    /// Snapshot of the machine inventory used for resolution.
    pub inventory_snapshot: Inventory,
    /// Expanded and pinned runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<ManifestRun>,
    /// Resolved git commit SHAs per named source (spec §8.1 R-SRC-1).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, String>,
}

impl SchemaDoc for Manifest {
    const SCHEMA: &'static str = "calo.manifest/1";

    fn schema_str(&self) -> &str {
        &self.schema
    }

    fn check(&self) -> Result<(), SchemaError> {
        if self.schema != Self::SCHEMA {
            return Err(SchemaError::VersionMismatch {
                expected: Self::SCHEMA.to_string(),
                found: self.schema.clone(),
            });
        }
        if self.suite.name.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "suite.name".to_string(),
            });
        }
        if self.suite.hash.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "suite.hash".to_string(),
            });
        }
        Ok(())
    }
}
