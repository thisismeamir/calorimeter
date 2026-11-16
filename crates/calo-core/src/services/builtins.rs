//! Built-in Rust services (`stats.summary`, `ipc`, `cache_miss_rate`, `speedup`, `parallel_efficiency`, `imbalance`, spec §12.3).

use crate::services::error::ServiceError;
use crate::services::frame::{Column, Frame};
use crate::services::registry::Options;
use crate::services::Service;
use calo_schema::{ScalarValue, Tier};

const TIERS_T0: &[Tier] = &[Tier::T0];
const TIERS_M0: &[Tier] = &[Tier::M0];

/// `stats.summary`: median/mean estimator, deterministic bootstrap CI, and MAD outlier filter (spec §12.2, §12.3).
pub struct StatsSummaryService;

impl Service for StatsSummaryService {
    fn name(&self) -> &'static str {
        "stats.summary"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["wall_s"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &[
            "median",
            "mean",
            "ci_low",
            "ci_high",
            "mad",
            "outliers_removed",
            "n",
            "bootstrap_seed",
        ]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T0
    }

    fn run(&self, table: &Frame, opts: &Options) -> Result<Frame, ServiceError> {
        let col = table.require_column(self.name(), &opts.metric)?;
        let raw_vals = col.to_f64_vec(&opts.metric)?;
        if raw_vals.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "input column is empty".to_string(),
            });
        }

        let raw_median = compute_median(&raw_vals);
        let abs_devs: Vec<f64> = raw_vals.iter().map(|v| (v - raw_median).abs()).collect();
        let mad = compute_median(&abs_devs);

        // Apply MAD outlier filter if configured (spec §12.2)
        let (filtered, removed_count) = if let Some(out_cfg) = &opts.outliers {
            if out_cfg.method.eq_ignore_ascii_case("mad") {
                // Consistent normal scale factor for MAD = 1.4826
                let scaled_mad = mad * 1.4826;
                let kept: Vec<f64> = raw_vals
                    .iter()
                    .copied()
                    .filter(|&v| {
                        let dev = (v - raw_median).abs();
                        if scaled_mad > 0.0 {
                            dev <= out_cfg.threshold * scaled_mad
                        } else {
                            dev == 0.0
                        }
                    })
                    .collect();
                let removed = raw_vals.len().saturating_sub(kept.len());
                if kept.is_empty() {
                    (raw_vals.clone(), 0)
                } else {
                    (kept, removed)
                }
            } else {
                (raw_vals.clone(), 0)
            }
        } else {
            (raw_vals.clone(), 0)
        };

        let median = compute_median(&filtered);
        let mean = compute_mean(&filtered);
        let use_mean = opts.estimator.eq_ignore_ascii_case("mean");

        let (ci_low, ci_high) = bootstrap_ci(
            &filtered,
            use_mean,
            opts.ci.level,
            opts.ci.resamples.max(1),
            opts.ci.seed,
        );

        let mut out = Frame::new();
        out.insert_column("median", Column::Float(vec![median]))?;
        out.insert_column("mean", Column::Float(vec![mean]))?;
        out.insert_column("ci_low", Column::Float(vec![ci_low]))?;
        out.insert_column("ci_high", Column::Float(vec![ci_high]))?;
        out.insert_column("mad", Column::Float(vec![mad]))?;
        out.insert_column("outliers_removed", Column::Int(vec![removed_count as i64]))?;
        out.insert_column("n", Column::Int(vec![filtered.len() as i64]))?;
        out.insert_column("bootstrap_seed", Column::Int(vec![opts.ci.seed as i64]))?;
        out.set_meta("bootstrap_seed", ScalarValue::Int(opts.ci.seed as i64));
        Ok(out)
    }
}

/// `ipc`: instructions per cycle (`instructions / cycles`, spec §12.3).
pub struct IpcService;

impl Service for IpcService {
    fn name(&self) -> &'static str {
        "ipc"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["instructions", "cycles"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["ipc"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T0
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let instr = table
            .require_column(self.name(), "instructions")?
            .to_f64_vec("instructions")?;
        let cycles = table
            .require_column(self.name(), "cycles")?
            .to_f64_vec("cycles")?;

        let mut ipc = Vec::with_capacity(instr.len());
        for (i, c) in instr.into_iter().zip(cycles) {
            if c <= 0.0 {
                return Err(ServiceError::InvalidInput {
                    service: self.name().to_string(),
                    reason: "cycles must be > 0".to_string(),
                });
            }
            ipc.push(i / c);
        }

        let mut out = table.clone();
        out.insert_column("ipc", Column::Float(ipc))?;
        Ok(out)
    }
}

/// `cache_miss_rate`: cache misses per instruction (`cache-misses / instructions`, spec §12.3).
pub struct CacheMissRateService;

impl Service for CacheMissRateService {
    fn name(&self) -> &'static str {
        "cache_miss_rate"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["cache-misses", "instructions"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["cache_miss_rate"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T0
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let misses = table
            .require_column(self.name(), "cache-misses")?
            .to_f64_vec("cache-misses")?;
        let denom_col = if table.column("cache-references").is_some() {
            "cache-references"
        } else {
            "instructions"
        };
        let denom = table
            .require_column(self.name(), denom_col)?
            .to_f64_vec(denom_col)?;

        let mut rates = Vec::with_capacity(misses.len());
        for (m, d) in misses.into_iter().zip(denom) {
            if d <= 0.0 {
                return Err(ServiceError::InvalidInput {
                    service: self.name().to_string(),
                    reason: format!("{denom_col} must be > 0"),
                });
            }
            rates.push(m / d);
        }

        let mut out = table.clone();
        out.insert_column("cache_miss_rate", Column::Float(rates))?;
        Ok(out)
    }
}

/// `speedup`: ratio `T(base) / T` (spec §12.3).
pub struct SpeedupService;

impl Service for SpeedupService {
    fn name(&self) -> &'static str {
        "speedup"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["wall_s"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["speedup"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T0
    }

    fn run(&self, table: &Frame, opts: &Options) -> Result<Frame, ServiceError> {
        let times = table
            .require_column(self.name(), &opts.metric)?
            .to_f64_vec(&opts.metric)?;
        if times.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty time column".to_string(),
            });
        }

        let base_idx = find_baseline_row(table, opts).unwrap_or(0);
        let t_base = times[base_idx];

        let mut speedups = Vec::with_capacity(times.len());
        for t in times {
            if t <= 0.0 {
                return Err(ServiceError::InvalidInput {
                    service: self.name().to_string(),
                    reason: "time value must be > 0".to_string(),
                });
            }
            speedups.push(t_base / t);
        }

        let mut out = table.clone();
        out.insert_column("speedup", Column::Float(speedups))?;
        Ok(out)
    }
}

/// `parallel_efficiency`: strong (`T(base)*P(base) / (T*P)`) and weak (`T(base) / T`) scaling efficiency (spec §8.5 R-BEN-5, §12.3).
pub struct ParallelEfficiencyService;

impl Service for ParallelEfficiencyService {
    fn name(&self) -> &'static str {
        "parallel_efficiency"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["wall_s"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["parallel_efficiency"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T0
    }

    fn run(&self, table: &Frame, opts: &Options) -> Result<Frame, ServiceError> {
        let times = table
            .require_column(self.name(), &opts.metric)?
            .to_f64_vec(&opts.metric)?;
        if times.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty time column".to_string(),
            });
        }

        let base_idx = find_baseline_row(table, opts).unwrap_or(0);
        let t_base = times[base_idx];

        let eff = match opts.scaling_kind {
            calo_schema::suite::ScalingKind::Weak => {
                let mut v = Vec::with_capacity(times.len());
                for t in times {
                    if t <= 0.0 {
                        return Err(ServiceError::InvalidInput {
                            service: self.name().to_string(),
                            reason: "time value must be > 0".to_string(),
                        });
                    }
                    v.push(t_base / t);
                }
                v
            }
            calo_schema::suite::ScalingKind::Strong => {
                let p_vals = table
                    .require_column(self.name(), &opts.scaling_over)?
                    .to_f64_vec(&opts.scaling_over)?;
                let p_base = p_vals[base_idx];
                let mut v = Vec::with_capacity(times.len());
                for (t, p) in times.into_iter().zip(p_vals) {
                    if t <= 0.0 || p <= 0.0 {
                        return Err(ServiceError::InvalidInput {
                            service: self.name().to_string(),
                            reason: "time and resource count must be > 0".to_string(),
                        });
                    }
                    v.push((t_base * p_base) / (t * p));
                }
                v
            }
        };

        let mut out = table.clone();
        out.insert_column("parallel_efficiency", Column::Float(eff))?;
        Ok(out)
    }
}

/// `imbalance`: `max / mean` across ranks (spec §10 R-REC-7, §12.3).
pub struct ImbalanceService;

impl Service for ImbalanceService {
    fn name(&self) -> &'static str {
        "imbalance"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["wall_s"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["min", "mean", "max", "imbalance"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_M0
    }

    fn run(&self, table: &Frame, opts: &Options) -> Result<Frame, ServiceError> {
        let vals = table
            .require_column(self.name(), &opts.metric)?
            .to_f64_vec(&opts.metric)?;
        if vals.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty rank values".to_string(),
            });
        }

        let min_v = vals.iter().copied().fold(f64::INFINITY, f64::min);
        let max_v = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mean_v = compute_mean(&vals);
        if mean_v <= 0.0 {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "mean across ranks must be > 0".to_string(),
            });
        }
        let imb = max_v / mean_v;

        let mut out = Frame::new();
        out.insert_column("min", Column::Float(vec![min_v]))?;
        out.insert_column("mean", Column::Float(vec![mean_v]))?;
        out.insert_column("max", Column::Float(vec![max_v]))?;
        out.insert_column("imbalance", Column::Float(vec![imb]))?;
        Ok(out)
    }
}

fn find_baseline_row(table: &Frame, opts: &Options) -> Option<usize> {
    if opts.baseline.is_empty() {
        // If `scaling_over` column exists, pick the row with minimum resource count
        if let Some(col) = table.column(&opts.scaling_over) {
            if let Ok(p_vals) = col.to_f64_vec(&opts.scaling_over) {
                return p_vals
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(idx, _)| idx);
            }
        }
        return Some(0);
    }

    'row_loop: for row_idx in 0..table.num_rows() {
        for (k, expected) in &opts.baseline {
            let col = table
                .column(k)
                .or_else(|| table.column(&format!("params.{k}")))?;
            let actual = col.get_value(row_idx)?.to_scalar()?;
            if !crate::plan::selector::scalars_equal(&actual, expected) {
                continue 'row_loop;
            }
        }
        return Some(row_idx);
    }
    None
}

fn compute_mean(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.iter().sum::<f64>() / (vals.len() as f64)
}

fn compute_median(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let mut sorted = vals.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        0.5 * (sorted[mid - 1] + sorted[mid])
    }
}

/// Deterministic SplitMix64 PRNG for reproducible bootstrap resampling (spec §12.2 R-SVC-3).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_index(&mut self, len: usize) -> usize {
        (self.next_u64() as usize) % len
    }
}

fn bootstrap_ci(
    sample: &[f64],
    use_mean: bool,
    level: f64,
    resamples: u32,
    seed: u64,
) -> (f64, f64) {
    if sample.len() <= 1 {
        let v = sample.first().copied().unwrap_or(0.0);
        return (v, v);
    }

    let mut rng = SplitMix64::new(seed);
    let n = sample.len();
    let b = resamples as usize;
    let mut estimates = Vec::with_capacity(b);
    let mut draw = vec![0.0; n];

    for _ in 0..b {
        for slot in &mut draw {
            *slot = sample[rng.next_index(n)];
        }
        let est = if use_mean {
            compute_mean(&draw)
        } else {
            compute_median(&draw)
        };
        estimates.push(est);
    }

    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let alpha = (1.0 - level).clamp(0.0, 1.0);
    let low_idx = ((alpha * 0.5) * ((b - 1) as f64)).floor() as usize;
    let high_idx = ((1.0 - alpha * 0.5) * ((b - 1) as f64)).ceil() as usize;

    (
        estimates[low_idx.min(b - 1)],
        estimates[high_idx.min(b - 1)],
    )
}
