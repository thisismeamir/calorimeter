use calo_core::export::{dump_schemas_to_dir, validate_against_schema};
use calo_schema::yaml::Value;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn schema_dump_is_deterministic_and_lint_validates_schema_path() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir1 = std::env::temp_dir().join(format!("calo_sch1_{nanos}"));
    let dir2 = std::env::temp_dir().join(format!("calo_sch2_{nanos}"));

    let files1 = dump_schemas_to_dir(&dir1).unwrap();
    let files2 = dump_schemas_to_dir(&dir2).unwrap();
    assert_eq!(files1.len(), 7);

    for (p1, p2) in files1.iter().zip(files2.iter()) {
        assert_eq!(
            fs::read_to_string(p1).unwrap(),
            fs::read_to_string(p2).unwrap()
        );
    }

    // Validate a wrongly-typed field against a schema and verify JSON-pointer error path
    let suite_schema: Value = calo_schema::yaml::from_str(
        r#"
type: object
required: [schema, suite]
properties:
  schema: { const: calo.suite/1 }
  suite:
    type: object
    required: [name]
    properties:
      name: { type: string }
  benchmarks:
    type: array
    items:
      type: object
      properties:
        repeat: { type: integer }
"#,
    )
    .unwrap();

    let bad_suite: Value = calo_schema::yaml::from_str(
        r#"
schema: calo.suite/1
suite:
  name: bad
benchmarks:
  - repeat: "not_an_int"
"#,
    )
    .unwrap();

    let errs = validate_against_schema(&bad_suite, &suite_schema).unwrap_err();
    assert_eq!(errs.len(), 1);
    assert!(errs[0].to_string().contains("/benchmarks/0/repeat"));

    let _ = fs::remove_dir_all(&dir1);
    let _ = fs::remove_dir_all(&dir2);
}

#[cfg(feature = "parquet")]
#[test]
fn parquet_export_round_trip_and_truncated_tail_warning() {
    use calo_core::export::export_sidecar_to_parquet;
    use calo_core::table::{crc32_ieee, Block, ColumnData, TABLE_MAGIC};
    use calo_schema::tables::header_for;
    use calo_schema::{to_yaml_string, ScalarValue, WarningCode};
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use std::collections::BTreeMap;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("calo_pq_{nanos}"));
    fs::create_dir_all(&dir).unwrap();

    let mut hdr = header_for(
        "gpu.counters",
        BTreeMap::from([("device".to_string(), ScalarValue::Int(0))]),
    )
    .unwrap();
    hdr.strings = BTreeMap::from([(
        "counter".to_string(),
        vec!["dram__bytes_read.sum".to_string()],
    )]);

    let blk = Block::new(
        2,
        BTreeMap::from([
            ("corr_id".to_string(), ColumnData::Int64(vec![10, 20])),
            ("counter".to_string(), ColumnData::StringId(vec![0, 0])),
            ("value".to_string(), ColumnData::Float64(vec![1024.0, 2048.0])),
        ]),
    );

    let mut buf = Vec::new();
    buf.extend_from_slice(TABLE_MAGIC);
    let hy = to_yaml_string(&hdr).unwrap();
    buf.extend_from_slice(&(hy.len() as u32).to_le_bytes());
    buf.extend_from_slice(hy.as_bytes());

    let mut payload = Vec::new();
    payload.extend_from_slice(&2u32.to_le_bytes());
    for v in [10i64, 20] {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    for v in [0u32, 0] {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    for v in [1024.0f64, 2048.0] {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    let crc = crc32_ieee(&payload);
    buf.extend_from_slice(&payload);
    buf.extend_from_slice(&crc.to_le_bytes());
    // Append a truncated second block
    buf.extend_from_slice(&[1, 0, 0, 0, 99, 99]);

    let src = dir.join("counters.calotbl");
    let dst = dir.join("counters.parquet");
    fs::write(&src, &buf).unwrap();

    let outcome = export_sidecar_to_parquet(&src, &dst).unwrap();
    assert_eq!(outcome.rows_written, 2);
    assert_eq!(outcome.warnings[0].code, WarningCode::TraceOverflow);

    let pq_file = fs::File::open(&dst).unwrap();
    let pq_reader = SerializedFileReader::new(pq_file).unwrap();
    assert_eq!(pq_reader.metadata().file_metadata().num_rows(), 2);

    let _ = fs::remove_dir_all(&dir);
}
