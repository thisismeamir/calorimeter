//! Binary columnar sidecar reader (`calo.table/1`, spec §11).
//!
//! Endianness assumption: On-disk layout is strictly little-endian (`LE`) for all multi-byte
//! integers (`uint32`, `int32`, `int64`, `string_id`, `crc32`) and IEEE-754 floats (`float32`, `float64`).
//! All values are decoded via `from_le_bytes`, ensuring identical behavior on both little-endian
//! and big-endian host architectures without `unsafe`.

pub mod block;
pub mod crc32;
pub mod error;
pub mod reader;
pub mod rows;

pub use block::{Block, ColumnData};
pub use crc32::crc32_ieee;
pub use error::TableError;
pub use reader::{open, open_reader, BlockIter, TableReader};
pub use rows::{decode_rows, TableRow};

/// Magic 8-byte header prefix for `calo.table/1` binary sidecar files (spec §11).
pub const TABLE_MAGIC: &[u8; 8] = b"CALOTBL1";
const MAX_HEADER_SIZE: usize = 16 * 1024 * 1024; // 16 MiB
#[cfg(test)]
pub(crate) mod test_writer {
    use super::{crc32_ieee, Block, ColumnData, TABLE_MAGIC};
    use calo_schema::table::TableHeader;
    use calo_schema::to_yaml_string;
    /// Test-only helper to serialize a `TableHeader` and a slice of `Block`s into `calo.table/1` bytes.
    #[allow(dead_code)]
    pub(crate) fn write_table_bytes(header: &TableHeader, blocks: &[Block]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(TABLE_MAGIC);

        let header_yaml = to_yaml_string(header).expect("serialize table header");
        let header_bytes = header_yaml.as_bytes();
        let header_len = u32::try_from(header_bytes.len()).expect("header len fits in u32");
        buf.extend_from_slice(&header_len.to_le_bytes());
        buf.extend_from_slice(header_bytes);

        for block in blocks {
            let mut block_payload = Vec::new();
            let rows_u32 = block.rows();
            block_payload.extend_from_slice(&rows_u32.to_le_bytes());

            for col_def in &header.columns {
                let col = block
                    .column(&col_def.name)
                    .expect("column present in block");
                match col {
                    ColumnData::Int32(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        for v in vs {
                            block_payload.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    ColumnData::Int64(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        for v in vs {
                            block_payload.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    ColumnData::Float32(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        for v in vs {
                            block_payload.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    ColumnData::Float64(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        for v in vs {
                            block_payload.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    ColumnData::Uint8(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        block_payload.extend_from_slice(vs);
                    }
                    ColumnData::StringId(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        for v in vs {
                            block_payload.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    ColumnData::Uint32(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        for v in vs {
                            block_payload.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    ColumnData::Uint64(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        for v in vs {
                            block_payload.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                    ColumnData::Bool(vs) => {
                        assert_eq!(vs.len(), rows_u32 as usize);
                        block_payload.extend(vs.iter().map(|&v| u8::from(v)));
                    }
                }
            }

            let crc = crc32_ieee(&block_payload);
            buf.extend_from_slice(&block_payload);
            buf.extend_from_slice(&crc.to_le_bytes());
        }

        buf
    }
}
