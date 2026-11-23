//! Parquet sidecar export (feature `parquet`) and JSON Schema 2020-12 YAML validation (spec §0.2, §10 R-REC-2).

pub mod error;
#[cfg(feature = "parquet")]
pub mod parquet_export;
pub mod schema_validate;

pub use error::ExportError;
#[cfg(feature = "parquet")]
pub use parquet_export::{export_sidecar_to_parquet, ParquetExportOutcome};
pub use schema_validate::{dump_schemas_to_dir, validate_against_schema};
