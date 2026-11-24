//! M1 PMPI aggregation core and narrow C ABI for `calo-shim-mpi` (Spec §9.5 B17–B23, §16 R-SAF-4).
//!
//! `unsafe` is strictly confined to the `extern "C"` entry points and `clock_gettime`.

use std::{
    collections::BTreeMap,
    ffi::{c_char, CStr},
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

use calo_schema::record::{MpiFunctionSummary, RecordMpi};

/// 16 power-of-two message-size histogram bins:
/// `[0, <=64B, <=128B, <=256B, ..., <=512KiB, <=1MiB, >1MiB]`.
const HIST_BINS: usize = 16;

static SHIM_DISABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<MpiAggregator>> = Mutex::new(None);

#[derive(Debug, Clone)]
struct FuncAccum {
    calls: u64,
    total_ns: u64,
    min_ns: u64,
    max_ns: u64,
    bytes_sent: u64,
    bytes_recv: u64,
    size_hist: [u64; HIST_BINS],
}

impl FuncAccum {
    fn new(dur_ns: u64, sent: u64, recv: u64) -> Self {
        let mut hist = [0u64; HIST_BINS];
        hist[size_bin(sent.max(recv))] = 1;
        Self {
            calls: 1,
            total_ns: dur_ns,
            min_ns: dur_ns,
            max_ns: dur_ns,
            bytes_sent: sent,
            bytes_recv: recv,
            size_hist: hist,
        }
    }

    fn record(&mut self, dur_ns: u64, sent: u64, recv: u64) {
        self.calls = self.calls.saturating_add(1);
        self.total_ns = self.total_ns.saturating_add(dur_ns);
        self.min_ns = self.min_ns.min(dur_ns);
        self.max_ns = self.max_ns.max(dur_ns);
        self.bytes_sent = self.bytes_sent.saturating_add(sent);
        self.bytes_recv = self.bytes_recv.saturating_add(recv);
        self.size_hist[size_bin(sent.max(recv))] += 1;
    }
}

/// Sparse per-peer point-to-point entry (Spec §9.5 B17).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerAccum {
    pub msgs: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub total_ns: u64,
}

/// Collective call instance entry indexed by `(comm, seq)` (Spec §9.5 B18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectiveEntry {
    pub name: String,
    pub comm: String,
    pub seq: u64,
    pub start_ns: i64,
    pub end_ns: i64,
    pub bytes: u64,
}

/// Thread-safe M1 MPI aggregator (supports `MPI_THREAD_MULTIPLE`, Spec §9.5 B21).
#[derive(Debug)]
pub struct MpiAggregator {
    pub rank: i32,
    pub size: i32,
    pub thread_level: String,
    pub shim_overhead_ns_per_call: f64,
    pub out_dir: Option<PathBuf>,
    funcs: BTreeMap<(String, String), FuncAccum>,
    peers: BTreeMap<i32, PeerAccum>,
    collectives: Vec<CollectiveEntry>,
}

impl MpiAggregator {
    pub fn new(
        rank: i32,
        size: i32,
        thread_level: impl Into<String>,
        overhead_ns: f64,
        out_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            rank,
            size,
            thread_level: thread_level.into(),
            shim_overhead_ns_per_call: overhead_ns,
            out_dir,
            funcs: BTreeMap::new(),
            peers: BTreeMap::new(),
            collectives: Vec::new(),
        }
    }

    pub fn record_call(
        &mut self,
        name: &str,
        comm_label: &str,
        start_ns: i64,
        end_ns: i64,
        bytes_sent: u64,
        bytes_recv: u64,
        peer: i32,
    ) {
        let dur_ns = end_ns.saturating_sub(start_ns).max(0) as u64;
        let key = (name.to_string(), comm_label.to_string());
        self.funcs
            .entry(key)
            .and_modify(|acc| acc.record(dur_ns, bytes_sent, bytes_recv))
            .or_insert_with(|| FuncAccum::new(dur_ns, bytes_sent, bytes_recv));

        if peer >= 0 {
            let p = self.peers.entry(peer).or_default();
            p.msgs = p.msgs.saturating_add(1);
            p.bytes_sent = p.bytes_sent.saturating_add(bytes_sent);
            p.bytes_recv = p.bytes_recv.saturating_add(bytes_recv);
            p.total_ns = p.total_ns.saturating_add(dur_ns);
        }
    }

    pub fn record_collective(
        &mut self,
        name: &str,
        comm_label: &str,
        seq: u64,
        start_ns: i64,
        end_ns: i64,
        bytes: u64,
    ) {
        const MAX_COLLECTIVES: usize = 200_000;
        if self.collectives.len() < MAX_COLLECTIVES {
            self.collectives.push(CollectiveEntry {
                name: name.to_string(),
                comm: comm_label.to_string(),
                seq,
                start_ns,
                end_ns,
                bytes,
            });
        }
    }

    pub fn peers(&self) -> &BTreeMap<i32, PeerAccum> {
        &self.peers
    }

    pub fn collectives(&self) -> &[CollectiveEntry] {
        &self.collectives
    }

    /// Builds a `calo_schema::record::RecordMpi` summary.
    pub fn to_record_mpi(&self) -> RecordMpi {
        let functions = self
            .funcs
            .iter()
            .map(|((name, comm), acc)| MpiFunctionSummary {
                name: name.clone(),
                comm: comm.clone(),
                calls: acc.calls,
                total_s: (acc.total_ns as f64) * 1e-9,
                min_s: (acc.min_ns as f64) * 1e-9,
                max_s: (acc.max_ns as f64) * 1e-9,
                bytes_sent: acc.bytes_sent,
                bytes_recv: acc.bytes_recv,
                size_hist: acc.size_hist.to_vec(),
            })
            .collect();

        RecordMpi {
            thread_level: self.thread_level.clone(),
            shim_overhead_ns_per_call: self.shim_overhead_ns_per_call,
            functions,
            peers: None,
            collectives: None,
            trace: None,
            clock_sync: None,
        }
    }

    /// Atomically writes `mpi_rank<k>.yaml` into `dir` (never panics, R-SAF-4).
    pub fn write_summary(&self, dir: &Path) -> std::io::Result<PathBuf> {
        fs::create_dir_all(dir)?;
        let out_path = dir.join(format!("mpi_rank{}.yaml", self.rank.max(0)));
        let tmp_path = out_path.with_extension("yaml.tmp");
        let rec = self.to_record_mpi();
        let yaml =
            calo_schema::yaml::to_string(&rec).map_err(|e| std::io::Error::other(e.to_string()))?;
        {
            let mut f = File::create(&tmp_path)?;
            f.write_all(yaml.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp_path, &out_path)?;
        Ok(out_path)
    }
}

fn size_bin(bytes: u64) -> usize {
    if bytes == 0 {
        return 0;
    }
    if bytes <= 64 {
        return 1;
    }
    let shifted = (bytes - 1) >> 6; // divide by 64
    let log2 = (64 - shifted.leading_zeros()) as usize;
    (1 + log2).min(HIST_BINS - 1)
}

fn thread_level_name(level: i32) -> &'static str {
    match level {
        0 => "MPI_THREAD_SINGLE",
        1 => "MPI_THREAD_FUNNELED",
        2 => "MPI_THREAD_SERIALIZED",
        3 => "MPI_THREAD_MULTIPLE",
        _ => "UNKNOWN",
    }
}

fn comm_label(comm_id: u64) -> String {
    if comm_id == 0 {
        "MPI_COMM_WORLD".to_string()
    } else {
        format!("comm_{comm_id:x}")
    }
}

/// Monotonic raw timestamp in nanoseconds (`CLOCK_MONOTONIC_RAW`, Spec §3 I6).
#[no_mangle]
pub extern "C" fn calo_mpi_now_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid writable `timespec`.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut ts) };
    if rc == 0 {
        (ts.tv_sec as i64)
            .saturating_mul(1_000_000_000)
            .saturating_add(ts.tv_nsec as i64)
    } else {
        0
    }
}

fn measure_overhead_ns() -> f64 {
    const ITERS: u64 = 256;
    let t0 = calo_mpi_now_ns();
    let mut acc = 0i64;
    for _ in 0..ITERS {
        acc = acc.wrapping_add(calo_mpi_now_ns());
    }
    std::hint::black_box(acc);
    let t1 = calo_mpi_now_ns();
    (t1.saturating_sub(t0).max(0) as f64) / (ITERS as f64)
}

/// Initializes the M1 PMPI shim on this rank (Spec §9.5 B20, B21).
#[no_mangle]
pub extern "C" fn calo_mpi_init(rank: i32, size: i32, thread_level: i32) {
    if SHIM_DISABLED.load(Ordering::Relaxed) {
        return;
    }
    let overhead = measure_overhead_ns();
    let out_dir = std::env::var_os("CALO_MPI_OUT").map(PathBuf::from);
    if let Ok(mut guard) = STATE.lock() {
        *guard = Some(MpiAggregator::new(
            rank,
            size,
            thread_level_name(thread_level),
            overhead,
            out_dir,
        ));
    } else {
        SHIM_DISABLED.store(true, Ordering::Relaxed);
    }
}

/// Records one MPI call from the C PMPI wrapper.
///
/// # Safety
/// `name` must be a valid NUL-terminated C string (or null, in which case the call is ignored).
#[no_mangle]
pub unsafe extern "C" fn calo_mpi_record_call(
    name: *const c_char,
    comm_id: u64,
    start_ns: i64,
    end_ns: i64,
    bytes_sent: u64,
    bytes_recv: u64,
    peer: i32,
) {
    if SHIM_DISABLED.load(Ordering::Relaxed) || name.is_null() {
        return;
    }
    // SAFETY: Caller guarantees `name` is a valid NUL-terminated C string.
    let Ok(fn_name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return;
    };
    let comm = comm_label(comm_id);
    if let Ok(mut guard) = STATE.lock() {
        if let Some(ref mut agg) = *guard {
            agg.record_call(
                fn_name, &comm, start_ns, end_ns, bytes_sent, bytes_recv, peer,
            );
        }
    } else {
        SHIM_DISABLED.store(true, Ordering::Relaxed);
    }
}

/// Records one collective call instance `(comm, seq)` from the C PMPI wrapper (Spec §9.5 B18).
///
/// # Safety
/// `name` must be a valid NUL-terminated C string (or null, in which case the call is ignored).
#[no_mangle]
pub unsafe extern "C" fn calo_mpi_record_collective(
    name: *const c_char,
    comm_id: u64,
    seq: u64,
    start_ns: i64,
    end_ns: i64,
    bytes: u64,
) {
    if SHIM_DISABLED.load(Ordering::Relaxed) || name.is_null() {
        return;
    }
    // SAFETY: Caller guarantees `name` is a valid NUL-terminated C string.
    let Ok(fn_name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return;
    };
    let comm = comm_label(comm_id);
    if let Ok(mut guard) = STATE.lock() {
        if let Some(ref mut agg) = *guard {
            agg.record_call(fn_name, &comm, start_ns, end_ns, bytes, bytes, -1);
            agg.record_collective(fn_name, &comm, seq, start_ns, end_ns, bytes);
        }
    } else {
        SHIM_DISABLED.store(true, Ordering::Relaxed);
    }
}

/// Flushes `mpi_rank<k>.yaml` at `MPI_Finalize`. Never fails the workload (Spec §16 R-SAF-4).
#[no_mangle]
pub extern "C" fn calo_mpi_finalize() {
    if SHIM_DISABLED.load(Ordering::Relaxed) {
        return;
    }
    let agg_opt = match STATE.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => {
            SHIM_DISABLED.store(true, Ordering::Relaxed);
            return;
        }
    };
    if let Some(agg) = agg_opt {
        if let Some(ref dir) = agg.out_dir {
            if agg.write_summary(dir).is_err() {
                SHIM_DISABLED.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn aggregates_calls_peers_collectives_and_size_histogram() {
        let mut agg = MpiAggregator::new(0, 4, "MPI_THREAD_MULTIPLE", 18.5, None);
        agg.record_call("MPI_Send", "MPI_COMM_WORLD", 1000, 2500, 1024, 0, 1);
        agg.record_call("MPI_Send", "MPI_COMM_WORLD", 3000, 3500, 2048, 0, 1);
        agg.record_collective("MPI_Allreduce", "MPI_COMM_WORLD", 0, 4000, 9000, 4096);

        let rec = agg.to_record_mpi();
        assert_eq!(rec.thread_level, "MPI_THREAD_MULTIPLE");
        assert_eq!(rec.functions.len(), 1); // record_collective didn't call record_call directly
        let send = &rec.functions[0];
        assert_eq!(send.name, "MPI_Send");
        assert_eq!(send.calls, 2);
        assert_eq!(send.bytes_sent, 3072);
        assert!((send.total_s - 2.0e-6).abs() < 1e-12);
        assert_eq!(agg.peers()[&1].msgs, 2);
        assert_eq!(agg.collectives().len(), 1);
    }

    #[test]
    fn thread_multiple_concurrent_recording_and_yaml_round_trip() {
        let dir = std::env::temp_dir().join(format!("calo_mpi_test_{}", std::process::id()));
        let agg = Arc::new(Mutex::new(MpiAggregator::new(
            2,
            4,
            "MPI_THREAD_MULTIPLE",
            12.0,
            Some(dir.clone()),
        )));

        let mut handles = Vec::new();
        for t in 0..4 {
            let a = Arc::clone(&agg);
            handles.push(thread::spawn(move || {
                for i in 0..250 {
                    let mut g = a.lock().unwrap();
                    g.record_call("MPI_Isend", "MPI_COMM_WORLD", i * 10, i * 10 + 5, 512, 0, t);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let guard = agg.lock().unwrap();
        let path = guard.write_summary(&dir).expect("write mpi_rank2.yaml");
        let raw = fs::read_to_string(&path).expect("read yaml");
        let decoded: RecordMpi = calo_schema::yaml::from_str(&raw).expect("decode RecordMpi");
        assert_eq!(decoded.functions[0].calls, 1000);
        assert_eq!(decoded.functions[0].bytes_sent, 512_000);
        let _ = fs::remove_dir_all(&dir);
    }
}
