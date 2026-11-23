//! Typed row decoding helpers plugging `calo_schema::tables` into `calo_core::table::Block` (spec §11).

use crate::table::block::Block;
use crate::table::error::TableError;
use calo_schema::TableHeader;
use calo_schema::tables::{
    validate_header, GpuClockCorrelationRow, GpuCounterRow, GpuKernelRow, GpuMemcpyRow,
    GpuStateRow, MpiCollectiveRow, MpiPeerRow, MpiTraceRow, ProfileChainRow, ProfileMapRow,
    ProfileSampleRow, SeriesHostRow,
};

/// Trait implemented by typed `calo.table/1` row structs (spec §11).
pub trait TableRow: Sized {
    /// Expected `header.table` name.
    const TABLE_NAME: &'static str;

    /// Decodes all rows in `block` using `header` for string dictionary resolution.
    fn decode_block(block: &Block, header: &TableHeader) -> Result<Vec<Self>, TableError>;
}

/// Validates `header` against `R::TABLE_NAME` and decodes all rows in `block` (spec §11).
pub fn decode_rows<R: TableRow>(
    block: &Block,
    header: &TableHeader,
) -> Result<Vec<R>, TableError> {
    validate_header(header)?;
    if header.table != R::TABLE_NAME {
        return Err(TableError::HeaderSchema(
            calo_schema::SchemaError::TableValidation {
                table: header.table.clone(),
                reason: format!("expected table `{}`, found `{}`", R::TABLE_NAME, header.table),
            },
        ));
    }
    R::decode_block(block, header)
}

impl TableRow for SeriesHostRow {
    const TABLE_NAME: &'static str = "series.host";

    fn decode_block(block: &Block, _header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let t_ns = block.int64("t_ns")?;
        let rss_bytes = block.int64("rss_bytes")?;
        let utime_ns = block.int64("utime_ns")?;
        let stime_ns = block.int64("stime_ns")?;
        let power_pkg_w = block.float32("power_pkg_w")?;
        let power_dram_w = block.float32("power_dram_w")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                t_ns: t_ns[i],
                rss_bytes: rss_bytes[i],
                utime_ns: utime_ns[i],
                stime_ns: stime_ns[i],
                power_pkg_w: power_pkg_w[i],
                power_dram_w: power_dram_w[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for ProfileSampleRow {
    const TABLE_NAME: &'static str = "profile.samples";

    fn decode_block(block: &Block, _header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let time_ns = block.int64("time_ns")?;
        let pid = block.int32("pid")?;
        let tid = block.int32("tid")?;
        let cpu = block.int32("cpu")?;
        let ip = block.int64("ip")?;
        let chain_start = block.int64("chain_start")?;
        let chain_len = block.int32("chain_len")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                time_ns: time_ns[i],
                pid: pid[i],
                tid: tid[i],
                cpu: cpu[i],
                ip: ip[i],
                chain_start: chain_start[i],
                chain_len: chain_len[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for ProfileChainRow {
    const TABLE_NAME: &'static str = "profile.chains";

    fn decode_block(block: &Block, _header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let ip = block.int64("ip")?;
        Ok(ip.iter().map(|&v| Self { ip: v }).collect())
    }
}

impl TableRow for ProfileMapRow {
    const TABLE_NAME: &'static str = "profile.maps";

    fn decode_block(block: &Block, header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let pid = block.int32("pid")?;
        let addr = block.int64("addr")?;
        let len = block.int64("len")?;
        let pgoff = block.int64("pgoff")?;
        let prot = block.int32("prot")?;
        let path = block.strings("path", header)?;
        let build_id = block.strings("build_id", header)?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                pid: pid[i],
                addr: addr[i],
                len: len[i],
                pgoff: pgoff[i],
                prot: prot[i],
                path: path[i].to_string(),
                build_id: build_id[i].to_string(),
            });
        }
        Ok(rows)
    }
}

impl TableRow for MpiPeerRow {
    const TABLE_NAME: &'static str = "mpi.peers";

    fn decode_block(block: &Block, _header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let comm_id = block.int32("comm_id")?;
        let peer_rank = block.int32("peer_rank")?;
        let direction = block.uint8("direction")?;
        let msgs = block.int64("msgs")?;
        let bytes = block.int64("bytes")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                comm_id: comm_id[i],
                peer_rank: peer_rank[i],
                direction: direction[i],
                msgs: msgs[i],
                bytes: bytes[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for MpiCollectiveRow {
    const TABLE_NAME: &'static str = "mpi.collectives";

    fn decode_block(block: &Block, header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let comm_id = block.int32("comm_id")?;
        let seq = block.int64("seq")?;
        let func = block.strings("func", header)?;
        let enter_ns = block.int64("enter_ns")?;
        let exit_ns = block.int64("exit_ns")?;
        let bytes = block.int64("bytes")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                comm_id: comm_id[i],
                seq: seq[i],
                func: func[i].to_string(),
                enter_ns: enter_ns[i],
                exit_ns: exit_ns[i],
                bytes: bytes[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for MpiTraceRow {
    const TABLE_NAME: &'static str = "mpi.trace";

    fn decode_block(block: &Block, header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let enter_ns = block.int64("enter_ns")?;
        let exit_ns = block.int64("exit_ns")?;
        let func = block.strings("func", header)?;
        let comm_id = block.int32("comm_id")?;
        let peer = block.int32("peer")?;
        let bytes = block.int64("bytes")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                enter_ns: enter_ns[i],
                exit_ns: exit_ns[i],
                func: func[i].to_string(),
                comm_id: comm_id[i],
                peer: peer[i],
                bytes: bytes[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for GpuStateRow {
    const TABLE_NAME: &'static str = "gpu.state";

    fn decode_block(block: &Block, _header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let t_ns = block.int64("t_ns")?;
        let util_pct = block.float32("util_pct")?;
        let mem_used_bytes = block.int64("mem_used_bytes")?;
        let power_w = block.float32("power_w")?;
        let sm_mhz = block.int32("sm_mhz")?;
        let mem_mhz = block.int32("mem_mhz")?;
        let temp_c = block.float32("temp_c")?;
        let throttle_mask = block.int64("throttle_mask")?;
        let link_tx_bytes = block.int64("link_tx_bytes")?;
        let link_rx_bytes = block.int64("link_rx_bytes")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                t_ns: t_ns[i],
                util_pct: util_pct[i],
                mem_used_bytes: mem_used_bytes[i],
                power_w: power_w[i],
                sm_mhz: sm_mhz[i],
                mem_mhz: mem_mhz[i],
                temp_c: temp_c[i],
                throttle_mask: throttle_mask[i],
                link_tx_bytes: link_tx_bytes[i],
                link_rx_bytes: link_rx_bytes[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for GpuKernelRow {
    const TABLE_NAME: &'static str = "gpu.kernels";

    fn decode_block(block: &Block, header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let device = block.int32("device")?;
        let stream = block.int32("stream")?;
        let name = block.strings("name", header)?;
        let start_ns = block.int64("start_ns")?;
        let end_ns = block.int64("end_ns")?;
        let grid_x = block.int32("grid_x")?;
        let grid_y = block.int32("grid_y")?;
        let grid_z = block.int32("grid_z")?;
        let block_x = block.int32("block_x")?;
        let block_y = block.int32("block_y")?;
        let block_z = block.int32("block_z")?;
        let regs = block.int32("regs")?;
        let smem_bytes = block.int32("smem_bytes")?;
        let corr_id = block.int64("corr_id")?;
        let host_tid = block.int32("host_tid")?;
        let region = block.strings("region", header)?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                device: device[i],
                stream: stream[i],
                name: name[i].to_string(),
                start_ns: start_ns[i],
                end_ns: end_ns[i],
                grid_x: grid_x[i],
                grid_y: grid_y[i],
                grid_z: grid_z[i],
                block_x: block_x[i],
                block_y: block_y[i],
                block_z: block_z[i],
                regs: regs[i],
                smem_bytes: smem_bytes[i],
                corr_id: corr_id[i],
                host_tid: host_tid[i],
                region: region[i].to_string(),
            });
        }
        Ok(rows)
    }
}

impl TableRow for GpuMemcpyRow {
    const TABLE_NAME: &'static str = "gpu.memcpy";

    fn decode_block(block: &Block, _header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let device = block.int32("device")?;
        let stream = block.int32("stream")?;
        let kind = block.uint8("kind")?;
        let bytes = block.int64("bytes")?;
        let start_ns = block.int64("start_ns")?;
        let end_ns = block.int64("end_ns")?;
        let corr_id = block.int64("corr_id")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                device: device[i],
                stream: stream[i],
                kind: kind[i],
                bytes: bytes[i],
                start_ns: start_ns[i],
                end_ns: end_ns[i],
                corr_id: corr_id[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for GpuCounterRow {
    const TABLE_NAME: &'static str = "gpu.counters";

    fn decode_block(block: &Block, header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let corr_id = block.int64("corr_id")?;
        let counter = block.strings("counter", header)?;
        let value = block.float64("value")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                corr_id: corr_id[i],
                counter: counter[i].to_string(),
                value: value[i],
            });
        }
        Ok(rows)
    }
}

impl TableRow for GpuClockCorrelationRow {
    const TABLE_NAME: &'static str = "gpu.clock_correlation";

    fn decode_block(block: &Block, _header: &TableHeader) -> Result<Vec<Self>, TableError> {
        let n = block.rows() as usize;
        let host_ns = block.int64("host_ns")?;
        let device_ns = block.int64("device_ns")?;

        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            rows.push(Self {
                host_ns: host_ns[i],
                device_ns: device_ns[i],
            });
        }
        Ok(rows)
    }
}
