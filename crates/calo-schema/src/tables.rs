//! Normative `calo.table/1` sidecar column definitions and typed row structs (spec §11).

use crate::common::ScalarValue;
use crate::error::SchemaError;
use crate::table::{ColumnDef, ColumnType, TableHeader};
use crate::SchemaDoc;
use std::collections::BTreeMap;

/// Column specification tuple `(name, type)` for a sidecar table (spec §11).
pub type ColSpec = (&'static str, ColumnType);

/// Columns for `series.host` (spec §11).
pub const SERIES_HOST_COLUMNS: &[ColSpec] = &[
    ("t_ns", ColumnType::Int64),
    ("rss_bytes", ColumnType::Int64),
    ("utime_ns", ColumnType::Int64),
    ("stime_ns", ColumnType::Int64),
    ("power_pkg_w", ColumnType::Float32),
    ("power_dram_w", ColumnType::Float32),
];

/// Columns for `profile.samples` (spec §11).
pub const PROFILE_SAMPLES_COLUMNS: &[ColSpec] = &[
    ("time_ns", ColumnType::Int64),
    ("pid", ColumnType::Int32),
    ("tid", ColumnType::Int32),
    ("cpu", ColumnType::Int32),
    ("ip", ColumnType::Int64),
    ("chain_start", ColumnType::Int64),
    ("chain_len", ColumnType::Int32),
];

/// Columns for `profile.chains` (spec §11).
pub const PROFILE_CHAINS_COLUMNS: &[ColSpec] = &[("ip", ColumnType::Int64)];

/// Columns for `profile.maps` (spec §11).
pub const PROFILE_MAPS_COLUMNS: &[ColSpec] = &[
    ("pid", ColumnType::Int32),
    ("addr", ColumnType::Int64),
    ("len", ColumnType::Int64),
    ("pgoff", ColumnType::Int64),
    ("prot", ColumnType::Int32),
    ("path", ColumnType::StringId),
    ("build_id", ColumnType::StringId),
];

/// Columns for `mpi.peers` (spec §11).
pub const MPI_PEERS_COLUMNS: &[ColSpec] = &[
    ("comm_id", ColumnType::Int32),
    ("peer_rank", ColumnType::Int32),
    ("direction", ColumnType::Uint8),
    ("msgs", ColumnType::Int64),
    ("bytes", ColumnType::Int64),
];

/// Columns for `mpi.collectives` (spec §11).
pub const MPI_COLLECTIVES_COLUMNS: &[ColSpec] = &[
    ("comm_id", ColumnType::Int32),
    ("seq", ColumnType::Int64),
    ("func", ColumnType::StringId),
    ("enter_ns", ColumnType::Int64),
    ("exit_ns", ColumnType::Int64),
    ("bytes", ColumnType::Int64),
];

/// Columns for `mpi.trace` (spec §11).
pub const MPI_TRACE_COLUMNS: &[ColSpec] = &[
    ("enter_ns", ColumnType::Int64),
    ("exit_ns", ColumnType::Int64),
    ("func", ColumnType::StringId),
    ("comm_id", ColumnType::Int32),
    ("peer", ColumnType::Int32),
    ("bytes", ColumnType::Int64),
];

/// Columns for `gpu.state` (spec §11).
pub const GPU_STATE_COLUMNS: &[ColSpec] = &[
    ("t_ns", ColumnType::Int64),
    ("util_pct", ColumnType::Float32),
    ("mem_used_bytes", ColumnType::Int64),
    ("power_w", ColumnType::Float32),
    ("sm_mhz", ColumnType::Int32),
    ("mem_mhz", ColumnType::Int32),
    ("temp_c", ColumnType::Float32),
    ("throttle_mask", ColumnType::Int64),
    ("link_tx_bytes", ColumnType::Int64),
    ("link_rx_bytes", ColumnType::Int64),
];

/// Columns for `gpu.kernels` (spec §11).
pub const GPU_KERNELS_COLUMNS: &[ColSpec] = &[
    ("device", ColumnType::Int32),
    ("stream", ColumnType::Int32),
    ("name", ColumnType::StringId),
    ("start_ns", ColumnType::Int64),
    ("end_ns", ColumnType::Int64),
    ("grid_x", ColumnType::Int32),
    ("grid_y", ColumnType::Int32),
    ("grid_z", ColumnType::Int32),
    ("block_x", ColumnType::Int32),
    ("block_y", ColumnType::Int32),
    ("block_z", ColumnType::Int32),
    ("regs", ColumnType::Int32),
    ("smem_bytes", ColumnType::Int32),
    ("corr_id", ColumnType::Int64),
    ("host_tid", ColumnType::Int32),
    ("region", ColumnType::StringId),
];

/// Columns for `gpu.memcpy` (spec §11).
pub const GPU_MEMCPY_COLUMNS: &[ColSpec] = &[
    ("device", ColumnType::Int32),
    ("stream", ColumnType::Int32),
    ("kind", ColumnType::Uint8),
    ("bytes", ColumnType::Int64),
    ("start_ns", ColumnType::Int64),
    ("end_ns", ColumnType::Int64),
    ("corr_id", ColumnType::Int64),
];

/// Columns for `gpu.counters` (spec §11).
pub const GPU_COUNTERS_COLUMNS: &[ColSpec] = &[
    ("corr_id", ColumnType::Int64),
    ("counter", ColumnType::StringId),
    ("value", ColumnType::Float64),
];

/// Columns for `gpu.clock_correlation` (spec §11).
pub const GPU_CLOCK_CORRELATION_COLUMNS: &[ColSpec] = &[
    ("host_ns", ColumnType::Int64),
    ("device_ns", ColumnType::Int64),
];

/// Typed row for `series.host` (spec §11).
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesHostRow {
    /// Monotonic sample timestamp in nanoseconds.
    pub t_ns: i64,
    /// Resident set size in bytes.
    pub rss_bytes: i64,
    /// User CPU time in nanoseconds.
    pub utime_ns: i64,
    /// System CPU time in nanoseconds.
    pub stime_ns: i64,
    /// CPU package power in Watts (`NaN` if unmeasured).
    pub power_pkg_w: f32,
    /// DRAM power in Watts (`NaN` if unmeasured).
    pub power_dram_w: f32,
}

/// Typed row for `profile.samples` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSampleRow {
    /// Sample monotonic timestamp in nanoseconds.
    pub time_ns: i64,
    /// Process ID.
    pub pid: i32,
    /// Thread ID.
    pub tid: i32,
    /// CPU core ID.
    pub cpu: i32,
    /// Instruction pointer stored as `i64` bit pattern.
    pub ip: i64,
    /// Start index into `profile.chains`.
    pub chain_start: i64,
    /// Number of frames in the callchain.
    pub chain_len: i32,
}

impl ProfileSampleRow {
    /// Returns the instruction pointer as an unsigned 64-bit address (spec §11).
    pub const fn ip_u64(&self) -> u64 {
        self.ip as u64
    }
}

/// Typed row for `profile.chains` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChainRow {
    /// Callchain instruction pointer stored as `i64` bit pattern.
    pub ip: i64,
}

impl ProfileChainRow {
    /// Returns the instruction pointer as an unsigned 64-bit address (spec §11).
    pub const fn ip_u64(&self) -> u64 {
        self.ip as u64
    }
}

/// Typed row for `profile.maps` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMapRow {
    /// Process ID.
    pub pid: i32,
    /// Mapping base address stored as `i64` bit pattern.
    pub addr: i64,
    /// Mapping length in bytes.
    pub len: i64,
    /// Page offset in backing file.
    pub pgoff: i64,
    /// Memory protection flags.
    pub prot: i32,
    /// Resolved mapped file path.
    pub path: String,
    /// Resolved ELF build-id string.
    pub build_id: String,
}

impl ProfileMapRow {
    /// Returns `addr` as an unsigned 64-bit address (spec §11).
    pub const fn addr_u64(&self) -> u64 {
        self.addr as u64
    }
}

/// Typed row for `mpi.peers` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpiPeerRow {
    /// Communicator ID.
    pub comm_id: i32,
    /// Peer rank index.
    pub peer_rank: i32,
    /// Direction (`0` = send, `1` = recv).
    pub direction: u8,
    /// Message count.
    pub msgs: i64,
    /// Total bytes transferred.
    pub bytes: i64,
}

/// Typed row for `mpi.collectives` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpiCollectiveRow {
    /// Communicator ID.
    pub comm_id: i32,
    /// Collective sequence number within communicator.
    pub seq: i64,
    /// Resolved MPI collective function name.
    pub func: String,
    /// Entry monotonic timestamp in nanoseconds.
    pub enter_ns: i64,
    /// Exit monotonic timestamp in nanoseconds.
    pub exit_ns: i64,
    /// Total bytes transferred.
    pub bytes: i64,
}

/// Typed row for `mpi.trace` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpiTraceRow {
    /// Entry monotonic timestamp in nanoseconds.
    pub enter_ns: i64,
    /// Exit monotonic timestamp in nanoseconds.
    pub exit_ns: i64,
    /// Resolved MPI function name.
    pub func: String,
    /// Communicator ID.
    pub comm_id: i32,
    /// Peer rank (or `-1` if not point-to-point).
    pub peer: i32,
    /// Bytes transferred.
    pub bytes: i64,
}

/// Typed row for `gpu.state` (spec §11).
#[derive(Debug, Clone, PartialEq)]
pub struct GpuStateRow {
    /// Monotonic sample timestamp in nanoseconds.
    pub t_ns: i64,
    /// GPU compute utilization percentage (`NaN` if unmeasured).
    pub util_pct: f32,
    /// Device memory used in bytes.
    pub mem_used_bytes: i64,
    /// Device power draw in Watts (`NaN` if unmeasured).
    pub power_w: f32,
    /// Streaming multiprocessor / CU clock in MHz.
    pub sm_mhz: i32,
    /// Device memory clock in MHz.
    pub mem_mhz: i32,
    /// Device temperature in Celsius (`NaN` if unmeasured).
    pub temp_c: f32,
    /// Vendor throttle reason bitmask stored as `i64`.
    pub throttle_mask: i64,
    /// Interconnect transmit bytes.
    pub link_tx_bytes: i64,
    /// Interconnect receive bytes.
    pub link_rx_bytes: i64,
}

/// Typed row for `gpu.kernels` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuKernelRow {
    /// GPU device ordinal.
    pub device: i32,
    /// Stream / queue ID.
    pub stream: i32,
    /// Resolved kernel name.
    pub name: String,
    /// Device start timestamp in nanoseconds.
    pub start_ns: i64,
    /// Device end timestamp in nanoseconds.
    pub end_ns: i64,
    /// Grid X dimension.
    pub grid_x: i32,
    /// Grid Y dimension.
    pub grid_y: i32,
    /// Grid Z dimension.
    pub grid_z: i32,
    /// Block X dimension.
    pub block_x: i32,
    /// Block Y dimension.
    pub block_y: i32,
    /// Block Z dimension.
    pub block_z: i32,
    /// Registers per thread.
    pub regs: i32,
    /// Dynamic/static shared memory bytes.
    pub smem_bytes: i32,
    /// Host-device correlation ID.
    pub corr_id: i64,
    /// Host launcher thread ID.
    pub host_tid: i32,
    /// Resolved host region name active at launch.
    pub region: String,
}

/// Typed row for `gpu.memcpy` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuMemcpyRow {
    /// GPU device ordinal.
    pub device: i32,
    /// Stream / queue ID.
    pub stream: i32,
    /// Operation kind (`0` H2D, `1` D2H, `2` D2D, `3` P2P, `4` memset).
    pub kind: u8,
    /// Bytes transferred or set.
    pub bytes: i64,
    /// Device start timestamp in nanoseconds.
    pub start_ns: i64,
    /// Device end timestamp in nanoseconds.
    pub end_ns: i64,
    /// Host-device correlation ID.
    pub corr_id: i64,
}

/// Typed row for `gpu.counters` (spec §11).
#[derive(Debug, Clone, PartialEq)]
pub struct GpuCounterRow {
    /// Kernel correlation ID.
    pub corr_id: i64,
    /// Resolved counter name.
    pub counter: String,
    /// Counter value (`NaN` if unmeasured).
    pub value: f64,
}

/// Typed row for `gpu.clock_correlation` (spec §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuClockCorrelationRow {
    /// Host `CLOCK_MONOTONIC_RAW` timestamp in nanoseconds.
    pub host_ns: i64,
    /// Device clock timestamp in nanoseconds.
    pub device_ns: i64,
}

/// Looks up the normative column specification for a known `calo.table/1` table name (spec §11).
pub fn columns_for_table(table: &str) -> Option<&'static [ColSpec]> {
    match table {
        "series.host" => Some(SERIES_HOST_COLUMNS),
        "profile.samples" => Some(PROFILE_SAMPLES_COLUMNS),
        "profile.chains" => Some(PROFILE_CHAINS_COLUMNS),
        "profile.maps" => Some(PROFILE_MAPS_COLUMNS),
        "mpi.peers" => Some(MPI_PEERS_COLUMNS),
        "mpi.collectives" => Some(MPI_COLLECTIVES_COLUMNS),
        "mpi.trace" => Some(MPI_TRACE_COLUMNS),
        "gpu.state" => Some(GPU_STATE_COLUMNS),
        "gpu.kernels" => Some(GPU_KERNELS_COLUMNS),
        "gpu.memcpy" => Some(GPU_MEMCPY_COLUMNS),
        "gpu.counters" => Some(GPU_COUNTERS_COLUMNS),
        "gpu.clock_correlation" => Some(GPU_CLOCK_CORRELATION_COLUMNS),
        _ => None,
    }
}

/// Constructs a `TableHeader` for `table` with `meta` using the normative column definitions (spec §11).
pub fn header_for(
    table: &str,
    meta: BTreeMap<String, ScalarValue>,
) -> Result<TableHeader, SchemaError> {
    let specs = columns_for_table(table).ok_or_else(|| SchemaError::TableValidation {
        table: table.to_string(),
        reason: format!("unknown table `{table}`"),
    })?;

    let columns = specs
        .iter()
        .map(|&(name, col_type)| ColumnDef {
            name: name.to_string(),
            col_type,
        })
        .collect();

    Ok(TableHeader {
        schema: TableHeader::SCHEMA.to_string(),
        table: table.to_string(),
        columns,
        strings: BTreeMap::new(),
        meta,
    })
}

/// Validates a `TableHeader` against its normative column definition (spec §11).
///
/// Missing or retyped required columns are rejected; extra trailing columns are permitted
/// for forward compatibility (spec §18).
pub fn validate_header(header: &TableHeader) -> Result<(), SchemaError> {
    header.check()?;
    let specs = columns_for_table(&header.table).ok_or_else(|| SchemaError::TableValidation {
        table: header.table.clone(),
        reason: format!("unknown table `{}`", header.table),
    })?;

    for &(expected_name, expected_type) in specs {
        let Some(found_col) = header.columns.iter().find(|c| c.name == expected_name) else {
            return Err(SchemaError::TableValidation {
                table: header.table.clone(),
                reason: format!("missing required column `{expected_name}`"),
            });
        };
        if found_col.col_type != expected_type {
            return Err(SchemaError::TableValidation {
                table: header.table.clone(),
                reason: format!(
                    "column `{expected_name}` has type `{:?}`, expected `{:?}`",
                    found_col.col_type, expected_type
                ),
            });
        }
    }

    Ok(())
}
