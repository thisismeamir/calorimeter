//! Typed errors for reading `calo.table/1` binary sidecar files (spec §11).

use calo_schema::SchemaError;
use thiserror::Error;

/// Errors produced when opening or reading a `calo.table/1` binary sidecar (spec §11).
#[derive(Debug, Error)]
pub enum TableError {
    /// Underlying I/O error.
    #[error("I/O error reading table: {0}")]
    Io(#[from] std::io::Error),

    /// File did not begin with the 8-byte `"CALOTBL1"` magic header (spec §11).
    #[error("invalid table magic: expected `CALOTBL1`, found `{found:?}`")]
    InvalidMagic {
        /// Raw 8-byte prefix found in the file.
        found: [u8; 8],
    },

    /// File ended before the fixed header (`magic`, `header_len`, or `header` bytes) completed.
    #[error("unexpected EOF while reading table header")]
    UnexpectedHeaderEof,

    /// Embedded header was not valid UTF-8.
    #[error("table header is not valid UTF-8: {0}")]
    InvalidHeaderUtf8(#[from] std::str::Utf8Error),

    /// Embedded YAML header failed schema validation (`calo.table/1`).
    #[error("invalid table header schema: {0}")]
    HeaderSchema(#[from] SchemaError),

    /// A trailing block was truncated mid-write (e.g. workload or node crash, spec §11).
    /// All `good_blocks` preceding this error were yielded intact.
    #[error("truncated block tail after {good_blocks} complete block(s)")]
    TruncatedTail {
        /// Number of complete blocks successfully read prior to the truncated tail.
        good_blocks: usize,
    },

    /// Block CRC32 checksum verification failed (spec §11).
    #[error(
        "CRC32 mismatch in block {block_index}: expected {expected:#010x}, computed {actual:#010x}"
    )]
    CrcMismatch {
        /// 0-based block index.
        block_index: usize,
        /// CRC32 stored at the end of the block.
        expected: u32,
        /// CRC32 computed over the block payload.
        actual: u32,
    },

    /// Requested column name does not exist in the table header.
    #[error("unknown column `{name}`")]
    UnknownColumn {
        /// Requested column name.
        name: String,
    },

    /// Requested typed column accessor did not match the column's declared `ColumnType`.
    #[error("column `{name}` type mismatch: expected `{expected}`, found `{actual}`")]
    ColumnTypeMismatch {
        /// Column name.
        name: String,
        /// Expected type name.
        expected: &'static str,
        /// Actual type name.
        actual: &'static str,
    },

    /// A `string_id` index was out of bounds for the column's string dictionary in `header.strings`.
    #[error("string_id {id} out of bounds for column `{column}` (dictionary len {dict_len})")]
    InvalidStringId {
        /// Column name.
        column: String,
        /// Out-of-bounds string dictionary index.
        id: u32,
        /// Length of the dictionary in `header.strings`.
        dict_len: usize,
    },

    /// Embedded header length exceeds the supported maximum.
    #[error("table header length {length} exceeds maximum allowed size")]
    InvalidHeaderLength { length: usize },
}
