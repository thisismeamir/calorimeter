//! Cross-check tests reading `calo-probe` `TableWriter` sidecars with the real
//! `calo-core::table` reader, asserting every column of all 12 tables, dictionary overflow,
//! and exact byte-offset truncation behavior (Spec §11).

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use calo_core::table::{decode_rows, open, ColumnData, TableError, TableRow};
use calo_probe::table_writer::{TableWriter, DEFAULT_HEADER_RESERVED_BYTES};
use calo_schema::tables::{
    self, GpuClockCorrelationRow, GpuCounterRow, GpuKernelRow, GpuMemcpyRow, GpuStateRow,
    MpiCollectiveRow, MpiPeerRow, MpiTraceRow, ProfileChainRow, ProfileMapRow, ProfileSampleRow,
    SeriesHostRow,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "calo_p4_xcheck_{}_{}_{label}",
        std::process::id(),
        id
    ));
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

fn read_all_rows<R: TableRow>(path: &std::path::Path) -> Vec<R> {
    let mut reader = open(path).expect("open table");
    let header = reader.header().clone();
    let mut out = Vec::new();
    for block_res in reader.blocks() {
        let block = block_res.expect("valid block");
        out.extend(decode_rows::<R>(&block, &header).expect("decode rows"));
    }
    out
}

#[test]
fn header_definitions_match_normative_column_specs() {
    let all_tables = [
        ("series.host", tables::SERIES_HOST_COLUMNS),
        ("profile.samples", tables::PROFILE_SAMPLES_COLUMNS),
        ("profile.chains", tables::PROFILE_CHAINS_COLUMNS),
        ("profile.maps", tables::PROFILE_MAPS_COLUMNS),
        ("mpi.peers", tables::MPI_PEERS_COLUMNS),
        ("mpi.collectives", tables::MPI_COLLECTIVES_COLUMNS),
        ("mpi.trace", tables::MPI_TRACE_COLUMNS),
        ("gpu.state", tables::GPU_STATE_COLUMNS),
        ("gpu.kernels", tables::GPU_KERNELS_COLUMNS),
        ("gpu.memcpy", tables::GPU_MEMCPY_COLUMNS),
        ("gpu.counters", tables::GPU_COUNTERS_COLUMNS),
        (
            "gpu.clock_correlation",
            tables::GPU_CLOCK_CORRELATION_COLUMNS,
        ),
    ];

    for (name, expected) in all_tables {
        let hdr = tables::header_for(name, BTreeMap::new()).expect("header_for");
        assert_eq!(
            hdr.columns.len(),
            expected.len(),
            "column count mismatch for {name}"
        );
        for (col, &(exp_name, exp_ty)) in hdr.columns.iter().zip(expected.iter()) {
            assert_eq!(col.name, exp_name, "column name mismatch in {name}");
            assert_eq!(
                col.col_type, exp_ty,
                "column type mismatch in {name}.{exp_name}"
            );
        }
    }
}

#[test]
fn all_twelve_tables_two_row_exact_column_round_trip() {
    let dir = temp_dir("all_12_exact");

    // 1. series.host (includes NaN + negative i64)
    let p = dir.join("series.host.calotbl");
    let rows = vec![
        SeriesHostRow {
            t_ns: -5,
            rss_bytes: 4096,
            utime_ns: 100,
            stime_ns: 50,
            power_pkg_w: f32::NAN,
            power_dram_w: 12.25,
        },
        SeriesHostRow {
            t_ns: 999_999,
            rss_bytes: 8192,
            utime_ns: 200,
            stime_ns: 75,
            power_pkg_w: 88.5,
            power_dram_w: f32::NAN,
        },
    ];
    let mut w = TableWriter::create_host_series(&p, "series.host.calotbl").unwrap();
    for r in &rows {
        w.push_host_series(r).unwrap();
    }
    w.finish().unwrap();
    let got: Vec<SeriesHostRow> = read_all_rows(&p);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].t_ns, -5);
    assert!(got[0].power_pkg_w.is_nan());
    assert_eq!(got[0].power_dram_w, 12.25);
    assert_eq!(got[1].power_pkg_w, 88.5);
    assert!(got[1].power_dram_w.is_nan());

    // 2. profile.samples (includes i64::MIN bit pattern for ip)
    let p = dir.join("profile.samples.calotbl");
    let rows = vec![
        ProfileSampleRow {
            time_ns: 10,
            pid: 100,
            tid: 101,
            cpu: 3,
            ip: i64::MIN,
            chain_start: 0,
            chain_len: 2,
        },
        ProfileSampleRow {
            time_ns: 20,
            pid: 100,
            tid: 102,
            cpu: 7,
            ip: -42,
            chain_start: 2,
            chain_len: 5,
        },
    ];
    let mut w = TableWriter::create_profile_samples(&p, "profile.samples.calotbl").unwrap();
    for r in &rows {
        w.push_profile_sample(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<ProfileSampleRow>(&p), rows);

    // 3. profile.chains
    let p = dir.join("profile.chains.calotbl");
    let rows = vec![
        ProfileChainRow { ip: i64::MIN },
        ProfileChainRow { ip: 0x7fff_1234 },
    ];
    let mut w = TableWriter::create_profile_chains(&p, "profile.chains.calotbl").unwrap();
    for r in &rows {
        w.push_profile_chain(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<ProfileChainRow>(&p), rows);

    // 4. profile.maps
    let p = dir.join("profile.maps.calotbl");
    let rows = vec![
        ProfileMapRow {
            pid: 10,
            addr: i64::MIN,
            len: 0x4000,
            pgoff: 0,
            prot: 5,
            path: "/bin/sh".into(),
            build_id: "deadbeef".into(),
        },
        ProfileMapRow {
            pid: 11,
            addr: 0x400000,
            len: 0x8000,
            pgoff: 0x1000,
            prot: 7,
            path: "/lib/libc.so.6".into(),
            build_id: "cafebabe".into(),
        },
    ];
    let mut w = TableWriter::create_profile_maps(&p, "profile.maps.calotbl").unwrap();
    for r in &rows {
        w.push_profile_map(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<ProfileMapRow>(&p), rows);

    // 5. mpi.peers
    let p = dir.join("mpi.peers.calotbl");
    let rows = vec![
        MpiPeerRow {
            comm_id: 1,
            peer_rank: 2,
            direction: 0,
            msgs: 10,
            bytes: 4096,
        },
        MpiPeerRow {
            comm_id: 1,
            peer_rank: 3,
            direction: 1,
            msgs: 20,
            bytes: 8192,
        },
    ];
    let mut w = TableWriter::create_mpi_peers(&p, "mpi.peers.calotbl").unwrap();
    for r in &rows {
        w.push_mpi_peer(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<MpiPeerRow>(&p), rows);

    // 6. mpi.collectives
    let p = dir.join("mpi.collectives.calotbl");
    let rows = vec![
        MpiCollectiveRow {
            comm_id: 1,
            seq: 1,
            func: "MPI_Allreduce".into(),
            enter_ns: 100,
            exit_ns: 250,
            bytes: 1024,
        },
        MpiCollectiveRow {
            comm_id: 2,
            seq: 2,
            func: "MPI_Bcast".into(),
            enter_ns: 300,
            exit_ns: 450,
            bytes: 2048,
        },
    ];
    let mut w = TableWriter::create_mpi_collectives(&p, "mpi.collectives.calotbl").unwrap();
    for r in &rows {
        w.push_mpi_collective(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<MpiCollectiveRow>(&p), rows);

    // 7. mpi.trace
    let p = dir.join("mpi.trace.calotbl");
    let rows = vec![
        MpiTraceRow {
            enter_ns: 10,
            exit_ns: 20,
            func: "MPI_Send".into(),
            comm_id: 0,
            peer: 1,
            bytes: 512,
        },
        MpiTraceRow {
            enter_ns: 30,
            exit_ns: 45,
            func: "MPI_Barrier".into(),
            comm_id: 0,
            peer: -1,
            bytes: 0,
        },
    ];
    let mut w = TableWriter::create_mpi_trace(&p, "mpi.trace.calotbl").unwrap();
    for r in &rows {
        w.push_mpi_trace(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<MpiTraceRow>(&p), rows);

    // 8. gpu.state
    let p = dir.join("gpu.state.calotbl");
    let rows = vec![
        GpuStateRow {
            t_ns: 100,
            util_pct: 95.5,
            mem_used_bytes: 1_000_000,
            power_w: 250.0,
            sm_mhz: 1600,
            mem_mhz: 1200,
            temp_c: 65.0,
            throttle_mask: i64::MIN,
            link_tx_bytes: 100,
            link_rx_bytes: 200,
        },
        GpuStateRow {
            t_ns: 200,
            util_pct: f32::NAN,
            mem_used_bytes: 2_000_000,
            power_w: f32::NAN,
            sm_mhz: 1500,
            mem_mhz: 1200,
            temp_c: f32::NAN,
            throttle_mask: 4,
            link_tx_bytes: 300,
            link_rx_bytes: 400,
        },
    ];
    let mut w = TableWriter::create_gpu_state(&p, "gpu.state.calotbl", 0).unwrap();
    for r in &rows {
        w.push_gpu_state(r).unwrap();
    }
    w.finish().unwrap();
    let got: Vec<GpuStateRow> = read_all_rows(&p);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], rows[0]);
    assert!(got[1].util_pct.is_nan());
    assert!(got[1].power_w.is_nan());
    assert!(got[1].temp_c.is_nan());
    assert_eq!(got[1].throttle_mask, 4);

    // 9. gpu.kernels
    let p = dir.join("gpu.kernels.calotbl");
    let rows = vec![
        GpuKernelRow {
            device: 0,
            stream: 1,
            name: "k_a".into(),
            start_ns: 10,
            end_ns: 20,
            grid_x: 1,
            grid_y: 2,
            grid_z: 3,
            block_x: 32,
            block_y: 4,
            block_z: 1,
            regs: 48,
            smem_bytes: 2048,
            corr_id: 101,
            host_tid: 7,
            region: "step".into(),
        },
        GpuKernelRow {
            device: 1,
            stream: 2,
            name: "k_b".into(),
            start_ns: 30,
            end_ns: 50,
            grid_x: 4,
            grid_y: 1,
            grid_z: 1,
            block_x: 64,
            block_y: 1,
            block_z: 1,
            regs: 32,
            smem_bytes: 0,
            corr_id: 102,
            host_tid: 8,
            region: "reduce".into(),
        },
    ];
    let mut w = TableWriter::create_gpu_kernels(&p, "gpu.kernels.calotbl").unwrap();
    for r in &rows {
        w.push_gpu_kernel(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<GpuKernelRow>(&p), rows);

    // 10. gpu.memcpy
    let p = dir.join("gpu.memcpy.calotbl");
    let rows = vec![
        GpuMemcpyRow {
            device: 0,
            stream: 1,
            kind: 0,
            bytes: 4096,
            start_ns: 10,
            end_ns: 15,
            corr_id: 201,
        },
        GpuMemcpyRow {
            device: 0,
            stream: 2,
            kind: 4,
            bytes: 8192,
            start_ns: 20,
            end_ns: 25,
            corr_id: 202,
        },
    ];
    let mut w = TableWriter::create_gpu_memcpy(&p, "gpu.memcpy.calotbl").unwrap();
    for r in &rows {
        w.push_gpu_memcpy(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<GpuMemcpyRow>(&p), rows);

    // 11. gpu.counters
    let p = dir.join("gpu.counters.calotbl");
    let rows = vec![
        GpuCounterRow {
            corr_id: 1,
            counter: "sm__active".into(),
            value: 82.5,
        },
        GpuCounterRow {
            corr_id: 2,
            counter: "dram__bytes".into(),
            value: f64::NAN,
        },
    ];
    let mut w = TableWriter::create_gpu_counters(&p, "gpu.counters.calotbl").unwrap();
    for r in &rows {
        w.push_gpu_counter(r).unwrap();
    }
    w.finish().unwrap();
    let got: Vec<GpuCounterRow> = read_all_rows(&p);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], rows[0]);
    assert!(got[1].value.is_nan());

    // 12. gpu.clock_correlation
    let p = dir.join("gpu.clock_correlation.calotbl");
    let rows = vec![
        GpuClockCorrelationRow {
            host_ns: -100,
            device_ns: 500,
        },
        GpuClockCorrelationRow {
            host_ns: 1000,
            device_ns: 1600,
        },
    ];
    let mut w =
        TableWriter::create_gpu_clock_correlation(&p, "gpu.clock_correlation.calotbl").unwrap();
    for r in &rows {
        w.push_gpu_clock_correlation(r).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(read_all_rows::<GpuClockCorrelationRow>(&p), rows);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dictionary_overflow_encodes_minus_one_readable_in_raw_column() {
    let dir = temp_dir("dict_minus_one");
    let p = dir.join("maps.calotbl");
    let hdr = tables::header_for("profile.maps", BTreeMap::new()).unwrap();
    let mut w = TableWriter::create_with_reserved_header(&p, "maps.calotbl", hdr, 400).unwrap();
    for i in 0..25 {
        w.push_profile_map(&ProfileMapRow {
            pid: 1,
            addr: 0x1000,
            len: 0x1000,
            pgoff: 0,
            prot: 5,
            path: format!("/overflow/path/number_{i}/libexample.so"),
            build_id: format!("id_{i:08x}"),
        })
        .unwrap();
    }
    let outcome = w.finish().unwrap();
    assert!(outcome.dict_overflow > 0);

    let mut reader = open(&p).unwrap();
    let block = reader.blocks().next().unwrap().unwrap();
    let ColumnData::StringId(ids) = block.column("path").unwrap() else {
        panic!("expected StringId column");
    };
    assert!(
        ids.contains(&u32::MAX),
        "expected overflowed string id -1 (u32::MAX) in column"
    );
    assert!(matches!(
        reader.resolve_string("path", u32::MAX),
        Err(TableError::InvalidStringId { .. })
    ));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn exact_byte_truncation_header_interior_and_block_boundaries() {
    let dir = temp_dir("truncate_exact");
    let maps_path = dir.join("profile.maps.calotbl");

    let mut w = TableWriter::create_profile_maps(&maps_path, "profile.maps.calotbl").unwrap();
    for i in 0..5i32 {
        w.push_profile_map(&ProfileMapRow {
            pid: 100,
            addr: 0x400000 + (i as i64) * 0x1000,
            len: 0x1000,
            pgoff: 0,
            prot: 5,
            path: format!("/usr/lib/lib{i}.so"),
            build_id: format!("abcd{i:04}"),
        })
        .unwrap();
    }
    w.flush_block().unwrap();
    for i in 5..8i32 {
        w.push_profile_map(&ProfileMapRow {
            pid: 100,
            addr: 0x400000 + (i as i64) * 0x1000,
            len: 0x1000,
            pgoff: 0,
            prot: 5,
            path: format!("/usr/lib/lib{i}.so"),
            build_id: format!("abcd{i:04}"),
        })
        .unwrap();
    }
    w.flush_block().unwrap();

    let block_ends = w.block_ends().to_vec();
    assert_eq!(block_ends.len(), 2);
    w.finish().unwrap();

    let full = fs::read(&maps_path).unwrap();
    let trunc_path = dir.join("truncated.calotbl");
    let header_end = 12 + DEFAULT_HEADER_RESERVED_BYTES;

    // 1. Truncation inside the header region: error, never panics.
    for len in [0usize, 1, 7, 8, 11, 12, 64, 512, header_end - 1] {
        fs::write(&trunc_path, &full[..len]).unwrap();
        assert!(
            open(&trunc_path).is_err(),
            "header truncation at {len} must return Err"
        );
    }

    // 2. Truncation exactly at block 0 boundary: exactly 1 block (5 rows), no tail error.
    let b0_end = block_ends[0] as usize;
    fs::write(&trunc_path, &full[..b0_end]).unwrap();
    let mut reader = open(&trunc_path).unwrap();
    let blocks: Vec<_> = reader.blocks().collect();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].as_ref().unwrap().rows(), 5);

    // 3. Truncation at every byte inside block 1: yields block 0 (5 rows) + TruncatedTail { good_blocks: 1 }.
    let b1_end = block_ends[1] as usize;
    for len in (b0_end + 1)..b1_end {
        fs::write(&trunc_path, &full[..len]).unwrap();
        let mut reader = open(&trunc_path).unwrap();
        let mut iter = reader.blocks();
        let b0 = iter.next().unwrap().expect("block 0 intact");
        assert_eq!(b0.rows(), 5);
        let tail = iter.next().expect("expected TruncatedTail error");
        assert!(
            matches!(tail, Err(TableError::TruncatedTail { good_blocks: 1 })),
            "at byte {len}, expected TruncatedTail {{ good_blocks: 1 }}, got: {tail:?}"
        );
        assert!(iter.next().is_none());
    }

    // 4. Truncation exactly at block 1 boundary: both blocks, no error.
    fs::write(&trunc_path, &full[..b1_end]).unwrap();
    let mut reader = open(&trunc_path).unwrap();
    let blocks: Vec<_> = reader.blocks().map(|b| b.unwrap().rows()).collect();
    assert_eq!(blocks, vec![5, 3]);

    let _ = fs::remove_dir_all(&dir);
}
