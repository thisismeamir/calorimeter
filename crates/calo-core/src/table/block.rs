//! Decoded columnar data block and typed column accessors (`calo.table/1`, spec §11).

use crate::table::error::TableError;
use calo_schema::table::TableHeader;
use std::collections::BTreeMap;

/// Typed column data vector within a decoded `Block` (spec §11).
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    /// `int32` column values.
    Int32(Vec<i32>),
    /// `int64` column values.
    Int64(Vec<i64>),
    /// `float32` column values.
    Float32(Vec<f32>),
    /// `float64` column values.
    Float64(Vec<f64>),
    /// `uint8` column values.
    Uint8(Vec<u8>),
    /// `string_id` dictionary indices (`u32`).
    StringId(Vec<u32>),
    // `uint32` column values
    Uint32(Vec<u32>),
    // `uint64` column values
    Uint64(Vec<u64>),
    // `bool` column values
    Bool(Vec<bool>),
}

impl ColumnData {
    /// Returns the schema type name of this column data variant.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Int32(_) => "int32",
            Self::Int64(_) => "int64",
            Self::Float32(_) => "float32",
            Self::Float64(_) => "float64",
            Self::Uint8(_) => "uint8",
            Self::StringId(_) => "string_id",
            Self::Uint32(_) => "uint32",
            Self::Uint64(_) => "uint64",
            Self::Bool(_) => "bool",
        }
    }
}

/// A single decoded, CRC32-verified block from a `calo.table/1` sidecar file (spec §11).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    rows: u32,
    columns: BTreeMap<String, ColumnData>,
}

impl Block {
    /// Constructs a new `Block` with `rows` rows and named `columns`.
    pub fn new(rows: u32, columns: BTreeMap<String, ColumnData>) -> Self {
        Self { rows, columns }
    }

    /// Number of rows in this block.
    pub const fn rows(&self) -> u32 {
        self.rows
    }

    /// Returns a reference to the raw `ColumnData` for `name`.
    pub fn column(&self, name: &str) -> Result<&ColumnData, TableError> {
        self.columns
            .get(name)
            .ok_or_else(|| TableError::UnknownColumn {
                name: name.to_string(),
            })
    }

    /// Typed accessor for an `int32` column (spec §11).
    pub fn int32(&self, name: &str) -> Result<&[i32], TableError> {
        match self.column(name)? {
            ColumnData::Int32(v) => Ok(v.as_slice()),
            other => Err(TableError::ColumnTypeMismatch {
                name: name.to_string(),
                expected: "int32",
                actual: other.type_name(),
            }),
        }
    }

    /// Typed accessor for an `int64` column (spec §11).
    pub fn int64(&self, name: &str) -> Result<&[i64], TableError> {
        match self.column(name)? {
            ColumnData::Int64(v) => Ok(v.as_slice()),
            other => Err(TableError::ColumnTypeMismatch {
                name: name.to_string(),
                expected: "int64",
                actual: other.type_name(),
            }),
        }
    }

    /// Typed accessor for a `float32` column (spec §11).
    pub fn float32(&self, name: &str) -> Result<&[f32], TableError> {
        match self.column(name)? {
            ColumnData::Float32(v) => Ok(v.as_slice()),
            other => Err(TableError::ColumnTypeMismatch {
                name: name.to_string(),
                expected: "float32",
                actual: other.type_name(),
            }),
        }
    }

    /// Typed accessor for a `float64` column (spec §11).
    pub fn float64(&self, name: &str) -> Result<&[f64], TableError> {
        match self.column(name)? {
            ColumnData::Float64(v) => Ok(v.as_slice()),
            other => Err(TableError::ColumnTypeMismatch {
                name: name.to_string(),
                expected: "float64",
                actual: other.type_name(),
            }),
        }
    }

    /// Typed accessor for a `uint8` column (spec §11).
    pub fn uint8(&self, name: &str) -> Result<&[u8], TableError> {
        match self.column(name)? {
            ColumnData::Uint8(v) => Ok(v.as_slice()),
            other => Err(TableError::ColumnTypeMismatch {
                name: name.to_string(),
                expected: "uint8",
                actual: other.type_name(),
            }),
        }
    }

    /// Typed accessor for raw `string_id` dictionary indices (`u32`) (spec §11).
    pub fn string_ids(&self, name: &str) -> Result<&[u32], TableError> {
        match self.column(name)? {
            ColumnData::StringId(v) => Ok(v.as_slice()),
            other => Err(TableError::ColumnTypeMismatch {
                name: name.to_string(),
                expected: "string_id",
                actual: other.type_name(),
            }),
        }
    }

    /// Resolves a `string_id` column against the dictionary in `header.strings` (spec §11).
    pub fn strings<'a>(
        &self,
        name: &str,
        header: &'a TableHeader,
    ) -> Result<Vec<&'a str>, TableError> {
        let ids = self.string_ids(name)?;
        let dict = header.strings.get(name).map_or(&[][..], Vec::as_slice);
        let mut resolved = Vec::with_capacity(ids.len());
        for &id in ids {
            let idx = id as usize;
            let s = dict.get(idx).ok_or_else(|| TableError::InvalidStringId {
                column: name.to_string(),
                id,
                dict_len: dict.len(),
            })?;
            resolved.push(s.as_str());
        }
        Ok(resolved)
    }
}
