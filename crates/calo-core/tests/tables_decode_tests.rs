use calo_core::table::{decode_rows, open_reader, Block, ColumnData, TABLE_MAGIC};
use calo_schema::table::TableHeader;
use calo_schema::tables::{header_for, ProfileMapRow, ProfileSampleRow, SeriesHostRow};
use calo_schema::to_yaml_string;
use std::collections::BTreeMap;
use std::io::Cursor;

fn build_bytes(header: &TableHeader, blocks: &[Block]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(TABLE_MAGIC);
    let hdr_yaml = to_yaml_string(header).unwrap();
    buf.extend_from_slice(&(hdr_yaml.len() as u32).to_le_bytes());
    buf.extend_from_slice(hdr_yaml.as_bytes());
    for block in blocks {
        let mut payload = Vec::new();
        payload.extend_from_slice(&block.rows().to_le_bytes());
        for col_def in &header.columns {
            match block.column(&col_def.name).unwrap() {
                ColumnData::Int32(vs) => vs
                    .iter()
                    .for_each(|v| payload.extend_from_slice(&v.to_le_bytes())),
                ColumnData::Int64(vs) => vs
                    .iter()
                    .for_each(|v| payload.extend_from_slice(&v.to_le_bytes())),
                ColumnData::Float32(vs) => vs
                    .iter()
                    .for_each(|v| payload.extend_from_slice(&v.to_le_bytes())),
                ColumnData::Float64(vs) => vs
                    .iter()
                    .for_each(|v| payload.extend_from_slice(&v.to_le_bytes())),
                ColumnData::Uint8(vs) => payload.extend_from_slice(vs),
                ColumnData::StringId(vs) => vs
                    .iter()
                    .for_each(|v| payload.extend_from_slice(&v.to_le_bytes())),
                ColumnData::Uint32(vs) => vs
                    .iter()
                    .for_each(|v| payload.extend_from_slice(&v.to_le_bytes())),
                ColumnData::Uint64(vs) => vs
                    .iter()
                    .for_each(|v| payload.extend_from_slice(&v.to_le_bytes())),
                ColumnData::Bool(vs) => vs.iter().for_each(|&v| payload.push(u8::from(v))),
            }
        }
        let crc = calo_core::table::crc32_ieee(&payload);
        buf.extend_from_slice(&payload);
        buf.extend_from_slice(&crc.to_le_bytes());
    }
    buf
}

#[test]
fn nan_for_unmeasured_float_is_preserved() {
    let hdr = header_for("series.host", BTreeMap::new()).unwrap();
    let block = Block::new(
        1,
        BTreeMap::from([
            ("t_ns".to_string(), ColumnData::Int64(vec![1_000_000])),
            ("rss_bytes".to_string(), ColumnData::Int64(vec![4096])),
            ("utime_ns".to_string(), ColumnData::Int64(vec![500_000])),
            ("stime_ns".to_string(), ColumnData::Int64(vec![100_000])),
            ("power_pkg_w".to_string(), ColumnData::Float32(vec![145.5])),
            (
                "power_dram_w".to_string(),
                ColumnData::Float32(vec![f32::NAN]),
            ),
        ]),
    );

    let bytes = build_bytes(&hdr, &[block]);
    let mut reader = open_reader(Cursor::new(bytes)).unwrap();
    let read_hdr = reader.header().clone();
    let blk = reader.blocks().next().unwrap().unwrap();
    let rows = decode_rows::<SeriesHostRow>(&blk, &read_hdr).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].power_pkg_w, 145.5);
    assert!(rows[0].power_dram_w.is_nan());
}

#[test]
fn int64_address_bit_pattern_round_trip() {
    let high_addr: u64 = 0xFFFF_8000_DEAD_BEEF;
    let high_ip: u64 = 0x0000_7FFF_FFFF_1234;

    let mut map_hdr = header_for("profile.maps", BTreeMap::new()).unwrap();
    map_hdr.strings = BTreeMap::from([
        ("path".to_string(), vec!["/usr/lib/libfoo.so".to_string()]),
        ("build_id".to_string(), vec!["deadbeef".to_string()]),
    ]);
    let map_blk = Block::new(
        1,
        BTreeMap::from([
            ("pid".to_string(), ColumnData::Int32(vec![1234])),
            (
                "addr".to_string(),
                ColumnData::Int64(vec![high_addr as i64]),
            ),
            ("len".to_string(), ColumnData::Int64(vec![0x10000])),
            ("pgoff".to_string(), ColumnData::Int64(vec![0])),
            ("prot".to_string(), ColumnData::Int32(vec![5])),
            ("path".to_string(), ColumnData::StringId(vec![0])),
            ("build_id".to_string(), ColumnData::StringId(vec![0])),
        ]),
    );
    let map_bytes = build_bytes(&map_hdr, &[map_blk]);
    let mut r1 = open_reader(Cursor::new(map_bytes)).unwrap();
    let h1 = r1.header().clone();
    let b1 = r1.blocks().next().unwrap().unwrap();
    let map_rows = decode_rows::<ProfileMapRow>(&b1, &h1).unwrap();
    assert_eq!(map_rows[0].addr_u64(), high_addr);

    let smp_hdr = header_for("profile.samples", BTreeMap::new()).unwrap();
    let smp_blk = Block::new(
        1,
        BTreeMap::from([
            ("time_ns".to_string(), ColumnData::Int64(vec![42])),
            ("pid".to_string(), ColumnData::Int32(vec![1234])),
            ("tid".to_string(), ColumnData::Int32(vec![1234])),
            ("cpu".to_string(), ColumnData::Int32(vec![3])),
            ("ip".to_string(), ColumnData::Int64(vec![high_ip as i64])),
            ("chain_start".to_string(), ColumnData::Int64(vec![0])),
            ("chain_len".to_string(), ColumnData::Int32(vec![4])),
        ]),
    );
    let smp_bytes = build_bytes(&smp_hdr, &[smp_blk]);
    let mut r2 = open_reader(Cursor::new(smp_bytes)).unwrap();
    let h2 = r2.header().clone();
    let b2 = r2.blocks().next().unwrap().unwrap();
    let smp_rows = decode_rows::<ProfileSampleRow>(&b2, &h2).unwrap();
    assert_eq!(smp_rows[0].ip_u64(), high_ip);
}
