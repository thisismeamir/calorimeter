//! Minimal typed columnar `Frame` for in-memory service execution (spec §12.2 R-SVC-1).

use crate::expr::{Env, Value};
use crate::services::error::ServiceError;
use calo_schema::ScalarValue;
use std::collections::BTreeMap;

/// Typed column storage inside a `Frame` (spec §12.2 R-SVC-1).
#[derive(Debug, Clone, PartialEq)]
pub enum Column {
    /// 64-bit signed integer column.
    Int(Vec<i64>),
    /// 64-bit floating-point column.
    Float(Vec<f64>),
    /// Boolean column.
    Bool(Vec<bool>),
    /// UTF-8 string column.
    Str(Vec<String>),
}

impl Column {
    /// Number of elements in the column.
    pub fn len(&self) -> usize {
        match self {
            Self::Int(v) => v.len(),
            Self::Float(v) => v.len(),
            Self::Bool(v) => v.len(),
            Self::Str(v) => v.len(),
        }
    }

    /// Returns `true` if the column has zero rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Human-readable column type name.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Bool(_) => "bool",
            Self::Str(_) => "str",
        }
    }

    /// Returns the value at `row` as an expression `Value`.
    pub fn get_value(&self, row: usize) -> Option<Value> {
        match self {
            Self::Int(v) => v.get(row).copied().map(Value::Int),
            Self::Float(v) => v.get(row).copied().map(Value::Float),
            Self::Bool(v) => v.get(row).copied().map(Value::Bool),
            Self::Str(v) => v.get(row).cloned().map(Value::Str),
        }
    }

    /// Extracts numeric values as `Vec<f64>` (converting `Int` to `f64` if needed).
    pub fn to_f64_vec(&self, col_name: &str) -> Result<Vec<f64>, ServiceError> {
        match self {
            Self::Float(v) => Ok(v.clone()),
            Self::Int(v) => Ok(v.iter().map(|&x| x as f64).collect()),
            other => Err(ServiceError::ColumnType {
                column: col_name.to_string(),
                expected: "float or int",
                actual: other.type_name(),
            }),
        }
    }
}

/// Minimal typed column frame operated on by built-in and expression services (spec §12.2 R-SVC-1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    rows: usize,
    columns: BTreeMap<String, Column>,
    meta: BTreeMap<String, ScalarValue>,
}

impl Frame {
    /// Creates an empty `Frame`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of rows in the frame.
    pub const fn num_rows(&self) -> usize {
        self.rows
    }

    /// Inserts or replaces a named column, enforcing consistent row counts across columns.
    pub fn insert_column(
        &mut self,
        name: impl Into<String>,
        col: Column,
    ) -> Result<(), ServiceError> {
        let name = name.into();
        if self.columns.is_empty() {
            self.rows = col.len();
        } else if col.len() != self.rows {
            return Err(ServiceError::RowCountMismatch {
                column: name,
                expected: self.rows,
                actual: col.len(),
            });
        }
        self.columns.insert(name, col);
        Ok(())
    }

    /// Returns a reference to the column `name` if present.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.get(name)
    }

    /// Returns a required column or `ServiceError::MissingColumn`.
    pub fn require_column(&self, service: &str, name: &str) -> Result<&Column, ServiceError> {
        self.columns
            .get(name)
            .ok_or_else(|| ServiceError::MissingColumn {
                service: service.to_string(),
                column: name.to_string(),
            })
    }

    /// Returns all columns in deterministic key order.
    pub const fn columns(&self) -> &BTreeMap<String, Column> {
        &self.columns
    }

    /// Sets a metadata scalar on the frame (e.g. `bootstrap_seed` or `unit`).
    pub fn set_meta(&mut self, key: impl Into<String>, val: ScalarValue) {
        self.meta.insert(key.into(), val);
    }

    /// Gets a metadata scalar from the frame.
    pub fn meta(&self, key: &str) -> Option<&ScalarValue> {
        self.meta.get(key)
    }

    /// Returns an `Env` view of a single row `row_idx` for expression evaluation (spec §12.2 R-SVC-5).
    pub fn row_env(&self, row_idx: usize) -> RowEnv<'_> {
        RowEnv {
            frame: self,
            row_idx,
        }
    }
}

/// Per-row `Env` adapter over a `Frame` (spec §12.2 R-SVC-5).
pub struct RowEnv<'a> {
    frame: &'a Frame,
    row_idx: usize,
}

impl Env for RowEnv<'_> {
    fn get(&self, path: &str) -> Option<Value> {
        if let Some(col) = self.frame.column(path) {
            return col.get_value(self.row_idx);
        }
        // Also allow `params.<k>` to match bare `<k>` or vice versa if stored under either key.
        if let Some(bare) = path.strip_prefix("params.") {
            if let Some(col) = self.frame.column(bare) {
                return col.get_value(self.row_idx);
            }
        } else {
            let prefixed = format!("params.{path}");
            if let Some(col) = self.frame.column(&prefixed) {
                return col.get_value(self.row_idx);
            }
        }
        None
    }
}
