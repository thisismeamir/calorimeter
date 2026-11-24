//! Streaming `calo.table/1` sidecar writer with reserved pwrite-updated string dictionary
//! header and bounded non-blocking writer thread (Spec §11, §9.2 B-O2).

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use calo_schema::{
    common::{ScalarValue, Warning, WarningCode},
    table::{ColumnType, Sidecar, TableHeader},
    tables::{
        self, GpuClockCorrelationRow, GpuCounterRow, GpuKernelRow, GpuMemcpyRow, GpuStateRow,
        MpiCollectiveRow, MpiPeerRow, MpiTraceRow, ProfileChainRow, ProfileMapRow,
        ProfileSampleRow, SeriesHostRow,
    },
    SchemaDoc,
};
use sha2::{Digest, Sha256};

use crate::series::HostSeriesRow;

/// Magic header bytes for `calo.table/1` (Spec §11).
pub const TABLE_MAGIC: &[u8; 8] = b"CALOTBL1";
/// Default reserved header region size in bytes (128 KiB).
pub const DEFAULT_HEADER_RESERVED_BYTES: usize = 128 * 1024;
/// Maximum rows per flushed block (Spec §11).
pub const MAX_BLOCK_ROWS: usize = 65_536;
/// Maximum buffered column payload bytes per block (4 MiB).
pub const MAX_BLOCK_BYTES: usize = 4 * 1024 * 1024;

impl From<HostSeriesRow> for SeriesHostRow {
    fn from(r: HostSeriesRow) -> Self {
        Self {
            t_ns: r.t_ns,
            rss_bytes: r.rss_bytes,
            utime_ns: r.utime_ns,
            stime_ns: r.stime_ns,
            power_pkg_w: r.power_pkg_w,
            power_dram_w: r.power_dram_w,
        }
    }
}

enum CellValue<'a> {
    Int32(i32),
    Int64(i64),
    Float32(f32),
    Float64(f64),
    Uint8(u8),
    Str(&'a str),
}

/// Summary returned when finishing a [`TableWriter`] or [`WriterHandle`].
#[derive(Debug, Clone)]
pub struct TableFinishOutcome {
    pub sidecar: Sidecar,
    pub dict_overflow: u64,
    pub queue_overflow: u64,
    pub writer_failed: u64,
    pub warnings: Vec<Warning>,
}

/// Streaming columnar `calo.table/1` writer.
///
/// # Crash & Dictionary Semantics
/// Reserves a fixed-size header region (`header_len` bytes, default 128 KiB) at offset 12,
/// padded with trailing spaces. `create()` `fsync`s the parent directory so the file entry
/// survives a crash; `finish()` `fsync`s the file itself.
///
/// Strings for `string_id` columns are buffered with each row and interned lazily at block
/// flush time. Whenever a block flushes and any dictionary grew, the YAML header is serialized
/// once and rewritten in place via `pwrite` (`FileExt::write_all_at`) at offset 12 without
/// per-block `fsync`. If the serialized header would exceed `header_len`, strings added since
/// the last persisted header are rolled back, `dict_frozen` is set, and unseen strings are
/// written as `-1` (`0xffff_ffffu32`) and counted in `dict_overflow`. After a crash mid-run,
/// the reader sees the last rewritten header: all numeric columns in complete CRC-valid blocks
/// are intact, and string IDs resolve for everything flushed before or with the last header
/// rewrite.
pub struct TableWriter {
    file: File,
    path: PathBuf,
    rel_path: String,
    header: TableHeader,
    header_len: usize,
    padded_header: Vec<u8>,
    dict_index: BTreeMap<String, BTreeMap<String, i32>>,
    dict_bytes_estimate: usize,
    dict_frozen: bool,
    dict_overflow: u64,
    col_bufs: Vec<Vec<u8>>,
    pending_strings: BTreeMap<usize, Vec<String>>,
    block_rows: usize,
    total_rows: u64,
    bytes_written: u64,
    block_ends: Vec<u64>,
}

impl TableWriter {
    /// Creates a new `calo.table/1` file with the default 128 KiB reserved header region.
    pub fn create(
        path: &Path,
        rel_path: impl Into<String>,
        header: TableHeader,
    ) -> io::Result<Self> {
        Self::create_with_reserved_header(path, rel_path, header, DEFAULT_HEADER_RESERVED_BYTES)
    }

    /// Creates a new `calo.table/1` file with a custom reserved header size in bytes.
    pub fn create_with_reserved_header(
        path: &Path,
        rel_path: impl Into<String>,
        mut header: TableHeader,
        header_len: usize,
    ) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        header.schema = TableHeader::SCHEMA.to_string();

        let mut dict_index = BTreeMap::new();
        let mut pending_strings = BTreeMap::new();
        let mut dict_bytes_estimate = 0usize;

        for (idx, col) in header.columns.iter().enumerate() {
            if col.col_type == ColumnType::StringId {
                let mut map = BTreeMap::new();
                if let Some(existing) = header.strings.get(&col.name) {
                    for (id, s) in existing.iter().enumerate() {
                        dict_bytes_estimate = dict_bytes_estimate.saturating_add(s.len() + 8);
                        map.insert(s.clone(), id as i32);
                    }
                } else {
                    header.strings.insert(col.name.clone(), Vec::new());
                }
                dict_index.insert(col.name.clone(), map);
                pending_strings.insert(idx, Vec::new());
            }
        }

        let padded_header = serialize_padded_header(&header, header_len)?;

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;

        let len_u32 = u32::try_from(header_len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "header_len exceeds u32::MAX")
        })?;
        file.write_all(TABLE_MAGIC)?;
        file.write_all(&len_u32.to_le_bytes())?;
        file.write_all(&padded_header)?;

        // fsync parent directory so the new file entry survives a crash.
        if let Some(parent) = path.parent() {
            let dir_path = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            if let Ok(dir) = File::open(dir_path) {
                let _ = dir.sync_all();
            }
        }

        let col_bufs = vec![Vec::new(); header.columns.len()];
        let initial_bytes = 12u64 + (header_len as u64);

        Ok(Self {
            file,
            path: path.to_path_buf(),
            rel_path: rel_path.into(),
            header,
            header_len,
            padded_header,
            dict_index,
            dict_bytes_estimate,
            dict_frozen: false,
            dict_overflow: 0,
            col_bufs,
            pending_strings,
            block_rows: 0,
            total_rows: 0,
            bytes_written: initial_bytes,
            block_ends: Vec::new(),
        })
    }

    fn header_or_io(table: &str, meta: BTreeMap<String, ScalarValue>) -> io::Result<TableHeader> {
        tables::header_for(table, meta)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))
    }

    pub fn create_host_series(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("series.host", BTreeMap::new())?,
        )
    }

    pub fn create_profile_samples(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("profile.samples", BTreeMap::new())?,
        )
    }

    pub fn create_profile_chains(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("profile.chains", BTreeMap::new())?,
        )
    }

    pub fn create_profile_maps(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("profile.maps", BTreeMap::new())?,
        )
    }

    pub fn create_gpu_state(
        path: &Path,
        rel_path: impl Into<String>,
        device_index: u32,
    ) -> io::Result<Self> {
        let mut meta = BTreeMap::new();
        meta.insert(
            "device".to_string(),
            ScalarValue::Int(i64::from(device_index)),
        );
        Self::create(path, rel_path, Self::header_or_io("gpu.state", meta)?)
    }

    pub fn create_mpi_peers(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("mpi.peers", BTreeMap::new())?,
        )
    }

    pub fn create_mpi_collectives(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("mpi.collectives", BTreeMap::new())?,
        )
    }

    pub fn create_mpi_trace(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("mpi.trace", BTreeMap::new())?,
        )
    }

    pub fn create_gpu_kernels(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("gpu.kernels", BTreeMap::new())?,
        )
    }

    pub fn create_gpu_memcpy(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("gpu.memcpy", BTreeMap::new())?,
        )
    }

    pub fn create_gpu_counters(path: &Path, rel_path: impl Into<String>) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("gpu.counters", BTreeMap::new())?,
        )
    }

    pub fn create_gpu_clock_correlation(
        path: &Path,
        rel_path: impl Into<String>,
    ) -> io::Result<Self> {
        Self::create(
            path,
            rel_path,
            Self::header_or_io("gpu.clock_correlation", BTreeMap::new())?,
        )
    }

    /// Returns the exact file byte offsets at the end of each flushed block.
    pub fn block_ends(&self) -> &[u64] {
        &self.block_ends
    }

    pub fn push_host_series(&mut self, row: &SeriesHostRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int64(row.t_ns),
            CellValue::Int64(row.rss_bytes),
            CellValue::Int64(row.utime_ns),
            CellValue::Int64(row.stime_ns),
            CellValue::Float32(row.power_pkg_w),
            CellValue::Float32(row.power_dram_w),
        ])
    }

    pub fn push_profile_sample(&mut self, row: &ProfileSampleRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int64(row.time_ns),
            CellValue::Int32(row.pid),
            CellValue::Int32(row.tid),
            CellValue::Int32(row.cpu),
            CellValue::Int64(row.ip),
            CellValue::Int64(row.chain_start),
            CellValue::Int32(row.chain_len),
        ])
    }

    pub fn push_profile_chain(&mut self, row: &ProfileChainRow) -> io::Result<()> {
        self.push_row(&[CellValue::Int64(row.ip)])
    }

    pub fn push_profile_map(&mut self, row: &ProfileMapRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int32(row.pid),
            CellValue::Int64(row.addr),
            CellValue::Int64(row.len),
            CellValue::Int64(row.pgoff),
            CellValue::Int32(row.prot),
            CellValue::Str(&row.path),
            CellValue::Str(&row.build_id),
        ])
    }

    pub fn push_mpi_peer(&mut self, row: &MpiPeerRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int32(row.comm_id),
            CellValue::Int32(row.peer_rank),
            CellValue::Uint8(row.direction),
            CellValue::Int64(row.msgs),
            CellValue::Int64(row.bytes),
        ])
    }

    pub fn push_mpi_collective(&mut self, row: &MpiCollectiveRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int32(row.comm_id),
            CellValue::Int64(row.seq),
            CellValue::Str(&row.func),
            CellValue::Int64(row.enter_ns),
            CellValue::Int64(row.exit_ns),
            CellValue::Int64(row.bytes),
        ])
    }

    pub fn push_mpi_trace(&mut self, row: &MpiTraceRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int64(row.enter_ns),
            CellValue::Int64(row.exit_ns),
            CellValue::Str(&row.func),
            CellValue::Int32(row.comm_id),
            CellValue::Int32(row.peer),
            CellValue::Int64(row.bytes),
        ])
    }

    pub fn push_gpu_state(&mut self, row: &GpuStateRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int64(row.t_ns),
            CellValue::Float32(row.util_pct),
            CellValue::Int64(row.mem_used_bytes),
            CellValue::Float32(row.power_w),
            CellValue::Int32(row.sm_mhz),
            CellValue::Int32(row.mem_mhz),
            CellValue::Float32(row.temp_c),
            CellValue::Int64(row.throttle_mask),
            CellValue::Int64(row.link_tx_bytes),
            CellValue::Int64(row.link_rx_bytes),
        ])
    }

    pub fn push_gpu_kernel(&mut self, row: &GpuKernelRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int32(row.device),
            CellValue::Int32(row.stream),
            CellValue::Str(&row.name),
            CellValue::Int64(row.start_ns),
            CellValue::Int64(row.end_ns),
            CellValue::Int32(row.grid_x),
            CellValue::Int32(row.grid_y),
            CellValue::Int32(row.grid_z),
            CellValue::Int32(row.block_x),
            CellValue::Int32(row.block_y),
            CellValue::Int32(row.block_z),
            CellValue::Int32(row.regs),
            CellValue::Int32(row.smem_bytes),
            CellValue::Int64(row.corr_id),
            CellValue::Int32(row.host_tid),
            CellValue::Str(&row.region),
        ])
    }

    pub fn push_gpu_memcpy(&mut self, row: &GpuMemcpyRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int32(row.device),
            CellValue::Int32(row.stream),
            CellValue::Uint8(row.kind),
            CellValue::Int64(row.bytes),
            CellValue::Int64(row.start_ns),
            CellValue::Int64(row.end_ns),
            CellValue::Int64(row.corr_id),
        ])
    }

    pub fn push_gpu_counter(&mut self, row: &GpuCounterRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int64(row.corr_id),
            CellValue::Str(&row.counter),
            CellValue::Float64(row.value),
        ])
    }

    pub fn push_gpu_clock_correlation(&mut self, row: &GpuClockCorrelationRow) -> io::Result<()> {
        self.push_row(&[
            CellValue::Int64(row.host_ns),
            CellValue::Int64(row.device_ns),
        ])
    }

    fn push_row(&mut self, row: &[CellValue<'_>]) -> io::Result<()> {
        if row.len() != self.header.columns.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "row column count {} does not match header {}",
                    row.len(),
                    self.header.columns.len()
                ),
            ));
        }

        let mut buffered_bytes = 0usize;
        for (i, (cell, col)) in row.iter().zip(&self.header.columns).enumerate() {
            match (cell, col.col_type) {
                (CellValue::Int32(v), ColumnType::Int32) => {
                    self.col_bufs[i].extend_from_slice(&v.to_le_bytes());
                }
                (CellValue::Int64(v), ColumnType::Int64) => {
                    self.col_bufs[i].extend_from_slice(&v.to_le_bytes());
                }
                (CellValue::Float32(v), ColumnType::Float32) => {
                    self.col_bufs[i].extend_from_slice(&v.to_le_bytes());
                }
                (CellValue::Float64(v), ColumnType::Float64) => {
                    self.col_bufs[i].extend_from_slice(&v.to_le_bytes());
                }
                (CellValue::Uint8(v), ColumnType::Uint8) => {
                    self.col_bufs[i].push(*v);
                }
                (CellValue::Str(s), ColumnType::StringId) => {
                    if let Some(pending) = self.pending_strings.get_mut(&i) {
                        pending.push((*s).to_string());
                    }
                    buffered_bytes += 4;
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("type mismatch for column '{}'", col.name),
                    ));
                }
            }
            buffered_bytes += self.col_bufs[i].len();
        }
        self.block_rows += 1;
        self.total_rows += 1;

        if self.block_rows >= MAX_BLOCK_ROWS || buffered_bytes >= MAX_BLOCK_BYTES {
            self.flush_block()?;
        }
        Ok(())
    }

    /// Resolves buffered strings for the current block, rewrites the header in place if
    /// dictionaries grew (rolling back on overflow), and flushes the block with its CRC32.
    pub fn flush_block(&mut self) -> io::Result<()> {
        if self.block_rows == 0 {
            return Ok(());
        }

        let mut staged: Vec<(String, Vec<String>)> = Vec::new();
        let mut any_added = false;
        let mut est_bytes = self.dict_bytes_estimate;

        if !self.dict_frozen {
            for (&col_idx, strings) in &self.pending_strings {
                let col_name = &self.header.columns[col_idx].name;
                let Some(col_map) = self.dict_index.get_mut(col_name) else {
                    continue;
                };
                let Some(col_vec) = self.header.strings.get_mut(col_name) else {
                    continue;
                };
                let mut added_for_col = Vec::new();
                for s in strings {
                    if !col_map.contains_key(s) {
                        let next_est = est_bytes.saturating_add(s.len() + 8);
                        if next_est > self.header_len {
                            self.dict_frozen = true;
                            break;
                        }
                        let id = col_vec.len() as i32;
                        col_vec.push(s.clone());
                        col_map.insert(s.clone(), id);
                        est_bytes = next_est;
                        added_for_col.push(s.clone());
                        any_added = true;
                    }
                }
                if !added_for_col.is_empty() {
                    staged.push((col_name.clone(), added_for_col));
                }
                if self.dict_frozen {
                    break;
                }
            }
        }

        if any_added {
            match serialize_padded_header(&self.header, self.header_len) {
                Ok(new_padded) => {
                    self.dict_bytes_estimate = est_bytes;
                    self.padded_header = new_padded;
                    self.file.write_all_at(&self.padded_header, 12)?;
                }
                Err(_) => {
                    for (col_name, added) in staged {
                        if let (Some(col_map), Some(col_vec)) = (
                            self.dict_index.get_mut(&col_name),
                            self.header.strings.get_mut(&col_name),
                        ) {
                            for s in added {
                                col_map.remove(&s);
                                col_vec.pop();
                            }
                        }
                    }
                    self.dict_frozen = true;
                }
            }
        }

        for (col_idx, strings) in &mut self.pending_strings {
            let col_name = &self.header.columns[*col_idx].name;
            let col_map = self.dict_index.get(col_name);
            let buf = &mut self.col_bufs[*col_idx];
            for s in strings.drain(..) {
                let id = col_map.and_then(|m| m.get(&s).copied()).unwrap_or_else(|| {
                    self.dict_overflow = self.dict_overflow.saturating_add(1);
                    -1
                });
                buf.extend_from_slice(&id.to_le_bytes());
            }
        }

        let rows_u32 = self.block_rows as u32;
        let rows_bytes = rows_u32.to_le_bytes();
        let mut crc = Crc32::new();
        crc.update(&rows_bytes);
        for buf in &self.col_bufs {
            crc.update(buf);
        }
        let crc_bytes = crc.finalize().to_le_bytes();

        self.file.write_all(&rows_bytes)?;
        self.bytes_written += 4;
        for buf in &mut self.col_bufs {
            self.file.write_all(buf)?;
            self.bytes_written += buf.len() as u64;
            buf.clear();
        }
        self.file.write_all(&crc_bytes)?;
        self.bytes_written += 4;
        self.file.flush()?;
        self.block_ends.push(self.bytes_written);
        self.block_rows = 0;
        Ok(())
    }

    /// Flushes any partial block, `fsync`s the file, computes SHA-256, and returns [`TableFinishOutcome`].
    pub fn finish(mut self) -> io::Result<TableFinishOutcome> {
        self.flush_block()?;
        self.file.sync_all()?;
        drop(self.file);

        let sha256 = sha256_file(&self.path)?;
        let mut warnings = Vec::new();
        if self.dict_overflow > 0 {
            warnings.push(Warning {
                code: WarningCode::TraceOverflow,
                message: format!(
                    "table '{}' string dictionary exceeded {} bytes; {} strings mapped to -1",
                    self.header.table, self.header_len, self.dict_overflow
                ),
            });
        }

        Ok(TableFinishOutcome {
            sidecar: Sidecar {
                path: self.rel_path,
                format: "calo.table/1".to_string(),
                schema: self.header.table,
                rows: self.total_rows,
                sha256,
            },
            dict_overflow: self.dict_overflow,
            queue_overflow: 0,
            writer_failed: 0,
            warnings,
        })
    }
}

fn serialize_padded_header(header: &TableHeader, header_len: usize) -> io::Result<Vec<u8>> {
    let yaml = serde_yaml_ng::to_string(header)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let bytes = yaml.as_bytes();
    if bytes.len() > header_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "serialized header ({} bytes) exceeds reserved header_len ({header_len})",
                bytes.len()
            ),
        ));
    }
    let mut padded = vec![b' '; header_len];
    padded[..bytes.len()].copy_from_slice(bytes);
    Ok(padded)
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut f, &mut hasher)?;
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
}

/// Typed commands sent over the bounded channel to the background writer thread.
pub enum WriterMsg {
    HostSeries(SeriesHostRow),
    ProfileSample(ProfileSampleRow),
    ProfileChain(ProfileChainRow),
    ProfileMap(ProfileMapRow),
    GpuState(GpuStateRow),
    MpiPeer(MpiPeerRow),
    MpiCollective(MpiCollectiveRow),
    MpiTrace(MpiTraceRow),
    GpuKernel(GpuKernelRow),
    GpuMemcpy(GpuMemcpyRow),
    GpuCounter(GpuCounterRow),
    GpuClockCorrelation(GpuClockCorrelationRow),
    FlushNow,
}

/// Non-blocking bounded-channel handle to a background [`TableWriter`] thread (Spec §9.2 B-O2).
pub struct WriterHandle {
    tx: Option<SyncSender<WriterMsg>>,
    overflow: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    join: Option<JoinHandle<io::Result<TableFinishOutcome>>>,
}

impl WriterHandle {
    /// Spawns a dedicated writer thread owning `writer` fed by a bounded `sync_channel(capacity)`.
    pub fn spawn(mut writer: TableWriter, capacity: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel::<WriterMsg>(capacity.max(1));
        let overflow = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicU64::new(0));
        let join = thread::spawn(move || {
            let mut first_err: Option<io::Error> = None;
            for msg in rx {
                let res = match msg {
                    WriterMsg::HostSeries(r) => writer.push_host_series(&r),
                    WriterMsg::ProfileSample(r) => writer.push_profile_sample(&r),
                    WriterMsg::ProfileChain(r) => writer.push_profile_chain(&r),
                    WriterMsg::ProfileMap(r) => writer.push_profile_map(&r),
                    WriterMsg::GpuState(r) => writer.push_gpu_state(&r),
                    WriterMsg::MpiPeer(r) => writer.push_mpi_peer(&r),
                    WriterMsg::MpiCollective(r) => writer.push_mpi_collective(&r),
                    WriterMsg::MpiTrace(r) => writer.push_mpi_trace(&r),
                    WriterMsg::GpuKernel(r) => writer.push_gpu_kernel(&r),
                    WriterMsg::GpuMemcpy(r) => writer.push_gpu_memcpy(&r),
                    WriterMsg::GpuCounter(r) => writer.push_gpu_counter(&r),
                    WriterMsg::GpuClockCorrelation(r) => writer.push_gpu_clock_correlation(&r),
                    WriterMsg::FlushNow => writer.flush_block(),
                };
                if let Err(e) = res {
                    first_err = Some(e);
                    break;
                }
            }
            if let Some(err) = first_err {
                // Best-effort flush/fsync of already-buffered data before propagating error.
                let _ = writer.finish();
                return Err(err);
            }
            writer.finish()
        });
        Self {
            tx: Some(tx),
            overflow,
            failed,
            join: Some(join),
        }
    }

    /// Returns the number of rows dropped due to a full writer queue.
    pub fn overflow(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }

    /// Returns the number of rows rejected because the writer thread exited/failed.
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }

    fn try_send_row(&self, msg: WriterMsg) {
        let Some(tx) = &self.tx else {
            return;
        };
        match tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.overflow.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Enqueues a flush request, retrying with 1 ms sleeps until `timeout` so a full queue
    /// does not drop a shutdown flush. Returns `true` if the flush message was enqueued.
    pub fn flush_now_blocking(&self, timeout: Duration) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        let deadline = Instant::now() + timeout;
        loop {
            match tx.try_send(WriterMsg::FlushNow) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => {
                    self.failed.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                Err(TrySendError::Full(_)) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    pub fn send_host_series(&self, row: impl Into<SeriesHostRow>) {
        self.try_send_row(WriterMsg::HostSeries(row.into()));
    }

    pub fn send_profile_sample(&self, row: ProfileSampleRow) {
        self.try_send_row(WriterMsg::ProfileSample(row));
    }

    pub fn send_profile_chain(&self, row: ProfileChainRow) {
        self.try_send_row(WriterMsg::ProfileChain(row));
    }

    pub fn send_profile_map(&self, row: ProfileMapRow) {
        self.try_send_row(WriterMsg::ProfileMap(row));
    }

    pub fn send_gpu_state(&self, row: GpuStateRow) {
        self.try_send_row(WriterMsg::GpuState(row));
    }

    pub fn send_mpi_peer(&self, row: MpiPeerRow) {
        self.try_send_row(WriterMsg::MpiPeer(row));
    }

    pub fn send_mpi_collective(&self, row: MpiCollectiveRow) {
        self.try_send_row(WriterMsg::MpiCollective(row));
    }

    pub fn send_mpi_trace(&self, row: MpiTraceRow) {
        self.try_send_row(WriterMsg::MpiTrace(row));
    }

    pub fn send_gpu_kernel(&self, row: GpuKernelRow) {
        self.try_send_row(WriterMsg::GpuKernel(row));
    }

    pub fn send_gpu_memcpy(&self, row: GpuMemcpyRow) {
        self.try_send_row(WriterMsg::GpuMemcpy(row));
    }

    pub fn send_gpu_counter(&self, row: GpuCounterRow) {
        self.try_send_row(WriterMsg::GpuCounter(row));
    }

    pub fn send_gpu_clock_correlation(&self, row: GpuClockCorrelationRow) {
        self.try_send_row(WriterMsg::GpuClockCorrelation(row));
    }

    /// Closes the channel, joins the writer thread, and returns the finalized outcome or underlying `io::Error`.
    pub fn finish(mut self) -> io::Result<TableFinishOutcome> {
        self.tx.take();
        let Some(join) = self.join.take() else {
            return Err(io::Error::other("WriterHandle::finish called twice"));
        };
        let mut outcome = join
            .join()
            .map_err(|_| io::Error::other("writer thread panicked"))??;
        outcome.queue_overflow = self.overflow();
        outcome.writer_failed = self.failed();
        if outcome.queue_overflow > 0 {
            outcome.warnings.push(Warning {
                code: WarningCode::TraceOverflow,
                message: format!(
                    "table '{}' writer queue dropped {} rows",
                    outcome.sidecar.schema, outcome.queue_overflow
                ),
            });
        }
        Ok(outcome)
    }
}

struct Crc32 {
    state: u32,
}

impl Crc32 {
    fn new() -> Self {
        Self { state: 0xffff_ffff }
    }

    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state ^= u32::from(b);
            for _ in 0..8 {
                let mask = (self.state & 1).wrapping_neg();
                self.state = (self.state >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }

    fn finalize(self) -> u32 {
        !self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn tmp_file(name: &str) -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("calo_p4_{}_{}_{name}", std::process::id(), id))
    }

    #[test]
    fn dictionary_overflow_maps_to_minus_one_and_emits_warning() {
        let p = tmp_file("dict_overflow.calotbl");
        let hdr = tables::header_for("profile.maps", BTreeMap::new()).unwrap();
        let mut writer =
            TableWriter::create_with_reserved_header(&p, "profile.maps.calotbl", hdr, 512)
                .expect("create");

        for i in 0..40 {
            writer
                .push_profile_map(&ProfileMapRow {
                    pid: 1,
                    addr: 0x1000,
                    len: 0x1000,
                    pgoff: 0,
                    prot: 5,
                    path: format!("/very/long/library/path/number_{i}/libfoo.so"),
                    build_id: format!("deadbeef{i:08x}"),
                })
                .expect("push map");
        }
        let outcome = writer.finish().expect("finish");
        assert!(outcome.dict_overflow > 0);
        assert!(outcome
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::TraceOverflow));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn writer_handle_flush_now_blocking_flushes_before_finish() {
        let p = tmp_file("flush_now.calotbl");
        let writer = TableWriter::create_host_series(&p, "series.host.calotbl").expect("create");
        let handle = WriterHandle::spawn(writer, 64);

        handle.send_host_series(SeriesHostRow {
            t_ns: 1,
            rss_bytes: 1024,
            utime_ns: 100,
            stime_ns: 50,
            power_pkg_w: f32::NAN,
            power_dram_w: f32::NAN,
        });
        assert!(handle.flush_now_blocking(Duration::from_secs(1)));

        let min_flushed = 12 + DEFAULT_HEADER_RESERVED_BYTES as u64;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut flushed = false;
        while Instant::now() < deadline {
            if fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > min_flushed {
                flushed = true;
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            flushed,
            "flush_now_blocking must flush buffered block before finish"
        );

        let outcome = handle.finish().expect("finish");
        assert_eq!(outcome.sidecar.rows, 1);
        let _ = fs::remove_file(&p);
    }

    #[test]
    #[ignore = "run in release mode for <2s timing budget"]
    fn hundred_thousand_distinct_strings_under_two_seconds() {
        let p = tmp_file("100k_strings.calotbl");
        let hdr = tables::header_for("profile.maps", BTreeMap::new()).unwrap();
        let mut writer = TableWriter::create_with_reserved_header(
            &p,
            "profile.maps.calotbl",
            hdr,
            8 * 1024 * 1024,
        )
        .expect("create");

        let t0 = Instant::now();
        for i in 0..100_000i32 {
            writer
                .push_profile_map(&ProfileMapRow {
                    pid: 1,
                    addr: i as i64,
                    len: 4096,
                    pgoff: 0,
                    prot: 5,
                    path: format!("/lib/m_{i}.so"),
                    build_id: format!("{i:08x}"),
                })
                .expect("push");
        }
        let outcome = writer.finish().expect("finish");
        assert_eq!(outcome.sidecar.rows, 100_000);
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "100k strings took {:?}",
            t0.elapsed()
        );
        let _ = fs::remove_file(&p);
    }
}
