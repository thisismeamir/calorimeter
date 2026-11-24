//! Instrumentation channel FIFO, spec §9.4 line parser, phase/region state machines, and
//! `events | aggregate` storage (Spec §9.4 B13).

#![forbid(unsafe_code)]

use std::{
    collections::{btree_map::Entry, BTreeMap},
    fs, io,
    os::fd::{AsRawFd, OwnedFd, RawFd},
    path::{Path, PathBuf},
};

use calo_schema::common::{Warning, WarningCode};

use crate::sys::channel::{open_channel_fifo, read_nonblocking};

/// Maximum accepted line length (bytes, excluding trailing `\n`).
pub const MAX_CHANNEL_LINE_BYTES: usize = 4096;
/// Maximum number of protocol warnings stored in memory per run.
pub const MAX_STORED_WARNINGS: usize = 100;

/// Region collection mode (`events` keeps nested trees; `aggregate` keeps bounded stats).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegionMode {
    #[default]
    Events,
    Aggregate,
}

/// Parsed channel protocol event (Spec §9.4).
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelEvent {
    Phase {
        name: String,
        start: bool,
        t_ns: i64,
    },
    Region {
        name: String,
        start: bool,
        tid: u64,
        t_ns: i64,
    },
    Metric {
        key: String,
        value: f64,
        unit: Option<String>,
        t_ns: i64,
    },
    Tag {
        key: String,
        value: String,
        t_ns: i64,
    },
}

/// Channel protocol statistics exported to `record.tags` as `channel.*` entries.
///
/// Note: region durations carry the read-batching error (bounded by `epoll` wake-up latency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelStats {
    pub lines: u64,
    pub bytes: u64,
    pub partial_joins: u64,
    pub too_long: u64,
    pub bad_utf8: u64,
    pub unknown_verbs: u64,
    pub protocol_errors: u64,
    pub events_dropped_cap: u64,
    pub read_batch_ns: i64,
    pub read_batch_size: usize,
}

/// Pure, epoll-independent channel line parser (Spec §9.4).
pub struct ChannelReader {
    partial: Vec<u8>,
    dropping_oversize: bool,
    stats: ChannelStats,
}

impl Default for ChannelReader {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelReader {
    pub fn new() -> Self {
        Self {
            partial: Vec::with_capacity(MAX_CHANNEL_LINE_BYTES),
            dropping_oversize: false,
            stats: ChannelStats::default(),
        }
    }

    pub fn stats(&self) -> ChannelStats {
        self.stats
    }

    /// Feeds raw bytes read at `now_ns` into the line buffer, emitting parsed [`ChannelEvent`]s.
    ///
    /// Handles partial lines across calls (`partial_joins`), drops lines > 4096 bytes
    /// (`too_long`), decodes invalid UTF-8 lossily (`bad_utf8`), and ignores unknown verbs.
    pub fn feed(&mut self, bytes: &[u8], now_ns: i64, out: &mut Vec<ChannelEvent>) {
        self.stats.bytes = self.stats.bytes.saturating_add(bytes.len() as u64);
        self.stats.read_batch_ns = now_ns;
        let mut had_carryover = !self.partial.is_empty();
        let mut batch_events = 0usize;

        for &b in bytes {
            if self.dropping_oversize {
                if b == b'\n' {
                    self.dropping_oversize = false;
                }
                continue;
            }
            if b == b'\n' {
                let line_bytes = std::mem::take(&mut self.partial);
                if had_carryover {
                    self.stats.partial_joins = self.stats.partial_joins.saturating_add(1);
                    had_carryover = false;
                }
                if let Some(ev) = self.parse_line(&line_bytes, now_ns) {
                    out.push(ev);
                    batch_events += 1;
                }
            } else if self.partial.len() >= MAX_CHANNEL_LINE_BYTES {
                self.partial.clear();
                had_carryover = false;
                self.dropping_oversize = true;
                self.stats.too_long = self.stats.too_long.saturating_add(1);
            } else {
                self.partial.push(b);
            }
        }
        self.stats.read_batch_size = batch_events;
    }

    fn parse_line(&mut self, raw: &[u8], now_ns: i64) -> Option<ChannelEvent> {
        let trimmed = raw.strip_suffix(b"\r").unwrap_or(raw);
        if trimmed.is_empty() {
            return None;
        }
        self.stats.lines = self.stats.lines.saturating_add(1);

        let text = match std::str::from_utf8(trimmed) {
            Ok(s) => std::borrow::Cow::Borrowed(s),
            Err(_) => {
                self.stats.bad_utf8 = self.stats.bad_utf8.saturating_add(1);
                String::from_utf8_lossy(trimmed)
            }
        };
        let line = text.trim_start();
        if line.is_empty() {
            return None;
        }

        // Handle `tag <key>=<rest of line>` first so spaces in the value are preserved verbatim.
        if let Some(rest) = line.strip_prefix("tag ") {
            let rest = rest.trim_start();
            let Some((key, val)) = rest.split_once('=') else {
                self.stats.protocol_errors += 1;
                return None;
            };
            if key.is_empty() || key.contains(char::is_whitespace) {
                self.stats.protocol_errors += 1;
                return None;
            }
            return Some(ChannelEvent::Tag {
                key: key.to_string(),
                value: val.to_string(),
                t_ns: now_ns,
            });
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }

        // Optional leading `tid=<n>` emitted by `calo-channel` and `calo_channel.h`.
        let (leading_tid, verb_idx) = if let Some(tid_s) = tokens[0].strip_prefix("tid=") {
            let Ok(tid) = tid_s.parse::<u64>() else {
                self.stats.protocol_errors += 1;
                return None;
            };
            (Some(tid), 1usize)
        } else {
            (None, 0usize)
        };

        let Some(&verb) = tokens.get(verb_idx) else {
            self.stats.protocol_errors += 1;
            return None;
        };
        let args = &tokens[verb_idx + 1..];

        match verb {
            "phase" => {
                if args.len() != 2 || leading_tid.is_some() {
                    self.stats.protocol_errors += 1;
                    return None;
                }
                let start = match args[1] {
                    "start" => true,
                    "end" => false,
                    _ => {
                        self.stats.protocol_errors += 1;
                        return None;
                    }
                };
                Some(ChannelEvent::Phase {
                    name: args[0].to_string(),
                    start,
                    t_ns: now_ns,
                })
            }
            "region" => {
                // Accepted forms:
                // - `tid=<n> region <name> start|end` (calo-channel / C header)
                // - `region tid=<n> <name> start|end`
                // - `region <name> start|end` (tid defaults to 0)
                let (tid, name, kind) = if let Some(t) = leading_tid {
                    if args.len() != 2 {
                        self.stats.protocol_errors += 1;
                        return None;
                    }
                    (t, args[0], args[1])
                } else if args.len() == 3 {
                    let Some(tid_s) = args[0].strip_prefix("tid=") else {
                        self.stats.protocol_errors += 1;
                        return None;
                    };
                    let Ok(t) = tid_s.parse::<u64>() else {
                        self.stats.protocol_errors += 1;
                        return None;
                    };
                    (t, args[1], args[2])
                } else if args.len() == 2 {
                    (0u64, args[0], args[1])
                } else {
                    self.stats.protocol_errors += 1;
                    return None;
                };

                let start = match kind {
                    "start" => true,
                    "end" => false,
                    _ => {
                        self.stats.protocol_errors += 1;
                        return None;
                    }
                };
                Some(ChannelEvent::Region {
                    name: name.to_string(),
                    start,
                    tid,
                    t_ns: now_ns,
                })
            }
            "metric" => {
                if leading_tid.is_some() || args.is_empty() || args.len() > 2 {
                    self.stats.protocol_errors += 1;
                    return None;
                }
                let Some((key, val_s)) = args[0].split_once('=') else {
                    self.stats.protocol_errors += 1;
                    return None;
                };
                if key.is_empty() {
                    self.stats.protocol_errors += 1;
                    return None;
                }
                let Ok(value) = val_s.parse::<f64>() else {
                    self.stats.protocol_errors += 1;
                    return None;
                };
                let unit = args.get(1).map(|u| (*u).to_string());
                Some(ChannelEvent::Metric {
                    key: key.to_string(),
                    value,
                    unit,
                    t_ns: now_ns,
                })
            }
            "tag" => {
                // Reached only if `tag` appeared with no space/args or after `tid=`.
                self.stats.protocol_errors += 1;
                None
            }
            _ => {
                self.stats.unknown_verbs = self.stats.unknown_verbs.saturating_add(1);
                None
            }
        }
    }
}

/// Parent-side FIFO owner (`<outputs_dir>/channel.fifo`).
///
/// Workloads receive `CALO_CHANNEL=<path>` in their environment and must open the FIFO
/// with `O_WRONLY` and write whole lines of `<= 4096` bytes in a single `write(2)` each
/// (atomic below `PIPE_BUF`).
pub struct ChannelFifo {
    path: PathBuf,
    read_fd: OwnedFd,
    dummy_write_fd: Option<OwnedFd>,
    reader: ChannelReader,
}

impl ChannelFifo {
    /// Creates `<outputs_dir>/channel.fifo` (mode 0600) and opens non-blocking read + dummy write fds.
    pub fn create(outputs_dir: &Path) -> io::Result<Self> {
        Self::create_named(outputs_dir, "channel.fifo")
    }

    /// Creates `<outputs_dir>/<filename>` (mode 0600) so concurrent ranks do not collide.
    pub fn create_named(outputs_dir: &Path, filename: &str) -> io::Result<Self> {
        fs::create_dir_all(outputs_dir)?;
        let path = outputs_dir.join(filename);
        let (read_fd, write_fd) = open_channel_fifo(&path)?;
        Ok(Self {
            path,
            read_fd,
            dummy_write_fd: Some(write_fd),
            reader: ChannelReader::new(),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_fd(&self) -> RawFd {
        self.read_fd.as_raw_fd()
    }

    pub fn stats(&self) -> ChannelStats {
        self.reader.stats()
    }

    /// Drains all currently readable bytes from the FIFO at `now_ns` into `out`.
    pub fn drain_ready(&mut self, now_ns: i64, out: &mut Vec<ChannelEvent>) -> io::Result<usize> {
        let mut buf = [0u8; 16 * 1024];
        let mut total = 0usize;
        loop {
            let n = read_nonblocking(self.read_fd.as_raw_fd(), &mut buf)?;
            if n == 0 {
                break;
            }
            total += n;
            self.reader.feed(&buf[..n], now_ns, out);
        }
        Ok(total)
    }

    /// Closes the dummy write end and performs a final drain after the child exits.
    pub fn close_and_drain(&mut self, now_ns: i64, out: &mut Vec<ChannelEvent>) -> io::Result<()> {
        self.dummy_write_fd.take();
        let _ = self.drain_ready(now_ns, out)?;
        Ok(())
    }
}

impl Drop for ChannelFifo {
    fn drop(&mut self) {
        self.dummy_write_fd.take();
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseRecord {
    pub start_ns: i64,
    pub end_ns: i64,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionNode {
    pub calls: u64,
    pub incl_s: f64,
    pub excl_s: f64,
    pub children: BTreeMap<String, RegionNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionAggStats {
    pub count: u64,
    pub sum_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReportedMetric {
    pub value: f64,
    pub unit: Option<String>,
}

#[derive(Debug, Clone)]
struct OpenFrame {
    name: String,
    start_ns: i64,
    children_incl_ns: i64,
    children: BTreeMap<String, RegionNode>,
}

/// Accumulates `ChannelEvent`s into phases, per-thread region trees (or bounded aggregates),
/// reported metrics, and tags.
pub struct ChannelAccumulator {
    mode: RegionMode,
    open_phases: BTreeMap<String, i64>,
    pub phases: BTreeMap<String, PhaseRecord>,
    stacks: BTreeMap<u64, Vec<OpenFrame>>,
    pub region_trees: BTreeMap<u64, BTreeMap<String, RegionNode>>,
    pub region_agg: BTreeMap<(u64, String), RegionAggStats>,
    pub reported: BTreeMap<String, ReportedMetric>,
    pub tags: BTreeMap<String, String>,
    pub protocol_errors: u64,
    pub events_dropped_cap: u64,
    pub warnings: Vec<Warning>,
}

impl ChannelAccumulator {
    pub fn new(mode: RegionMode) -> Self {
        Self {
            mode,
            open_phases: BTreeMap::new(),
            phases: BTreeMap::new(),
            stacks: BTreeMap::new(),
            region_trees: BTreeMap::new(),
            region_agg: BTreeMap::new(),
            reported: BTreeMap::new(),
            tags: BTreeMap::new(),
            protocol_errors: 0,
            events_dropped_cap: 0,
            warnings: Vec::new(),
        }
    }

    pub fn apply(&mut self, ev: ChannelEvent) {
        match ev {
            ChannelEvent::Phase { name, start, t_ns } => {
                if start {
                    match self.open_phases.entry(name) {
                        Entry::Occupied(occ) => {
                            let msg =
                                format!("repeated phase '{} start' while already open", occ.key());
                            self.record_protocol_error(msg);
                        }
                        Entry::Vacant(vac) => {
                            vac.insert(t_ns);
                        }
                    }
                } else if let Some(s) = self.open_phases.remove(&name) {
                    self.phases
                        .entry(name)
                        .and_modify(|rec| {
                            rec.end_ns = t_ns;
                            rec.count += 1;
                        })
                        .or_insert(PhaseRecord {
                            start_ns: s,
                            end_ns: t_ns,
                            count: 1,
                        });
                } else {
                    self.record_protocol_error(format!("phase '{name} end' without start"));
                }
            }
            ChannelEvent::Region {
                name,
                start,
                tid,
                t_ns,
            } => {
                if start {
                    self.stacks.entry(tid).or_default().push(OpenFrame {
                        name,
                        start_ns: t_ns,
                        children_incl_ns: 0,
                        children: BTreeMap::new(),
                    });
                } else {
                    self.close_region(tid, &name, t_ns);
                }
            }
            ChannelEvent::Metric {
                key, value, unit, ..
            } => {
                self.reported.insert(key, ReportedMetric { value, unit });
            }
            ChannelEvent::Tag { key, value, .. } => {
                self.tags.insert(key, value);
            }
        }
    }

    fn close_region(&mut self, tid: u64, name: &str, end_ns: i64) {
        let Some((match_idx, stack_len)) = self.stacks.get(&tid).and_then(|stack| {
            stack
                .iter()
                .rposition(|f| f.name == name)
                .map(|idx| (idx, stack.len()))
        }) else {
            let msg = if self.stacks.get(&tid).is_none_or(Vec::is_empty) {
                format!("region '{name} end' on tid {tid} with empty stack")
            } else {
                format!("region '{name} end' on tid {tid} without start")
            };
            self.record_protocol_error(msg);
            return;
        };

        let to_auto_close = stack_len.saturating_sub(1).saturating_sub(match_idx);
        for _ in 0..to_auto_close {
            if let Some(unclosed) = self.stacks.get_mut(&tid).and_then(Vec::pop) {
                self.record_protocol_error(format!(
                    "mismatched region '{name} end' on tid {tid}; auto-closing '{}'",
                    unclosed.name
                ));
                self.finish_frame(tid, unclosed, end_ns);
            }
        }
        if let Some(frame) = self.stacks.get_mut(&tid).and_then(Vec::pop) {
            self.finish_frame(tid, frame, end_ns);
        }
    }

    fn finish_frame(&mut self, tid: u64, frame: OpenFrame, end_ns: i64) {
        let incl_ns = end_ns.saturating_sub(frame.start_ns).max(0);
        let raw_excl_ns = incl_ns.saturating_sub(frame.children_incl_ns);
        let excl_ns = if raw_excl_ns < 0 {
            self.protocol_errors += 1;
            0
        } else {
            raw_excl_ns
        };

        let stack = self.stacks.entry(tid).or_default();
        if let Some(parent) = stack.last_mut() {
            parent.children_incl_ns = parent.children_incl_ns.saturating_add(incl_ns);
        }

        match self.mode {
            RegionMode::Aggregate => {
                let mut path = String::new();
                for ancestor in stack.iter() {
                    path.push_str(&ancestor.name);
                    path.push('/');
                }
                path.push_str(&frame.name);

                let dur = incl_ns as u64;
                self.region_agg
                    .entry((tid, path))
                    .and_modify(|st| {
                        st.count += 1;
                        st.sum_ns = st.sum_ns.saturating_add(dur);
                        st.min_ns = st.min_ns.min(dur);
                        st.max_ns = st.max_ns.max(dur);
                    })
                    .or_insert(RegionAggStats {
                        count: 1,
                        sum_ns: dur,
                        min_ns: dur,
                        max_ns: dur,
                    });
            }
            RegionMode::Events => {
                let incl_s = (incl_ns as f64) * 1e-9;
                let excl_s = (excl_ns as f64) * 1e-9;
                let target_map = if let Some(parent) = stack.last_mut() {
                    &mut parent.children
                } else {
                    self.region_trees.entry(tid).or_default()
                };
                merge_region_node(
                    target_map,
                    frame.name,
                    RegionNode {
                        calls: 1,
                        incl_s,
                        excl_s,
                        children: frame.children,
                    },
                );
            }
        }
    }

    /// Closes any unterminated phases and regions at child exit `exit_ns`.
    pub fn finalize_at_exit(&mut self, exit_ns: i64) {
        let open_phases: Vec<(String, i64)> =
            std::mem::take(&mut self.open_phases).into_iter().collect();
        for (name, start_ns) in open_phases {
            self.record_protocol_error(format!("unterminated phase '{name}' closed at exit"));
            self.phases
                .entry(name)
                .and_modify(|rec| {
                    rec.end_ns = exit_ns;
                    rec.count += 1;
                })
                .or_insert(PhaseRecord {
                    start_ns,
                    end_ns: exit_ns,
                    count: 1,
                });
        }

        let tids: Vec<u64> = self.stacks.keys().copied().collect();
        for tid in tids {
            while let Some(frame) = self.stacks.get_mut(&tid).and_then(Vec::pop) {
                self.record_protocol_error(format!(
                    "unterminated region '{}' on tid {tid} closed at exit",
                    frame.name
                ));
                self.finish_frame(tid, frame, exit_ns);
            }
        }
    }

    /// Combines reader-level and accumulator-level statistics.
    pub fn combined_stats(&self, reader_stats: ChannelStats) -> ChannelStats {
        ChannelStats {
            protocol_errors: reader_stats
                .protocol_errors
                .saturating_add(self.protocol_errors),
            events_dropped_cap: reader_stats
                .events_dropped_cap
                .saturating_add(self.events_dropped_cap),
            ..reader_stats
        }
    }

    fn record_protocol_error(&mut self, msg: String) {
        self.protocol_errors += 1;
        if self.warnings.len() < MAX_STORED_WARNINGS {
            self.warnings.push(Warning {
                code: WarningCode::ChannelProtocolError,
                message: msg,
            });
        } else {
            self.events_dropped_cap += 1;
        }
    }
}

fn merge_region_node(target: &mut BTreeMap<String, RegionNode>, name: String, node: RegionNode) {
    if let Some(existing) = target.get_mut(&name) {
        existing.calls += node.calls;
        existing.incl_s += node.incl_s;
        existing.excl_s += node.excl_s;
        for (child_name, child_node) in node.children {
            merge_region_node(&mut existing.children, child_name, child_node);
        }
    } else {
        target.insert(name, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_spec_9_4_forms_and_tracks_partial_joins() {
        let mut reader = ChannelReader::new();
        let mut out = Vec::new();

        reader.feed(b"phase in", 10, &mut out);
        assert!(out.is_empty());
        reader.feed(
            b"it start\ntid=42 region step start\nregion tid=7 sub start\nregion plain end\nmetric flops=1.5e9 FLOP/s\ntag msg=hello world\n",
            20,
            &mut out,
        );
        assert_eq!(reader.stats().partial_joins, 1);
        assert_eq!(out.len(), 6);
        assert_eq!(
            out[0],
            ChannelEvent::Phase {
                name: "init".into(),
                start: true,
                t_ns: 20,
            }
        );
        assert_eq!(
            out[1],
            ChannelEvent::Region {
                name: "step".into(),
                start: true,
                tid: 42,
                t_ns: 20,
            }
        );
        assert_eq!(
            out[2],
            ChannelEvent::Region {
                name: "sub".into(),
                start: true,
                tid: 7,
                t_ns: 20,
            }
        );
        assert_eq!(
            out[3],
            ChannelEvent::Region {
                name: "plain".into(),
                start: false,
                tid: 0,
                t_ns: 20,
            }
        );
        assert_eq!(
            out[4],
            ChannelEvent::Metric {
                key: "flops".into(),
                value: 1.5e9,
                unit: Some("FLOP/s".into()),
                t_ns: 20,
            }
        );
        assert_eq!(
            out[5],
            ChannelEvent::Tag {
                key: "msg".into(),
                value: "hello world".into(),
                t_ns: 20,
            }
        );
    }

    #[test]
    fn repeated_open_phase_is_protocol_error_and_repeated_complete_keeps_bounds() {
        let mut acc = ChannelAccumulator::new(RegionMode::Events);
        acc.apply(ChannelEvent::Phase {
            name: "p".into(),
            start: true,
            t_ns: 100,
        });
        acc.apply(ChannelEvent::Phase {
            name: "p".into(),
            start: true,
            t_ns: 150,
        });
        assert_eq!(acc.protocol_errors, 1);
        acc.apply(ChannelEvent::Phase {
            name: "p".into(),
            start: false,
            t_ns: 200,
        });
        acc.apply(ChannelEvent::Phase {
            name: "p".into(),
            start: true,
            t_ns: 300,
        });
        acc.apply(ChannelEvent::Phase {
            name: "p".into(),
            start: false,
            t_ns: 500,
        });
        assert_eq!(
            acc.phases["p"],
            PhaseRecord {
                start_ns: 100,
                end_ns: 500,
                count: 2,
            }
        );
    }

    #[test]
    fn warnings_capped_at_hundred_and_counted_in_events_dropped_cap() {
        let mut acc = ChannelAccumulator::new(RegionMode::Events);
        for _ in 0..250 {
            acc.apply(ChannelEvent::Phase {
                name: "x".into(),
                start: false,
                t_ns: 1,
            });
        }
        assert_eq!(acc.warnings.len(), MAX_STORED_WARNINGS);
        assert_eq!(acc.protocol_errors, 250);
        assert_eq!(acc.events_dropped_cap, 150);
    }

    #[test]
    fn aggregate_mode_bounded_len_and_finalize_at_exit_both_modes() {
        let mut acc = ChannelAccumulator::new(RegionMode::Aggregate);
        for i in 0..500_000i64 {
            acc.apply(ChannelEvent::Region {
                name: "step".into(),
                start: true,
                tid: 1,
                t_ns: i * 10,
            });
            acc.apply(ChannelEvent::Region {
                name: "step".into(),
                start: false,
                tid: 1,
                t_ns: i * 10 + 5,
            });
        }
        assert_eq!(acc.region_agg.len(), 1);
        for i in 500_000..1_000_000i64 {
            acc.apply(ChannelEvent::Region {
                name: "step".into(),
                start: true,
                tid: 1,
                t_ns: i * 10,
            });
            acc.apply(ChannelEvent::Region {
                name: "step".into(),
                start: false,
                tid: 1,
                t_ns: i * 10 + 5,
            });
        }
        assert_eq!(acc.region_agg.len(), 1);
        assert_eq!(acc.region_agg[&(1, "step".to_string())].count, 1_000_000);

        // Unterminated region closed by finalize_at_exit in Aggregate mode
        acc.apply(ChannelEvent::Region {
            name: "tail".into(),
            start: true,
            tid: 2,
            t_ns: 100,
        });
        acc.finalize_at_exit(250);
        assert_eq!(acc.region_agg[&(2, "tail".to_string())].sum_ns, 150);

        // Unterminated region + phase closed by finalize_at_exit in Events mode
        let mut ev_acc = ChannelAccumulator::new(RegionMode::Events);
        ev_acc.apply(ChannelEvent::Phase {
            name: "run".into(),
            start: true,
            t_ns: 10,
        });
        ev_acc.apply(ChannelEvent::Region {
            name: "r".into(),
            start: true,
            tid: 3,
            t_ns: 20,
        });
        ev_acc.finalize_at_exit(120);
        assert_eq!(ev_acc.phases["run"].end_ns, 120);
        assert_eq!(ev_acc.region_trees[&3]["r"].calls, 1);
    }
}
