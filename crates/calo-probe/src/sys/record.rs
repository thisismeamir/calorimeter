//! Decoding of perf ring-buffer records (spec §9.3 B7, B10).
//! Host-endian, as the kernel writes them.

pub const PERF_RECORD_EXIT: u32 = 4;
pub const PERF_RECORD_LOST: u32 = 2;
pub const PERF_RECORD_COMM: u32 = 3;
pub const PERF_RECORD_FORK: u32 = 7;
pub const PERF_RECORD_SAMPLE: u32 = 9;
pub const PERF_RECORD_MMAP2: u32 = 10;

pub const SAMPLE_IP: u64 = 1 << 0;
pub const SAMPLE_TID: u64 = 1 << 1;
pub const SAMPLE_TIME: u64 = 1 << 2;
pub const SAMPLE_CALLCHAIN: u64 = 1 << 5;
pub const SAMPLE_CPU: u64 = 1 << 7;

/// Fixed sample_type used by probe. Layout below depends on it.
pub const SAMPLE_TYPE: u64 = SAMPLE_IP | SAMPLE_TID | SAMPLE_TIME | SAMPLE_CPU | SAMPLE_CALLCHAIN;
/// Trailing sample_id on non-sample records: TID(8) + TIME(8) + CPU(8).
const SAMPLE_ID_LEN: usize = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub ip: u64,
    pub pid: u32,
    pub tid: u32,
    pub time_ns: u64,
    pub cpu: u32,
    /// Raw callchain including context markers (>= 0xffff_ffff_ffff_f001).
    /// Filtering is the control plane's job.
    pub callchain: Vec<u64>,
}


pub const SAMPLE_REGS_USER: u64 = 1 << 18;
pub const SAMPLE_STACK_USER: u64 = 1 << 19;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DwarfStack {
    pub abi: u64,
    pub regs: Vec<u64>,
    pub stack: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mmap2 {
    pub pid: u32,
    pub tid: u32,
    pub addr: u64,
    pub len: u64,
    pub pgoff: u64,
    pub prot: u32,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Sample(Sample),
    Mmap2(Mmap2),
    Comm { pid: u32, tid: u32, comm: String },
    Task { pid: u32, tid: u32, time_ns: u64, exit: bool },
    Lost { count: u64 },
    Other(u32),
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DecodeError {
    #[error("record truncated: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },
}

fn u16n(b: &[u8], o: usize) -> Result<u16, DecodeError> {
    b.get(o..o + 2)
        .map(|s| u16::from_ne_bytes([s[0], s[1]]))
        .ok_or(DecodeError::Truncated { need: o + 2, have: b.len() })
}
fn u32n(b: &[u8], o: usize) -> Result<u32, DecodeError> {
    b.get(o..o + 4)
        .map(|s| u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(DecodeError::Truncated { need: o + 4, have: b.len() })
}
fn u64n(b: &[u8], o: usize) -> Result<u64, DecodeError> {
    b.get(o..o + 8)
        .map(|s| u64::from_ne_bytes(s.try_into().unwrap_or([0; 8])))
        .ok_or(DecodeError::Truncated { need: o + 8, have: b.len() })
}
fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Decode one complete record (8-byte header included).
pub fn decode(rec: &[u8]) -> Result<Event, DecodeError> {
    let ty = u32n(rec, 0)?;
    let size = u16n(rec, 6)? as usize;
    if rec.len() < size {
        return Err(DecodeError::Truncated { need: size, have: rec.len() });
    }
    let rec = &rec[..size];
    Ok(match ty {
        PERF_RECORD_SAMPLE => {
            let nr = u64n(rec, 40)? as usize;
            let need = 48 + nr * 8;
            if rec.len() < need {
                return Err(DecodeError::Truncated { need, have: rec.len() });
            }
            let mut callchain = Vec::with_capacity(nr);
            for i in 0..nr {
                callchain.push(u64n(rec, 48 + i * 8)?);
            }
            Event::Sample(Sample {
                ip: u64n(rec, 8)?,
                pid: u32n(rec, 16)?,
                tid: u32n(rec, 20)?,
                time_ns: u64n(rec, 24)?,
                cpu: u32n(rec, 32)?,
                callchain,
            })
        }
        PERF_RECORD_MMAP2 => {
            let end = rec.len().saturating_sub(SAMPLE_ID_LEN).max(72);
            Event::Mmap2(Mmap2 {
                pid: u32n(rec, 8)?,
                tid: u32n(rec, 12)?,
                addr: u64n(rec, 16)?,
                len: u64n(rec, 24)?,
                pgoff: u64n(rec, 32)?,
                prot: u32n(rec, 64)?,
                path: cstr(rec.get(72..end).unwrap_or(&[])),
            })
        }
        PERF_RECORD_COMM => {
            let end = rec.len().saturating_sub(SAMPLE_ID_LEN).max(16);
            Event::Comm {
                pid: u32n(rec, 8)?,
                tid: u32n(rec, 12)?,
                comm: cstr(rec.get(16..end).unwrap_or(&[])),
            }
        }
        PERF_RECORD_FORK | PERF_RECORD_EXIT => Event::Task {
            pid: u32n(rec, 8)?,
            tid: u32n(rec, 16)?,
            time_ns: u64n(rec, 24)?,
            exit: ty == PERF_RECORD_EXIT,
        },
        PERF_RECORD_LOST => Event::Lost { count: u64n(rec, 16)? },
        other => Event::Other(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(ty: u32, size: usize) -> Vec<u8> {
        let mut v = ty.to_ne_bytes().to_vec();
        v.extend_from_slice(&0u16.to_ne_bytes());
        v.extend_from_slice(&(size as u16).to_ne_bytes());
        v
    }

    #[test]
    fn decodes_sample_with_callchain() {
        let nr = 3u64;
        let size = 48 + nr as usize * 8;
        let mut r = header(PERF_RECORD_SAMPLE, size);
        r.extend_from_slice(&0xdead_u64.to_ne_bytes()); // ip
        r.extend_from_slice(&10u32.to_ne_bytes()); // pid
        r.extend_from_slice(&11u32.to_ne_bytes()); // tid
        r.extend_from_slice(&123u64.to_ne_bytes()); // time
        r.extend_from_slice(&2u32.to_ne_bytes()); // cpu
        r.extend_from_slice(&0u32.to_ne_bytes()); // res
        r.extend_from_slice(&nr.to_ne_bytes());
        for ip in [1u64, 2, 3] {
            r.extend_from_slice(&ip.to_ne_bytes());
        }
        match decode(&r).unwrap() {
            Event::Sample(s) => {
                assert_eq!((s.ip, s.pid, s.tid, s.time_ns, s.cpu), (0xdead, 10, 11, 123, 2));
                assert_eq!(s.callchain, vec![1, 2, 3]);
            }
            e => panic!("unexpected {e:?}"),
        }
    }

    #[test]
    fn rejects_truncated_callchain() {
        let mut r = header(PERF_RECORD_SAMPLE, 48);
        r.extend_from_slice(&[0u8; 32]);
        r.extend_from_slice(&5u64.to_ne_bytes()); // nr=5 but no room
        assert!(matches!(decode(&r), Err(DecodeError::Truncated { .. })));
    }

    #[test]
    fn decodes_lost() {
        let mut r = header(PERF_RECORD_LOST, 24);
        r.extend_from_slice(&0u64.to_ne_bytes());
        r.extend_from_slice(&42u64.to_ne_bytes());
        assert_eq!(decode(&r).unwrap(), Event::Lost { count: 42 });
    }
}
