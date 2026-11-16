use calo_core::table::{
    crc32_ieee, open, open_reader, Block, ColumnData, TableError, TABLE_MAGIC,
};
use calo_schema::table::{ColumnDef, ColumnType, TableHeader};
use calo_schema::{to_yaml_string, ScalarValue, SchemaDoc};
use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

/// Test-only helper reproducing the `calo.table/1` binary format (spec §11).
fn build_test_table_bytes(header: &TableHeader, blocks: &[Block]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(TABLE_MAGIC);

    let header_yaml = to_yaml_string(header).expect("serialize header");
    let header_bytes = header_yaml.as_bytes();
    let header_len = u32::try_from(header_bytes.len()).unwrap();
    buf.extend_from_slice(&header_len.to_le_bytes());
    buf.extend_from_slice(header_bytes);

    for block in blocks {
        let mut payload = Vec::new();
        let rows = block.rows();
        payload.extend_from_slice(&rows.to_le_bytes());

        for col_def in &header.columns {
            match block.column(&col_def.name).unwrap() {
                ColumnData::Int32(vs) => {
                    for v in vs {
                        payload.extend_from_slice(&v.to_le_bytes());
                    }
                }
                ColumnData::Int64(vs) => {
                    for v in vs {
                        payload.extend_from_slice(&v.to_le_bytes());
                    }
                }
                ColumnData::Float32(vs) => {
                    for v in vs {
                        payload.extend_from_slice(&v.to_le_bytes());
                    }
                }
                ColumnData::Float64(vs) => {
                    for v in vs {
                        payload.extend_from_slice(&v.to_le_bytes());
                    }
                }
                ColumnData::Uint8(vs) => {
                    payload.extend_from_slice(vs);
                }
                ColumnData::StringId(vs) => {
                    for v in vs {
                        payload.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }

        let crc = crc32_ieee(&payload);
        buf.extend_from_slice(&payload);
        buf.extend_from_slice(&crc.to_le_bytes());
    }

    buf
}

fn sample_header() -> TableHeader {
    TableHeader {
        schema: TableHeader::SCHEMA.to_string(),
        table: "gpu.kernels".to_string(),
        columns: vec![
            ColumnDef {
                name: "start_ns".to_string(),
                col_type: ColumnType::Int64,
            },
            ColumnDef {
                name: "stream".to_string(),
                col_type: ColumnType::Int32,
            },
            ColumnDef {
                name: "occupancy".to_string(),
                col_type: ColumnType::Float32,
            },
            ColumnDef {
                name: "duration_s".to_string(),
                col_type: ColumnType::Float64,
            },
            ColumnDef {
                name: "flags".to_string(),
                col_type: ColumnType::Uint8,
            },
            ColumnDef {
                name: "name_id".to_string(),
                col_type: ColumnType::StringId,
            },
        ],
        strings: BTreeMap::from([(
            "name_id".to_string(),
            vec![
                "void step_evolve_kernel<double>()".to_string(),
                "void cufft_radix2_kernel()".to_string(),
            ],
        )]),
        meta: BTreeMap::from([
            ("device".to_string(), ScalarValue::Int(0)),
            ("clock".to_string(), ScalarValue::Str("device".to_string())),
        ]),
    }
}

fn sample_block_1() -> Block {
    Block::new(
        2,
        BTreeMap::from([
            ("start_ns".to_string(), ColumnData::Int64(vec![1000, 2000])),
            ("stream".to_string(), ColumnData::Int32(vec![1, 2])),
            (
                "occupancy".to_string(),
                ColumnData::Float32(vec![0.75, 0.50]),
            ),
            (
                "duration_s".to_string(),
                ColumnData::Float64(vec![0.00125, 0.00250]),
            ),
            ("flags".to_string(), ColumnData::Uint8(vec![1, 0])),
            ("name_id".to_string(), ColumnData::StringId(vec![0, 1])),
        ]),
    )
}

fn sample_block_2() -> Block {
    Block::new(
        1,
        BTreeMap::from([
            ("start_ns".to_string(), ColumnData::Int64(vec![3500])),
            ("stream".to_string(), ColumnData::Int32(vec![1])),
            ("occupancy".to_string(), ColumnData::Float32(vec![0.875])),
            ("duration_s".to_string(), ColumnData::Float64(vec![0.0009])),
            ("flags".to_string(), ColumnData::Uint8(vec![2])),
            ("name_id".to_string(), ColumnData::StringId(vec![0])),
        ]),
    )
}

#[test]
fn round_trip_all_column_types_and_string_dictionary() {
    let header = sample_header();
    let b1 = sample_block_1();
    let b2 = sample_block_2();
    let bytes = build_test_table_bytes(&header, &[b1.clone(), b2.clone()]);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp_path = std::env::temp_dir().join(format!("calo_tbl_rt_{nanos}.calotbl"));
    fs::write(&tmp_path, &bytes).unwrap();

    let mut reader = open(&tmp_path).expect("open table file");
    let hdr = reader.header().clone();
    assert_eq!(hdr, header);

    let read_blocks: Vec<Block> = reader
        .blocks()
        .collect::<Result<Vec<_>, _>>()
        .expect("all blocks valid");
    assert_eq!(read_blocks.len(), 2);
    assert_eq!(read_blocks[0], b1);
    assert_eq!(read_blocks[1], b2);

    // Verify typed accessors and string dictionary resolution
    assert_eq!(read_blocks[0].int64("start_ns").unwrap(), &[1000, 2000]);
    assert_eq!(read_blocks[0].int32("stream").unwrap(), &[1, 2]);
    assert_eq!(read_blocks[0].float32("occupancy").unwrap(), &[0.75, 0.50]);
    assert_eq!(
        read_blocks[0].float64("duration_s").unwrap(),
        &[0.00125, 0.00250]
    );
    assert_eq!(read_blocks[0].uint8("flags").unwrap(), &[1, 0]);
    assert_eq!(read_blocks[0].string_ids("name_id").unwrap(), &[0, 1]);
    assert_eq!(
        read_blocks[0].strings("name_id", &hdr).unwrap(),
        vec![
            "void step_evolve_kernel<double>()",
            "void cufft_radix2_kernel()"
        ]
    );

    let _ = fs::remove_file(&tmp_path);
}

#[test]
fn crc32_corruption_is_detected() {
    let header = sample_header();
    let b1 = sample_block_1();
    let mut bytes = build_test_table_bytes(&header, &[b1]);

    // Flip a bit in the block payload (before the 4-byte trailing CRC32)
    let corrupt_idx = bytes.len() - 6;
    bytes[corrupt_idx] ^= 0xFF;

    let mut reader = open_reader(Cursor::new(bytes)).unwrap();
    let mut iter = reader.blocks();
    let err = iter.next().unwrap().expect_err("corrupted block must fail CRC");
    assert!(matches!(
        err,
        TableError::CrcMismatch {
            block_index: 0,
            ..
        }
    ));
}

#[test]
fn truncated_tail_returns_good_blocks_then_distinguishable_error() {
    let header = sample_header();
    let b1 = sample_block_1();
    let b2 = sample_block_2();
    let mut bytes = build_test_table_bytes(&header, &[b1.clone(), b2]);

    // Truncate the last 5 bytes of block 2 (simulating a crash mid-write)
    bytes.truncate(bytes.len() - 5);

    let mut reader = open_reader(Cursor::new(bytes)).unwrap();
    let mut iter = reader.blocks();

    // First block is yielded intact
    let first = iter.next().unwrap().expect("first block must succeed");
    assert_eq!(first, b1);

    // Second block yields TruncatedTail { good_blocks: 1 }
    let second = iter.next().unwrap().expect_err("truncated tail must error");
    assert!(matches!(
        second,
        TableError::TruncatedTail { good_blocks: 1 }
    ));

    // Subsequent calls return None
    assert!(iter.next().is_none());
}

#[test]
fn empty_table_with_zero_blocks_succeeds() {
    let header = sample_header();
    let bytes = build_test_table_bytes(&header, &[]);

    let mut reader = open_reader(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.header(), &header);
    let blocks: Vec<_> = reader.blocks().collect();
    assert!(blocks.is_empty());
}
