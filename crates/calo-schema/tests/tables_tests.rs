use calo_schema::table::{ColumnDef, ColumnType, TableHeader};
use calo_schema::tables::{header_for, validate_header};
use calo_schema::{from_yaml_str, to_yaml_string, ScalarValue, SchemaError};
use std::collections::BTreeMap;

const ALL_TABLES: &[&str] = &[
    "series.host",
    "profile.samples",
    "profile.chains",
    "profile.maps",
    "mpi.peers",
    "mpi.collectives",
    "mpi.trace",
    "gpu.state",
    "gpu.kernels",
    "gpu.memcpy",
    "gpu.counters",
    "gpu.clock_correlation",
];

#[test]
fn header_round_trip_and_validation_for_every_table() {
    for &table_name in ALL_TABLES {
        let meta = BTreeMap::from([("device".to_string(), ScalarValue::Int(0))]);
        let hdr = header_for(table_name, meta).expect("known table must build header");
        validate_header(&hdr).expect("generated header must validate");

        let yaml_str = to_yaml_string(&hdr).expect("serialize header");
        let parsed: TableHeader = from_yaml_str(&yaml_str).expect("reparse header");
        assert_eq!(hdr, parsed);
        validate_header(&parsed).expect("reparsed header must validate");
    }
}

#[test]
fn validation_fails_on_missing_or_retyped_column_and_allows_trailing_extra() {
    let mut hdr = header_for("gpu.clock_correlation", BTreeMap::new()).unwrap();

    // Extra trailing column is allowed
    hdr.columns.push(ColumnDef {
        name: "extra_future_col".to_string(),
        col_type: ColumnType::Int32,
    });
    validate_header(&hdr).expect("trailing extra column allowed");

    // Retyped column is an error
    let mut retyped = hdr.clone();
    retyped.columns[0].col_type = ColumnType::Float64;
    let err = validate_header(&retyped).expect_err("retyped column must fail");
    assert!(matches!(err, SchemaError::TableValidation { .. }));

    // Missing column is an error
    let mut missing = hdr;
    missing.columns.remove(1);
    let err = validate_header(&missing).expect_err("missing column must fail");
    assert!(matches!(err, SchemaError::TableValidation { .. }));
}
