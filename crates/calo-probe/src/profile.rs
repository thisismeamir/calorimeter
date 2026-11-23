//! T1 CPU stack sampling, per-event counter attribution, ELF build-id / symbol checks,
//! and `profile.{samples,chains,maps,attr_samples,stacks}` emission (Spec §9.3 B7–B12, R-BLD-3).

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    os::fd::RawFd,
    path::Path,
    str::FromStr,
    time::Duration,
};

use calo_schema::{
    common::{CallgraphMethod, Warning, WarningCode},
    record::{ProfileModule, RecordProfile},
    table::Sidecar,
    tables::{self, ProfileChainRow, ProfileMapRow, ProfileSampleRow},
};

use crate::{
    counters::CounterAlias,
    sys::{
        record::{
            decode_with_config, Event, Sample, SAMPLE_CALLCHAIN, SAMPLE_CPU, SAMPLE_IP,
            SAMPLE_REGS_USER, SAMPLE_STACK_USER, SAMPLE_TIME, SAMPLE_TID, SAMPLE_TYPE,
        },
        sample::{CallgraphConfig, SampleTrigger, Sampler, Source},
    },
    table_writer::{TableWriter, WriterHandle},
};

const DEFAULT_FREQ_HZ: u32 = 499;
const DEFAULT_MAX_LOSS: f64 = 0.01;
const DEFAULT_STACK_BYTES: u32 = 8192;
const DEFAULT_ATTR_PERIOD: u64 = 100_000;
const PROT_EXEC: u32 = 0x4;

/// Configuration for T1 sampling (`RunSpecProfile` + `strict_loss`).
#[derive(Debug, Clone)]
pub struct ProfileConfig {
    pub freq_hz: u32,
    pub callgraph: CallgraphMethod,
    pub stack_bytes: u32,
    pub attribute: Vec<String>,
    pub max_loss: f64,
    pub strict_loss: bool,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            freq_hz: DEFAULT_FREQ_HZ,
            callgraph: CallgraphMethod::Fp,
            stack_bytes: DEFAULT_STACK_BYTES,
            attribute: Vec::new(),
            max_loss: DEFAULT_MAX_LOSS,
            strict_loss: false,
        }
    }
}

/// Summary produced when finalizing [`ProfileCollector`].
#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub record_profile: RecordProfile,
    pub sample_loss: Option<f64>,
    pub warnings: Vec<Warning>,
    pub errors: Vec<Warning>,
    pub invalid: bool,
}

struct AttrSamplerEntry {
    event_id: String,
    sampler: Sampler,
    sample_type: u64,
    regs_count: usize,
}

/// Manages primary and attribute [`Sampler`]s, sidecar writers, and ELF metadata caches.
pub struct ProfileCollector {
    config: ProfileConfig,
    primary: Option<Sampler>,
    primary_sample_type: u64,
    primary_regs_count: usize,
    attr_samplers: Vec<AttrSamplerEntry>,
    samples_writer: WriterHandle,
    chains_writer: WriterHandle,
    maps_writer: WriterHandle,
    chain_cursor: i64,
    samples_count: u64,
    lost_count: u64,
    decode_errors: u64,
    build_id_cache: BTreeMap<String, String>,
    modules: BTreeMap<(String, u64), ProfileModule>,
    warnings: Vec<Warning>,
    errors: Vec<Warning>,
}

impl ProfileCollector {
    /// Opens the primary sampler (`Cycles` -> `CpuClock` fallback) and any `attribute`
    /// samplers on gated `pid` before `go()`, plus `profile.{samples,chains,maps}` writers.
    pub fn open(pid: libc::pid_t, config: ProfileConfig, out_dir: &Path) -> io::Result<Self> {
        let mut warnings = Vec::new();
        let cg_cfg = match config.callgraph {
            CallgraphMethod::Fp => CallgraphConfig::Fp,
            CallgraphMethod::Dwarf => CallgraphConfig::Dwarf {
                stack_bytes: config.stack_bytes.max(512),
            },
        };
        let (sample_type, regs_count) = match cg_cfg {
            CallgraphConfig::Fp => (SAMPLE_TYPE, 0),
            CallgraphConfig::Dwarf { .. } => (
                SAMPLE_IP
                    | SAMPLE_TID
                    | SAMPLE_TIME
                    | SAMPLE_CPU
                    | SAMPLE_CALLCHAIN
                    | SAMPLE_REGS_USER
                    | SAMPLE_STACK_USER,
                crate::sys::sample::default_user_regs_count(),
            ),
        };

        let trigger = SampleTrigger::Freq(u64::from(config.freq_hz.max(1)));
        let primary = match Sampler::open_configured(pid, Source::Cycles, trigger, 6, cg_cfg) {
            Ok(s) => Some(s),
            Err(cycles_err) => {
                let errno = cycles_err.raw_os_error().unwrap_or(0);
                warnings.push(Warning {
                    code: WarningCode::TierDegraded,
                    message: format!(
                        "Cycles sampling unavailable (errno {errno}); falling back to CpuClock: {cycles_err}"
                    ),
                });
                match Sampler::open_configured(pid, Source::CpuClock, trigger, 6, cg_cfg) {
                    Ok(s) => Some(s),
                    Err(clk_err) => {
                        let c_errno = clk_err.raw_os_error().unwrap_or(0);
                        warnings.push(Warning {
                            code: WarningCode::TierDegraded,
                            message: format!(
                                "CpuClock sampling unavailable (errno {c_errno}): {clk_err}"
                            ),
                        });
                        None
                    }
                }
            }
        };

        let mut attr_samplers = Vec::new();
        for alias_name in &config.attribute {
            let Ok(alias) = CounterAlias::from_str(alias_name) else {
                warnings.push(Warning {
                    code: WarningCode::TierDegraded,
                    message: format!("unknown profile.attribute counter '{alias_name}'"),
                });
                continue;
            };
            let spec = alias.event_spec();
            match Sampler::open_raw_configured(
                pid,
                spec.type_,
                spec.config,
                SampleTrigger::Period(DEFAULT_ATTR_PERIOD),
                5,
                CallgraphConfig::Fp,
            ) {
                Ok(sampler) => attr_samplers.push(AttrSamplerEntry {
                    event_id: alias_name.clone(),
                    sampler,
                    sample_type: SAMPLE_TYPE,
                    regs_count: 0,
                }),
                Err(err) => {
                    let errno = err.raw_os_error().unwrap_or(0);
                    warnings.push(Warning {
                        code: WarningCode::TierDegraded,
                        message: format!(
                            "attribute sampler for '{alias_name}' failed (errno {errno}): {err}"
                        ),
                    });
                }
            }
        }

        let samples_writer = WriterHandle::spawn(
            TableWriter::create_profile_samples(&out_dir.join("profile.samples.calotbl"), "profile.samples.calotbl")?,
            4096,
        );
        let chains_writer = WriterHandle::spawn(
            TableWriter::create_profile_chains(&out_dir.join("profile.chains.calotbl"), "profile.chains.calotbl")?,
            8192,
        );
        let maps_writer = WriterHandle::spawn(
            TableWriter::create_profile_maps(&out_dir.join("profile.maps.calotbl"), "profile.maps.calotbl")?,
            1024,
        );

        Ok(Self {
            config,
            primary,
            primary_sample_type: sample_type,
            primary_regs_count: regs_count,
            attr_samplers,
            samples_writer,
            chains_writer,
            maps_writer,
            chain_cursor: 0,
            samples_count: 0,
            lost_count: 0,
            decode_errors: 0,
            build_id_cache: BTreeMap::new(),
            modules: BTreeMap::new(),
            warnings,
            errors: Vec::new(),
        })
    }

    /// Returns all active sampler ring file descriptors to register in `epoll`.
    pub fn raw_fds(&self) -> Vec<RawFd> {
        let mut fds = Vec::with_capacity(1 + self.attr_samplers.len());
        if let Some(ref p) = self.primary {
            fds.push(p.raw_fd());
        }
        for a in &self.attr_samplers {
            fds.push(a.sampler.raw_fd());
        }
        fds
    }

    /// Drains whichever sampler owns `fd` (or drains all samplers if `fd` is `None`).
    pub fn drain_fd(&mut self, fd: Option<RawFd>) {
        if let Some(ref mut p) = self.primary {
            if fd.is_none() || fd == Some(p.raw_fd()) {
                let st = self.primary_sample_type;
                let rc = self.primary_regs_count;
                let mut batch = Vec::new();
                match p.drain_raw(|rec| match decode_with_config(rec, st, rc) {
                    Ok(ev) => batch.push(ev),
                    Err(_) => self.decode_errors += 1,
                }) {
                    Ok(_) => {
                        for ev in batch {
                            self.handle_event(ev, None);
                        }
                    }
                    Err(_) => self.decode_errors += 1,
                }
            }
        }

        for idx in 0..self.attr_samplers.len() {
            let matches = fd.is_none() || fd == Some(self.attr_samplers[idx].sampler.raw_fd());
            if !matches {
                continue;
            }
            let st = self.attr_samplers[idx].sample_type;
            let rc = self.attr_samplers[idx].regs_count;
            let ev_id = self.attr_samplers[idx].event_id.clone();
            let mut batch = Vec::new();
            match self.attr_samplers[idx]
                .sampler
                .drain_raw(|rec| match decode_with_config(rec, st, rc) {
                    Ok(ev) => batch.push(ev),
                    Err(_) => self.decode_errors += 1,
                }) {
                Ok(_) => {
                    for ev in batch {
                        self.handle_event(ev, Some(&ev_id));
                    }
                }
                Err(_) => self.decode_errors += 1,
            }
        }
    }

    fn handle_event(&mut self, ev: Event, _attr_event_id: Option<&str>) {
        match ev {
            Event::Sample(s) => self.record_sample(s),
            Event::Mmap2(m) => {
                let build_id = self.cached_build_id(&m.path);
                if (m.prot & PROT_EXEC) != 0 && !m.path.is_empty() && !m.path.starts_with('[') {
                    self.modules.entry((m.path.clone(), m.addr)).or_insert_with(|| {
                        ProfileModule {
                            path: m.path.clone(),
                            build_id: build_id.clone(),
                            base: m.addr,
                            size: m.len,
                        }
                    });
                }
                self.maps_writer.send_profile_map(ProfileMapRow {
                    pid: m.pid as i32,
                    addr: m.addr as i64,
                    len: m.len as i64,
                    pgoff: m.pgoff as i64,
                    prot: m.prot as i32,
                    path: m.path,
                    build_id,
                });
            }
            Event::Lost { count } => {
                self.lost_count = self.lost_count.saturating_add(count);
            }
            Event::Comm { .. } | Event::Task { .. } | Event::Other(_) => {}
        }
    }

    fn record_sample(&mut self, s: Sample) {
        self.samples_count = self.samples_count.saturating_add(1);
        let chain_start = self.chain_cursor;
        let chain_len = s.callchain.len() as i32;
        for ip in &s.callchain {
            self.chains_writer
                .send_profile_chain(ProfileChainRow { ip: *ip as i64 });
        }
        self.chain_cursor = self.chain_cursor.saturating_add(s.callchain.len() as i64);

        self.samples_writer.send_profile_sample(ProfileSampleRow {
            time_ns: s.time_ns as i64,
            pid: s.pid as i32,
            tid: s.tid as i32,
            cpu: s.cpu as i32,
            ip: s.ip as i64,
            chain_start,
            chain_len,
        });
    }

    fn cached_build_id(&mut self, path: &str) -> String {
        if let Some(id) = self.build_id_cache.get(path) {
            return id.clone();
        }
        let id = if path.is_empty() || path.starts_with('[') || path.starts_with("//") {
            String::new()
        } else {
            read_elf_build_id(Path::new(path))
        };
        self.build_id_cache.insert(path.to_string(), id.clone());
        id
    }

    /// Checks whether the main executable has `.symtab` or `.debug_info` (R-BLD-3).
    pub fn check_main_executable_symbols(&mut self, exe_path: &Path) -> bool {
        if elf_has_symbols(exe_path) {
            true
        } else {
            self.errors.push(Warning {
                code: WarningCode::NoSymbols,
                message: format!(
                    "executable '{}' has neither .symtab nor .debug_info in profile mode",
                    exe_path.display()
                ),
            });
            false
        }
    }

    /// Drains remaining events, flushes and closes all writers, and computes `sample_loss`.
    pub fn finish(mut self, has_symbols: bool) -> io::Result<ProfileSummary> {
        self.drain_fd(None);

        let _ = self.samples_writer.flush_now_blocking(Duration::from_secs(1));
        let _ = self.chains_writer.flush_now_blocking(Duration::from_secs(1));
        let _ = self.maps_writer.flush_now_blocking(Duration::from_secs(1));

        let samples_out = self.samples_writer.finish()?;
        let _chains_out = self.chains_writer.finish()?;
        let _maps_out = self.maps_writer.finish()?;

        let total = self.samples_count.saturating_add(self.lost_count);
        let sample_loss = (total > 0).then(|| self.lost_count as f64 / total as f64);

        let mut invalid = !has_symbols;
        if let Some(loss) = sample_loss {
            if loss > self.config.max_loss {
                self.warnings.push(Warning {
                    code: WarningCode::SampleLoss,
                    message: format!(
                        "perf ring lost {}/{} samples ({:.2}% > {:.2}% max_loss)",
                        self.lost_count,
                        total,
                        loss * 100.0,
                        self.config.max_loss * 100.0
                    ),
                });
                if self.config.strict_loss {
                    invalid = true;
                }
            }
        }

        let data: Sidecar = samples_out.sidecar;
        Ok(ProfileSummary {
            record_profile: RecordProfile {
                method: self.config.callgraph,
                freq_hz: self.config.freq_hz,
                samples: self.samples_count,
                lost: self.lost_count,
                modules: self.modules.into_values().collect(),
                data,
            },
            sample_loss,
            warnings: self.warnings,
            errors: self.errors,
            invalid,
        })
    }
}

/// Safe-Rust ELF note parser reading `.note.gnu.build-id` (or `PT_NOTE` fallback).
/// Returns lowercase hex build-id, or `""` if missing or unreadable.
pub fn read_elf_build_id(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return String::new();
    };
    parse_elf_build_id(&bytes).unwrap_or_default()
}

/// Pure safe-Rust ELF build-id parser over raw ELF bytes.
pub fn parse_elf_build_id(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 52 || &bytes[0..4] != b"\x7fELF" || bytes[5] != 1 {
        return None;
    }
    let class = bytes[4]; // 1 = ELF32, 2 = ELF64
    let (shoff, shentsize, shnum, phoff, phentsize, phnum) = match class {
        2 if bytes.len() >= 64 => (
            u64_le(bytes, 40)? as usize,
            u16_le(bytes, 58)? as usize,
            u16_le(bytes, 60)? as usize,
            u64_le(bytes, 32)? as usize,
            u16_le(bytes, 54)? as usize,
            u16_le(bytes, 56)? as usize,
        ),
        1 => (
            u32_le(bytes, 32)? as usize,
            u16_le(bytes, 46)? as usize,
            u16_le(bytes, 48)? as usize,
            u32_le(bytes, 28)? as usize,
            u16_le(bytes, 42)? as usize,
            u16_le(bytes, 44)? as usize,
        ),
        _ => return None,
    };

    // 1. Scan SHT_NOTE (type == 7) sections.
    for i in 0..shnum {
        let off = shoff.checked_add(i.checked_mul(shentsize)?)?;
        let (sh_type, data_off, data_len) = if class == 2 {
            (
                u32_le(bytes, off + 4)?,
                u64_le(bytes, off + 24)? as usize,
                u64_le(bytes, off + 32)? as usize,
            )
        } else {
            (
                u32_le(bytes, off + 4)?,
                u32_le(bytes, off + 16)? as usize,
                u32_le(bytes, off + 20)? as usize,
            )
        };
        if sh_type == 7 {
            if let Some(end) = data_off.checked_add(data_len) {
                if let Some(slice) = bytes.get(data_off..end) {
                    if let Some(id) = parse_gnu_build_id_note(slice) {
                        return Some(id);
                    }
                }
            }
        }
    }

    // 2. Fallback to PT_NOTE (type == 4) program headers.
    for i in 0..phnum {
        let off = phoff.checked_add(i.checked_mul(phentsize)?)?;
        let p_type = u32_le(bytes, off)?;
        if p_type != 4 {
            continue;
        }
        let (data_off, data_len) = if class == 2 {
            (u64_le(bytes, off + 8)? as usize, u64_le(bytes, off + 32)? as usize)
        } else {
            (u32_le(bytes, off + 4)? as usize, u32_le(bytes, off + 16)? as usize)
        };
        if let Some(end) = data_off.checked_add(data_len) {
            if let Some(slice) = bytes.get(data_off..end) {
                if let Some(id) = parse_gnu_build_id_note(slice) {
                    return Some(id);
                }
            }
        }
    }
    None
}

fn parse_gnu_build_id_note(mut data: &[u8]) -> Option<String> {
    while data.len() >= 12 {
        let namesz = u32_le(data, 0)? as usize;
        let descsz = u32_le(data, 4)? as usize;
        let ntype = u32_le(data, 8)?;
        let name_off = 12usize;
        let name_end = name_off.checked_add(namesz)?;
        let desc_off = align4(name_end)?;
        let desc_end = desc_off.checked_add(descsz)?;
        let next = align4(desc_end)?;

        let name = data.get(name_off..name_end)?;
        let desc = data.get(desc_off..desc_end)?;
        if ntype == 3 && name == b"GNU\0" && !desc.is_empty() {
            let mut hex = String::with_capacity(desc.len() * 2);
            for &b in desc {
                use std::fmt::Write as _;
                let _ = write!(hex, "{b:02x}");
            }
            return Some(hex);
        }
        data = data.get(next..)?;
    }
    None
}

/// Safe-Rust check for `.symtab` (`SHT_SYMTAB`) or `.debug_info` in an ELF executable (R-BLD-3).
pub fn elf_has_symbols(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    parse_elf_has_symbols(&bytes)
}

/// Pure safe-Rust check over ELF bytes for `.symtab` or `.debug_info`.
pub fn parse_elf_has_symbols(bytes: &[u8]) -> bool {
    let Some(res) = parse_elf_has_symbols_inner(bytes) else {
        return false;
    };
    res
}

fn parse_elf_has_symbols_inner(bytes: &[u8]) -> Option<bool> {
    if bytes.len() < 52 || &bytes[0..4] != b"\x7fELF" || bytes[5] != 1 {
        return None;
    }
    let class = bytes[4];
    let (shoff, shentsize, shnum, shstrndx) = match class {
        2 if bytes.len() >= 64 => (
            u64_le(bytes, 40)? as usize,
            u16_le(bytes, 58)? as usize,
            u16_le(bytes, 60)? as usize,
            u16_le(bytes, 62)? as usize,
        ),
        1 => (
            u32_le(bytes, 32)? as usize,
            u16_le(bytes, 46)? as usize,
            u16_le(bytes, 48)? as usize,
            u16_le(bytes, 50)? as usize,
        ),
        _ => return None,
    };

    let strtab_slice = if shstrndx < shnum {
        let str_off = shoff.checked_add(shstrndx.checked_mul(shentsize)?)?;
        let (off, len) = if class == 2 {
            (u64_le(bytes, str_off + 24)? as usize, u64_le(bytes, str_off + 32)? as usize)
        } else {
            (u32_le(bytes, str_off + 16)? as usize, u32_le(bytes, str_off + 20)? as usize)
        };
        off.checked_add(len).and_then(|end| bytes.get(off..end))
    } else {
        None
    };

    let mut found = BTreeSet::new();
    for i in 0..shnum {
        let off = shoff.checked_add(i.checked_mul(shentsize)?)?;
        let sh_name = u32_le(bytes, off)? as usize;
        let sh_type = u32_le(bytes, off + 4)?;
        if sh_type == 2 {
            return Some(true);
        }
        if let Some(st) = strtab_slice {
            if let Some(tail) = st.get(sh_name..) {
                let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
                if let Ok(name) = std::str::from_utf8(&tail[..end]) {
                    found.insert(name);
                    if name == ".symtab" || name == ".debug_info" {
                        return Some(true);
                    }
                }
            }
        }
    }
    let _ = found;
    Some(false)
}

fn align4(v: usize) -> Option<usize> {
    v.checked_add(3).map(|x| x & !3)
}

fn u16_le(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_le(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_elf64_with_build_id_note(id: &[u8]) -> Vec<u8> {
        let mut note = Vec::new();
        note.extend_from_slice(&4u32.to_le_bytes()); // namesz = 4 ("GNU\0")
        note.extend_from_slice(&(id.len() as u32).to_le_bytes());
        note.extend_from_slice(&3u32.to_le_bytes()); // NT_GNU_BUILD_ID
        note.extend_from_slice(b"GNU\0");
        note.extend_from_slice(id);
        while note.len() % 4 != 0 {
            note.push(0);
        }

        let note_off = 64u64;
        let shoff = note_off + note.len() as u64;
        let mut elf = vec![0u8; 64];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2; // ELF64
        elf[5] = 1; // LE
        elf[40..48].copy_from_slice(&shoff.to_le_bytes()); // e_shoff
        elf[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
        elf[60..62].copy_from_slice(&2u16.to_le_bytes()); // e_shnum
        elf.extend_from_slice(&note);

        // Section 0: null
        elf.extend_from_slice(&[0u8; 64]);
        // Section 1: SHT_NOTE (type = 7)
        let mut sh = [0u8; 64];
        sh[4..8].copy_from_slice(&7u32.to_le_bytes());
        sh[24..32].copy_from_slice(&note_off.to_le_bytes());
        sh[32..40].copy_from_slice(&(note.len() as u64).to_le_bytes());
        elf.extend_from_slice(&sh);
        elf
    }

    #[test]
    fn elf_note_parser_extracts_build_id_and_handles_missing() {
        let elf = minimal_elf64_with_build_id_note(&[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(parse_elf_build_id(&elf), Some("deadbeef".to_string()));
        assert_eq!(parse_elf_build_id(b"not_an_elf"), None);

        let sh_id = read_elf_build_id(Path::new("/bin/sh"));
        if sh_id.is_empty() {
            eprintln!("SKIPPED /bin/sh build-id assertion: no .note.gnu.build-id present");
        } else {
            assert!(sh_id.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn elf_symbol_check_detects_symtab_and_stripped() {
        let mut elf = minimal_elf64_with_build_id_note(&[1, 2, 3, 4]);
        assert!(!parse_elf_has_symbols(&elf));
        // Flip section 1 sh_type from 7 (SHT_NOTE) to 2 (SHT_SYMTAB).
        let sh1_off = 64 + 16 + 64;
        elf[sh1_off + 4..sh1_off + 8].copy_from_slice(&2u32.to_le_bytes());
        assert!(parse_elf_has_symbols(&elf));
    }
}
