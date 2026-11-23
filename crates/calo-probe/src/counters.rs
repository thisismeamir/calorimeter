//! Performance counter alias table, overrides, `CounterSet`, and multiplexing detection
//! (Spec §9.3 B1–B3, R-INV-4, R-REC-5).

#![forbid(unsafe_code)]

use std::{collections::BTreeMap, fmt, str::FromStr};

use calo_schema::{Warning, WarningCode};
use perf_event_open_sys::bindings::{
    PERF_COUNT_HW_BRANCH_INSTRUCTIONS, PERF_COUNT_HW_BRANCH_MISSES, PERF_COUNT_HW_CACHE_L1D,
    PERF_COUNT_HW_CACHE_LL, PERF_COUNT_HW_CACHE_MISSES, PERF_COUNT_HW_CACHE_OP_READ,
    PERF_COUNT_HW_CACHE_REFERENCES, PERF_COUNT_HW_CACHE_RESULT_MISS, PERF_COUNT_HW_CPU_CYCLES,
    PERF_COUNT_HW_INSTRUCTIONS, PERF_COUNT_HW_STALLED_CYCLES_BACKEND,
    PERF_COUNT_HW_STALLED_CYCLES_FRONTEND, PERF_COUNT_SW_CONTEXT_SWITCHES,
    PERF_COUNT_SW_PAGE_FAULTS, PERF_COUNT_SW_TASK_CLOCK, PERF_TYPE_HARDWARE, PERF_TYPE_HW_CACHE,
    PERF_TYPE_RAW, PERF_TYPE_SOFTWARE,
};

use crate::sys::perf::{Counter, RawCount};

/// Privilege scope for performance counter collection (RunSpec `counters_scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CounterScope {
    /// Try `exclude_kernel = false` first; on `EACCES`/`EPERM` retry with `exclude_kernel = true`.
    #[default]
    Auto,
    /// Open directly with `exclude_kernel = true` (user-space only).
    User,
    /// Open with `exclude_kernel = false`; never retry with kernel excluded, degrade if denied.
    All,
}

/// Built-in performance counter aliases supported by `calo-probe` (Spec §9.3 B1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CounterAlias {
    Cycles,
    Instructions,
    CacheMisses,
    CacheReferences,
    BranchMisses,
    Branches,
    L1dMisses,
    LlcMisses,
    StalledBackend,
    StalledFrontend,
    TaskClock,
    PageFaults,
    ContextSwitches,
}

impl CounterAlias {
    /// Returns all built-in aliases in canonical order.
    pub const fn all() -> &'static [CounterAlias] {
        &[
            CounterAlias::Cycles,
            CounterAlias::Instructions,
            CounterAlias::CacheMisses,
            CounterAlias::CacheReferences,
            CounterAlias::BranchMisses,
            CounterAlias::Branches,
            CounterAlias::L1dMisses,
            CounterAlias::LlcMisses,
            CounterAlias::StalledBackend,
            CounterAlias::StalledFrontend,
            CounterAlias::TaskClock,
            CounterAlias::PageFaults,
            CounterAlias::ContextSwitches,
        ]
    }

    /// Returns the canonical kebab-case alias name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cycles => "cycles",
            Self::Instructions => "instructions",
            Self::CacheMisses => "cache-misses",
            Self::CacheReferences => "cache-references",
            Self::BranchMisses => "branch-misses",
            Self::Branches => "branches",
            Self::L1dMisses => "l1d-misses",
            Self::LlcMisses => "llc-misses",
            Self::StalledBackend => "stalled-backend",
            Self::StalledFrontend => "stalled-frontend",
            Self::TaskClock => "task-clock",
            Self::PageFaults => "page-faults",
            Self::ContextSwitches => "context-switches",
        }
    }

    /// Returns the default `(perf_event_attr.type_, perf_event_attr.config)` pair for this alias.
    pub const fn event_spec(self) -> EventSpec {
        match self {
            Self::Cycles => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_CPU_CYCLES as u64,
            },
            Self::Instructions => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_INSTRUCTIONS as u64,
            },
            Self::CacheReferences => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_CACHE_REFERENCES as u64,
            },
            Self::CacheMisses => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_CACHE_MISSES as u64,
            },
            Self::Branches => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_BRANCH_INSTRUCTIONS as u64,
            },
            Self::BranchMisses => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_BRANCH_MISSES as u64,
            },
            Self::StalledFrontend => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_STALLED_CYCLES_FRONTEND as u64,
            },
            Self::StalledBackend => EventSpec {
                type_: PERF_TYPE_HARDWARE,
                config: PERF_COUNT_HW_STALLED_CYCLES_BACKEND as u64,
            },
            Self::L1dMisses => EventSpec {
                type_: PERF_TYPE_HW_CACHE,
                config: hw_cache_config(
                    PERF_COUNT_HW_CACHE_L1D,
                    PERF_COUNT_HW_CACHE_OP_READ,
                    PERF_COUNT_HW_CACHE_RESULT_MISS,
                ),
            },
            Self::LlcMisses => EventSpec {
                type_: PERF_TYPE_HW_CACHE,
                config: hw_cache_config(
                    PERF_COUNT_HW_CACHE_LL,
                    PERF_COUNT_HW_CACHE_OP_READ,
                    PERF_COUNT_HW_CACHE_RESULT_MISS,
                ),
            },
            Self::TaskClock => EventSpec {
                type_: PERF_TYPE_SOFTWARE,
                config: PERF_COUNT_SW_TASK_CLOCK as u64,
            },
            Self::PageFaults => EventSpec {
                type_: PERF_TYPE_SOFTWARE,
                config: PERF_COUNT_SW_PAGE_FAULTS as u64,
            },
            Self::ContextSwitches => EventSpec {
                type_: PERF_TYPE_SOFTWARE,
                config: PERF_COUNT_SW_CONTEXT_SWITCHES as u64,
            },
        }
    }
}

const fn hw_cache_config(cache_id: u32, op_id: u32, result_id: u32) -> u64 {
    (cache_id as u64) | ((op_id as u64) << 8) | ((result_id as u64) << 16)
}

impl fmt::Display for CounterAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown counter alias or malformed override string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid counter specification '{spec}': {reason}")]
pub struct CounterParseError {
    pub spec: String,
    pub reason: String,
}

impl FromStr for CounterAlias {
    type Err = CounterParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cycles" => Ok(Self::Cycles),
            "instructions" => Ok(Self::Instructions),
            "cache-misses" => Ok(Self::CacheMisses),
            "cache-references" => Ok(Self::CacheReferences),
            "branch-misses" => Ok(Self::BranchMisses),
            "branches" => Ok(Self::Branches),
            "l1d-misses" => Ok(Self::L1dMisses),
            "llc-misses" => Ok(Self::LlcMisses),
            "stalled-backend" => Ok(Self::StalledBackend),
            "stalled-frontend" => Ok(Self::StalledFrontend),
            "task-clock" => Ok(Self::TaskClock),
            "page-faults" => Ok(Self::PageFaults),
            "context-switches" => Ok(Self::ContextSwitches),
            _ => Err(CounterParseError {
                spec: s.to_string(),
                reason: "unknown counter alias".to_string(),
            }),
        }
    }
}

/// Low-level `perf_event_attr` `(type_, config)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSpec {
    pub type_: u32,
    pub config: u64,
}

/// Parses a counter override string (`r<hex>` for raw PMU events or `<type>:<config>`).
pub fn parse_override_spec(raw: &str) -> Result<EventSpec, CounterParseError> {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed.strip_prefix('r') {
        let hex = hex.strip_prefix("0x").unwrap_or(hex);
        if hex.is_empty() {
            return Err(CounterParseError {
                spec: raw.to_string(),
                reason: "empty raw event hex value".to_string(),
            });
        }
        let config = u64::from_str_radix(hex, 16).map_err(|e| CounterParseError {
            spec: raw.to_string(),
            reason: format!("invalid raw hex config: {e}"),
        })?;
        return Ok(EventSpec {
            type_: PERF_TYPE_RAW,
            config,
        });
    }

    if let Some((ty_s, cfg_s)) = trimmed.split_once(':') {
        let type_ = parse_u32_auto(ty_s.trim()).map_err(|e| CounterParseError {
            spec: raw.to_string(),
            reason: format!("invalid event type: {e}"),
        })?;
        let config = parse_u64_auto(cfg_s.trim()).map_err(|e| CounterParseError {
            spec: raw.to_string(),
            reason: format!("invalid event config: {e}"),
        })?;
        return Ok(EventSpec { type_, config });
    }

    Err(CounterParseError {
        spec: raw.to_string(),
        reason: "expected 'r<hex>' or '<type>:<config>'".to_string(),
    })
}

fn parse_u32_auto(s: &str) -> Result<u32, std::num::ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        s.parse::<u32>()
    }
}

fn parse_u64_auto(s: &str) -> Result<u64, std::num::ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
    } else {
        s.parse::<u64>()
    }
}

/// Returns `true` if `time_running_ns < time_enabled_ns` beyond a 1% tolerance (R-REC-5).
///
/// Uses exact integer arithmetic: `(enabled - running) * 100 > enabled`.
pub fn mux_detected(count: &RawCount) -> bool {
    if count.time_enabled_ns == 0 || count.time_running_ns >= count.time_enabled_ns {
        return false;
    }
    let missing = count.time_enabled_ns - count.time_running_ns;
    missing.saturating_mul(100) > count.time_enabled_ns
}

/// Emits a single `mux_detected` warning if any counter in `counts` experienced > 1% multiplexing.
///
/// When any counter has `time_running_ns == 0 && time_enabled_ns > 0`, the warning explicitly
/// notes that the counter was never scheduled on the PMU.
pub fn check_mux_warning(counts: &BTreeMap<String, RawCount>) -> Option<Warning> {
    let mut muxed = Vec::new();
    let mut never_scheduled = Vec::new();

    for (name, c) in counts {
        if c.time_enabled_ns > 0 && c.time_running_ns == 0 {
            never_scheduled.push(name.as_str());
        } else if mux_detected(c) {
            muxed.push(name.as_str());
        }
    }

    if muxed.is_empty() && never_scheduled.is_empty() {
        return None;
    }

    let message = if !never_scheduled.is_empty() && muxed.is_empty() {
        format!(
            "counter(s) ({}) were never scheduled on the PMU (time_running_ns == 0 while time_enabled_ns > 0)",
            never_scheduled.join(", ")
        )
    } else if !never_scheduled.is_empty() {
        format!(
            "PMU multiplexing detected on ({}); counter(s) ({}) were never scheduled on the PMU (time_running_ns == 0); ratio metrics carry multiplexing uncertainty",
            muxed.join(", "),
            never_scheduled.join(", ")
        )
    } else {
        format!(
            "PMU multiplexing detected on counters ({}); ratio metrics carry multiplexing uncertainty",
            muxed.join(", ")
        )
    };

    Some(Warning {
        code: WarningCode::MuxDetected,
        message,
    })
}

struct OpenedCounter {
    name: String,
    kernel_excluded: bool,
    counter: Counter,
}

/// Collection of individually opened `perf_event_open` counters for a gated child (Spec §9.3 B1–B3).
pub struct CounterSet {
    opened: Vec<OpenedCounter>,
    warnings: Vec<Warning>,
}

impl CounterSet {
    /// Opens each alias in `aliases` on `pid` with `CounterScope::Auto` and no overrides.
    pub fn open(pid: libc::pid_t, aliases: &[CounterAlias]) -> Self {
        let empty = BTreeMap::new();
        Self::open_with_overrides(pid, aliases, &empty, CounterScope::Auto)
    }

    /// Opens each alias in `aliases` on `pid`, applying machine overrides from
    /// `runspec.counters_override` and `scope` (`Auto`, `User`, or `All`).
    /// Unavailable counters produce a `tier_degraded` warning naming the alias and errno
    /// without aborting the remaining counters.
    pub fn open_with_overrides(
        pid: libc::pid_t,
        aliases: &[CounterAlias],
        overrides: &BTreeMap<String, String>,
        scope: CounterScope,
    ) -> Self {
        let mut opened = Vec::with_capacity(aliases.len());
        let mut warnings = Vec::new();

        for &alias in aliases {
            let name = alias.as_str().to_string();
            let spec = if let Some(override_str) = overrides.get(&name) {
                match parse_override_spec(override_str) {
                    Ok(s) => s,
                    Err(e) => {
                        warnings.push(Warning {
                            code: WarningCode::TierDegraded,
                            message: format!(
                                "counter '{name}' override '{override_str}' invalid: {e}"
                            ),
                        });
                        continue;
                    }
                }
            } else {
                alias.event_spec()
            };

            let res = match scope {
                CounterScope::User => Counter::open_with_type(spec.type_, spec.config, pid, true)
                    .map(|counter| (counter, true)),
                CounterScope::All => Counter::open_with_type(spec.type_, spec.config, pid, false)
                    .map(|counter| (counter, false)),
                CounterScope::Auto => {
                    match Counter::open_with_type(spec.type_, spec.config, pid, false) {
                        Ok(counter) => Ok((counter, false)),
                        Err(err)
                            if matches!(
                                err.raw_os_error(),
                                Some(libc::EACCES | libc::EPERM)
                            ) =>
                        {
                            Counter::open_with_type(spec.type_, spec.config, pid, true)
                                .map(|counter| (counter, true))
                        }
                        Err(err) => Err(err),
                    }
                }
            };

            match res {
                Ok((counter, kernel_excluded)) => opened.push(OpenedCounter {
                    name,
                    kernel_excluded,
                    counter,
                }),
                Err(err) => {
                    let errno = err.raw_os_error().unwrap_or(0);
                    warnings.push(Warning {
                        code: WarningCode::TierDegraded,
                        message: format!(
                            "perf_event_open failed for counter '{name}' (errno {errno}): {err}"
                        ),
                    });
                }
            }
        }

        Self { opened, warnings }
    }

    /// Returns `true` if at least one counter opened successfully.
    pub fn any_open(&self) -> bool {
        !self.opened.is_empty()
    }

    /// Returns the names of all counters that opened successfully.
    pub fn open_names(&self) -> Vec<&str> {
        self.opened.iter().map(|c| c.name.as_str()).collect()
    }

    /// Returns the names of opened counters where `exclude_kernel` is `true`.
    pub fn kernel_excluded(&self) -> Vec<&str> {
        self.opened
            .iter()
            .filter(|c| c.kernel_excluded)
            .map(|c| c.name.as_str())
            .collect()
    }

    /// Returns warnings emitted while opening or reading counters.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Reads all opened counters.
    ///
    /// Note: counters opened with `inherit = 1` only include a child thread's counts
    /// after that thread exits, so mid-run reads undercount; only the final read after
    /// `wait4` is complete.
    pub fn read_all(&mut self) -> BTreeMap<String, RawCount> {
        let mut out = BTreeMap::new();
        for entry in &self.opened {
            match entry.counter.read() {
                Ok(raw) => {
                    out.insert(entry.name.clone(), raw);
                }
                Err(err) => {
                    let errno = err.raw_os_error().unwrap_or(0);
                    self.warnings.push(Warning {
                        code: WarningCode::TierDegraded,
                        message: format!(
                            "perf_event read failed for counter '{}' (errno {errno}): {err}",
                            entry.name
                        ),
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_table_round_trip() {
        for &alias in CounterAlias::all() {
            let s = alias.to_string();
            let parsed = CounterAlias::from_str(&s).expect("parse known alias");
            assert_eq!(parsed, alias);
        }
        assert!(CounterAlias::from_str("nonexistent-counter").is_err());
    }

    #[test]
    fn override_parsing_raw_and_typed() {
        let raw = parse_override_spec("r01c2").expect("parse r01c2");
        assert_eq!(
            raw,
            EventSpec {
                type_: PERF_TYPE_RAW,
                config: 0x01c2,
            }
        );

        let typed = parse_override_spec("4:0x1234").expect("parse 4:0x1234");
        assert_eq!(
            typed,
            EventSpec {
                type_: 4,
                config: 0x1234,
            }
        );

        assert!(parse_override_spec("r").is_err());
        assert!(parse_override_spec("bad").is_err());
    }

    #[test]
    fn mux_detected_respects_one_percent_tolerance() {
        assert!(!mux_detected(&RawCount {
            value: 100,
            time_enabled_ns: 0,
            time_running_ns: 0,
        }));
        assert!(!mux_detected(&RawCount {
            value: 100,
            time_enabled_ns: 1_000_000,
            time_running_ns: 1_000_000,
        }));
        assert!(!mux_detected(&RawCount {
            value: 100,
            time_enabled_ns: 1_000_000,
            time_running_ns: 990_000,
        }));
        assert!(mux_detected(&RawCount {
            value: 100,
            time_enabled_ns: 1_000_000,
            time_running_ns: 989_000,
        }));
        assert!(mux_detected(&RawCount {
            value: 0,
            time_enabled_ns: 1_000_000,
            time_running_ns: 0,
        }));
    }

    #[test]
    fn check_mux_warning_notes_never_scheduled_counter() {
        let mut counts = BTreeMap::new();
        counts.insert(
            "cycles".to_string(),
            RawCount {
                value: 0,
                time_enabled_ns: 1_000_000,
                time_running_ns: 0,
            },
        );
        let w = check_mux_warning(&counts).expect("expected mux warning");
        assert_eq!(w.code, WarningCode::MuxDetected);
        assert!(w.message.contains("never scheduled"));
        assert!(w.message.contains("cycles"));
    }
}
