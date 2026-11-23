//! `calo.table/1` -> Parquet converter gated behind `--features parquet` (spec §10 R-REC-2).

use crate::export::error::ExportError;
use crate::table::{open, ColumnData, TableError};
use calo_schema::common::{Warning, WarningCode};
use calo_schema::table::ColumnType;
use parquet::basic::{ConvertedType, Repetition, Type as PhysicalType};
use parquet::data_type::{ByteArray, ByteArrayType, DoubleType, FloatType, Int32Type, Int64Type};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;
use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;

/// Result of exporting one `calo.table/1` sidecar to Parquet (spec §10 R-REC-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParquetExportOutcome {
    /// Total rows written across all complete blocks.
    pub rows_written: usize,
    /// Warnings emitted (e.g. `TraceOverflow` on truncated tail).
    pub warnings: Vec<Warning>,
}

/// Converts a `calo.table/1` binary sidecar at `src_calotbl` into a Parquet file at `dst_parquet`
/// without modifying the original sidecar (spec §10 R-REC-2).
pub fn export_sidecar_to_parquet(
    src_calotbl: &Path,
    dst_parquet: &Path,
) -> Result<ParquetExportOutcome, ExportError> {
    let mut reader = open(src_calotbl)?;
    let header = reader.header().clone();

    if let Some(parent) = dst_parquet.parent() {
        fs::create_dir_all(parent).map_err(|source| ExportError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut fields = Vec::with_capacity(header.columns.len());
    for col in &header.columns {
        let (phys, conv) = match col.col_type {
            ColumnType::Int32 | ColumnType::Uint8 => (PhysicalType::INT32, ConvertedType::NONE),
            ColumnType::Int64 => (PhysicalType::INT64, ConvertedType::NONE),
            ColumnType::Float32 => (PhysicalType::FLOAT, ConvertedType::NONE),
            ColumnType::Float64 => (PhysicalType::DOUBLE, ConvertedType::NONE),
            ColumnType::StringId => (PhysicalType::BYTE_ARRAY, ConvertedType::UTF8),
        };
        let field = Type::primitive_type_builder(&col.name, phys)
            .with_repetition(Repetition::REQUIRED)
            .with_converted_type(conv)
            .build()
            .map_err(|e| ExportError::Parquet(e.to_string()))?;
        fields.push(Arc::new(field));
    }

    let schema = Arc::new(
        Type::group_type_builder(&header.table)
            .with_fields(fields)
            .build()
            .map_err(|e| ExportError::Parquet(e.to_string()))?,
    );

    let mut kv_meta: Vec<KeyValue> = Vec::new();
    kv_meta.push(KeyValue::new(
        "calo.table".to_string(),
        Some(header.table.clone()),
    ));
    for (k, v) in &header.meta {
        kv_meta.push(KeyValue::new(k.clone(), Some(v.to_string())));
    }

    let props = Arc::new(
        WriterProperties::builder()
            .set_dictionary_enabled(true)
            .set_key_value_metadata(Some(kv_meta))
            .build(),
    );

    let out_file = File::create(dst_parquet).map_err(|source| ExportError::Io {
        path: dst_parquet.to_path_buf(),
        source,
    })?;
    let mut writer = SerializedFileWriter::new(out_file, schema, props)
        .map_err(|e| ExportError::Parquet(e.to_string()))?;

    let mut rows_written = 0usize;
    let mut warnings = Vec::new();

    for block_res in reader.blocks() {
        let block = match block_res {
            Ok(b) => b,
            Err(TableError::TruncatedTail { good_blocks }) => {
                warnings.push(Warning {
                    code: WarningCode::TraceOverflow,
                    message: format!(
                        "truncated sidecar tail after {good_blocks} complete block(s) during Parquet export"
                    ),
                });
                break;
            }
            Err(e) => return Err(ExportError::Table(e)),
        };

        let mut rg = writer
            .next_row_group()
            .map_err(|e| ExportError::Parquet(e.to_string()))?;

        for col_def in &header.columns {
            let mut col_writer = rg
                .next_column()
                .map_err(|e| ExportError::Parquet(e.to_string()))?
                .ok_or_else(|| ExportError::Parquet("missing column writer".to_string()))?;

            match block.column(&col_def.name)? {
                ColumnData::Int32(vs) => {
                    col_writer
                        .typed::<Int32Type>()
                        .write_batch(vs, None, None)
                        .map_err(|e| ExportError::Parquet(e.to_string()))?;
                }
                ColumnData::Uint8(vs) => {
                    let widened: Vec<i32> = vs.iter().map(|&b| i32::from(b)).collect();
                    col_writer
                        .typed::<Int32Type>()
                        .write_batch(&widened, None, None)
                        .map_err(|e| ExportError::Parquet(e.to_string()))?;
                }
                ColumnData::Int64(vs) => {
                    col_writer
                        .typed::<Int64Type>()
                        .write_batch(vs, None, None)
                        .map_err(|e| ExportError::Parquet(e.to_string()))?;
                }
                ColumnData::Float32(vs) => {
                    col_writer
                        .typed::<FloatType>()
                        .write_batch(vs, None, None)
                        .map_err(|e| ExportError::Parquet(e.to_string()))?;
                }
                ColumnData::Float64(vs) => {
                    col_writer
                        .typed::<DoubleType>()
                        .write_batch(vs, None, None)
                        .map_err(|e| ExportError::Parquet(e.to_string()))?;
                }
                ColumnData::StringId(_) => {
                    let resolved = block.strings(&col_def.name, &header)?;
                    let byte_arrays: Vec<ByteArray> =
                        resolved.into_iter().map(|s| ByteArray::from(s)).collect();
                    col_writer
                        .typed::<ByteArrayType>()
                        .write_batch(&byte_arrays, None, None)
                        .map_err(|e| ExportError::Parquet(e.to_string()))?;
                }
            }

            col_writer
                .close()
                .map_err(|e| ExportError::Parquet(e.to_string()))?;
        }

        rg.close()
            .map_err(|e| ExportError::Parquet(e.to_string()))?;
        rows_written += block.rows() as usize;
    }

    writer
        .close()
        .map_err(|e| ExportError::Parquet(e.to_string()))?;

    Ok(ParquetExportOutcome {
        rows_written,
        warnings,
    })
}
