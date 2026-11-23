//! Streaming reader for `calo.table/1` binary sidecar files (spec §11).

use crate::table::block::{Block, ColumnData};
use crate::table::crc32::crc32_ieee;
use crate::table::error::TableError;
use crate::table::{MAX_HEADER_SIZE, TABLE_MAGIC};
use calo_schema::from_yaml_str;
use calo_schema::table::{ColumnType, TableHeader};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Streaming binary sidecar reader (`calo.table/1`, spec §11).
pub struct TableReader<R: Read = BufReader<File>> {
    reader: R,
    header: TableHeader,
}

/// Opens a `calo.table/1` sidecar file from `path` and parses its header (spec §11).
pub fn open(path: impl AsRef<Path>) -> Result<TableReader<BufReader<File>>, TableError> {
    let file = File::open(path)?;
    open_reader(BufReader::new(file))
}

/// Opens a `calo.table/1` stream from any `Read` implementation and parses its header (spec §11).
pub fn open_reader<R: Read>(mut reader: R) -> Result<TableReader<R>, TableError> {
    let mut magic = [0u8; 8];
    read_exact_or_header_eof(&mut reader, &mut magic)?;
    if &magic != TABLE_MAGIC {
        return Err(TableError::InvalidMagic { found: magic });
    }

    let mut len_bytes = [0u8; 4];
    read_exact_or_header_eof(&mut reader, &mut len_bytes)?;
    let header_len = u32::from_le_bytes(len_bytes) as usize;

    // Prevent unreasonable allocations from malformed or truncated headers.
    if header_len > MAX_HEADER_SIZE {
        return Err(TableError::InvalidHeaderLength { length: header_len });
    }

    let mut header_buf = vec![0u8; header_len];
    read_exact_or_header_eof(&mut reader, &mut header_buf)?;

    let header_str = std::str::from_utf8(&header_buf)?;
    let header: TableHeader = from_yaml_str(header_str)?;

    Ok(TableReader { reader, header })
}

impl<R: Read> TableReader<R> {
    /// Returns the parsed `calo.table/1` `TableHeader`.
    pub const fn header(&self) -> &TableHeader {
        &self.header
    }

    /// Resolves a single `string_id` for `column` using the header's string dictionary.
    pub fn resolve_string(&self, column: &str, id: u32) -> Result<&str, TableError> {
        let dict = self
            .header
            .strings
            .get(column)
            .map_or(&[][..], Vec::as_slice);
        dict.get(id as usize)
            .map(String::as_str)
            .ok_or_else(|| TableError::InvalidStringId {
                column: column.to_string(),
                id,
                dict_len: dict.len(),
            })
    }

    /// Returns an iterator over all blocks in the sidecar file (spec §11).
    ///
    /// If the final block is truncated (e.g., due to a crash during streaming write),
    /// all preceding complete blocks are yielded as `Ok(Block)` followed by a final
    /// `Err(TableError::TruncatedTail { good_blocks })`.
    pub fn blocks(&mut self) -> BlockIter<'_, R> {
        BlockIter {
            reader: &mut self.reader,
            header: &self.header,
            good_blocks: 0,
            finished: false,
        }
    }
}

/// Iterator yielding decoded `Block`s from a `TableReader` (spec §11).
pub struct BlockIter<'a, R: Read> {
    reader: &'a mut R,
    header: &'a TableHeader,
    good_blocks: usize,
    finished: bool,
}

impl<R: Read> Iterator for BlockIter<'_, R> {
    type Item = Result<Block, TableError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        // Read the 4-byte `rows` prefix.
        // 0 bytes read -> clean EOF at block boundary.
        // 1..3 bytes read -> truncated tail.
        let mut rows_bytes = [0u8; 4];
        match read_chunk(self.reader, &mut rows_bytes) {
            Ok(0) => {
                self.finished = true;
                return None;
            }
            Ok(n) if n < 4 => {
                self.finished = true;
                return Some(Err(TableError::TruncatedTail {
                    good_blocks: self.good_blocks,
                }));
            }
            Ok(_) => {}
            Err(e) => {
                self.finished = true;
                return Some(Err(TableError::Io(e)));
            }
        }

        let rows = u32::from_le_bytes(rows_bytes);
        let rows_usize = rows as usize;

        let bytes_per_row: usize = self
            .header
            .columns
            .iter()
            .map(|c| c.col_type.byte_width())
            .sum();
        let data_len = rows_usize.saturating_mul(bytes_per_row);

        // Allocate buffer holding `[rows_u32_le (4B) || column_data (data_len)]` so we can CRC32 in one pass
        let mut payload = vec![0u8; 4 + data_len];
        payload[..4].copy_from_slice(&rows_bytes);

        match read_chunk(self.reader, &mut payload[4..]) {
            Ok(n) if n < data_len => {
                self.finished = true;
                return Some(Err(TableError::TruncatedTail {
                    good_blocks: self.good_blocks,
                }));
            }
            Ok(_) => {}
            Err(e) => {
                self.finished = true;
                return Some(Err(TableError::Io(e)));
            }
        }

        let mut crc_bytes = [0u8; 4];
        match read_chunk(self.reader, &mut crc_bytes) {
            Ok(n) if n < 4 => {
                self.finished = true;
                return Some(Err(TableError::TruncatedTail {
                    good_blocks: self.good_blocks,
                }));
            }
            Ok(_) => {}
            Err(e) => {
                self.finished = true;
                return Some(Err(TableError::Io(e)));
            }
        }

        let expected_crc = u32::from_le_bytes(crc_bytes);
        let actual_crc = crc32_ieee(&payload);
        if expected_crc != actual_crc {
            self.finished = true;
            return Some(Err(TableError::CrcMismatch {
                block_index: self.good_blocks,
                expected: expected_crc,
                actual: actual_crc,
            }));
        }

        // Decode contiguous columns in little-endian format
        let mut offset = 4usize;
        let mut columns = BTreeMap::new();

        for col_def in &self.header.columns {
            let col_byte_len = rows_usize * col_def.col_type.byte_width();
            let slice = &payload[offset..offset + col_byte_len];
            offset += col_byte_len;

            let col_data = decode_column(col_def.col_type, slice, rows_usize);
            columns.insert(col_def.name.clone(), col_data);
        }

        self.good_blocks += 1;
        Some(Ok(Block::new(rows, columns)))
    }
}

fn decode_column(col_type: ColumnType, slice: &[u8], rows: usize) -> ColumnData {
    match col_type {
        ColumnType::Int32 => {
            let mut out = Vec::with_capacity(rows);
            for chunk in slice.as_chunks::<4>().0 {
                out.push(i32::from_le_bytes(*chunk));
            }
            ColumnData::Int32(out)
        }
        ColumnType::Uint32 => {
            let mut out = Vec::with_capacity(rows);
            for chunk in slice.as_chunks::<4>().0 {
                out.push(u32::from_le_bytes(*chunk));
            }
            ColumnData::Uint32(out)
        }
        ColumnType::Int64 => {
            let mut out = Vec::with_capacity(rows);
            for chunk in slice.as_chunks::<8>().0 {
                out.push(i64::from_le_bytes(*chunk));
            }
            ColumnData::Int64(out)
        }
        ColumnType::Uint64 => {
            let mut out = Vec::with_capacity(rows);
            for chunk in slice.as_chunks::<8>().0 {
                out.push(u64::from_le_bytes(*chunk));
            }
            ColumnData::Uint64(out)
        }
        ColumnType::Float32 => {
            let mut out = Vec::with_capacity(rows);
            for chunk in slice.as_chunks::<4>().0 {
                out.push(f32::from_le_bytes(*chunk));
            }
            ColumnData::Float32(out)
        }
        ColumnType::Float64 => {
            let mut out = Vec::with_capacity(rows);
            for chunk in slice.as_chunks::<8>().0 {
                out.push(f64::from_le_bytes(*chunk));
            }
            ColumnData::Float64(out)
        }
        ColumnType::Uint8 => ColumnData::Uint8(slice.to_vec()),
        ColumnType::Bool => ColumnData::Bool(slice.iter().map(|&byte| byte != 0).collect()),
        ColumnType::StringId => {
            let mut out = Vec::with_capacity(rows);
            for chunk in slice.as_chunks::<4>().0 {
                out.push(u32::from_le_bytes(*chunk));
            }
            ColumnData::StringId(out)
        }
    }
}

fn read_exact_or_header_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), TableError> {
    let n = read_chunk(reader, buf)?;
    if n < buf.len() {
        return Err(TableError::UnexpectedHeaderEof);
    }
    Ok(())
}

/// Reads up to `buf.len()` bytes, retrying on `Interrupted`, until `buf` is full or EOF is reached.
fn read_chunk<R: Read>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0usize;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}
