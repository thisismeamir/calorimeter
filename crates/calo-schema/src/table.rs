//! Binary sidecar table header and reference definitions (`calo.table/1`, spec §10, §11).

use crate::error::SchemaError;
use crate::SchemaDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Sidecar reference stored inside `calo.record/2` observation blocks (spec §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// Relative path to the sidecar file or directory.
    pub path: String,
    /// Format identifier (e.g. `"calo.table/1"`).
    pub format: String,
    /// Table schema name (e.g. `"gpu.kernels"`).
    pub schema: String,
    /// Total number of rows in the sidecar table.
    pub rows: u64,
    /// Hex-encoded SHA-256 digest of the sidecar file.
    pub sha256: String,
}

/// Column data type in a `calo.table/1` binary sidecar (spec §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    /// 32-bit signed integer (LE).
    Int32,
    /// 64-bit signed integer (LE).
    Int64,
    /// 32-bit IEEE-754 float (LE).
    Float32,
    /// 64-bit IEEE-754 float (LE).
    Float64,
    /// 8-bit unsigned integer.
    Uint8,
    /// 32-bit string dictionary index (LE).
    StringId,
}

impl ColumnType {
    /// Byte width of a single element of this column type (spec §11).
    pub const fn byte_width(self) -> usize {
        match self {
            Self::Uint8 => 1,
            Self::Int32 | Self::Float32 | Self::StringId => 4,
            Self::Int64 | Self::Float64 => 8,
        }
    }
}

/// Column descriptor in a `calo.table/1` header (spec §11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    /// Column name (with unit suffix per spec §0.3).
    pub name: String,
    /// Column element type.
    #[serde(rename = "type")]
    pub col_type: ColumnType,
}

/// YAML header embedded in a `calo.table/1` binary sidecar file (spec §11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableHeader {
    /// Schema header (`"calo.table/1"`).
    pub schema: String,
    /// Logical table name (e.g., `"gpu.kernels"`, `"series.host"`).
    pub table: String,
    /// Ordered list of columns in each block.
    pub columns: Vec<ColumnDef>,
    /// String dictionary tables keyed by `string_id` column name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub strings: BTreeMap<String, Vec<String>>,
    /// Arbitrary metadata map (e.g., `device`, `clock`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, crate::ScalarValue>,
}

impl SchemaDoc for TableHeader {
    const SCHEMA: &'static str = "calo.table/1";

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
        if self.table.is_empty() {
            return Err(SchemaError::MissingRequiredField {
                field: "table".to_string(),
            });
        }
        Ok(())
    }
}
