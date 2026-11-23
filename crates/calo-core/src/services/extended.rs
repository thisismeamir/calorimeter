//! Table-based and comparative services plus engineer-owned service stubs (spec §12.3).

use crate::services::error::ServiceError;
use crate::services::frame::{Column, Frame};
use crate::services::registry::Options;
use crate::services::Service;
use calo_schema::suite::ScalingKind;
use calo_schema::Tier;
use std::collections::BTreeMap;

const TIERS_T0: &[Tier] = &[Tier::T0];
const TIERS_T2: &[Tier] = &[Tier::T2];
const TIERS_M0: &[Tier] = &[Tier::M0];
const TIERS_M1: &[Tier] = &[Tier::M1];
const TIERS_G0: &[Tier] = &[Tier::G0];
const TIERS_G1: &[Tier] = &[Tier::G1];

/// Stub service for engineer-owned analyses requiring symbolization or clock alignment (Task 16).
pub struct StubService {
    name: &'static str,
    requires: &'static [Tier],
}

impl StubService {
    /// Creates a `StubService` returning `ServiceError::NotYetImplemented`.
    pub const fn new(name: &'static str, requires: &'static [Tier]) -> Self {
        Self { name, requires }
    }
}

impl Service for StubService {
    fn name(&self) -> &'static str {
        self.name
    }

    fn inputs(&self) -> &'static [&'static str] {
        &[]
    }

    fn produces(&self) -> &'static [&'static str] {
        &[]
    }

    fn requires(&self) -> &'static [Tier] {
        self.requires
    }

    fn run(&self, _table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        Err(ServiceError::NotYetImplemented {
            service: self.name.to_string(),
        })
    }
}

/// `compare`: Cliff's delta effect size, Mann-Whitney U with normal approximation and tie correction,
/// and seeded bootstrap CI of the median difference between `baseline` and `observed` (spec §12.3).
pub struct CompareService;

impl Service for CompareService {
    fn name(&self) -> &'static str {
        "compare"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["baseline", "observed"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &[
            "cliffs_delta",
            "mann_whitney_u",
            "p_value",
            "median_diff",
            "ci_low",
            "ci_high",
        ]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T0
    }

    fn run(&self, table: &Frame, opts: &Options) -> Result<Frame, ServiceError> {
        let base = table
            .require_column(self.name(), "baseline")?
            .to_f64_vec("baseline")?;
        let obs = table
            .require_column(self.name(), "observed")?
            .to_f64_vec("observed")?;

        if base.is_empty() || obs.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "both `baseline` and `observed` must be non-empty".to_string(),
            });
        }

        // Cliff's delta & Mann-Whitney U
        let mut greater = 0usize;
        let mut lesser = 0usize;
        let mut u_obs = 0.0f64;
        for &o in &obs {
            for &b in &base {
                if o > b {
                    greater += 1;
                    u_obs += 1.0;
                } else if o < b {
                    lesser += 1;
                } else {
                    u_obs += 0.5;
                }
            }
        }
        let n1 = base.len() as f64;
        let n2 = obs.len() as f64;
        let cliffs_delta = ((greater as f64) - (lesser as f64)) / (n1 * n2);

        // Normal approximation with tie correction for Mann-Whitney U
        let mut combined: Vec<f64> = base.iter().chain(obs.iter()).copied().collect();
        combined.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n_tot = n1 + n2;
        let mut tie_sum = 0.0f64;
        let mut i = 0;
        while i < combined.len() {
            let mut j = i + 1;
            while j < combined.len() && (combined[j] - combined[i]).abs() == 0.0 {
                j += 1;
            }
            let t = (j - i) as f64;
            if t > 1.0 {
                tie_sum += t * t * t - t;
            }
            i = j;
        }
        let mean_u = 0.5 * n1 * n2;
        let var_u = (n1 * n2 / 12.0) * ((n_tot + 1.0) - tie_sum / (n_tot * (n_tot - 1.0)).max(1.0));
        let p_value = if var_u > 0.0 {
            let z = (u_obs - mean_u).abs() / var_u.sqrt();
            erfc_approx(z / std::f64::consts::SQRT_2)
        } else {
            1.0
        };

        let med_diff = median_f64(&obs) - median_f64(&base);
        let (ci_low, ci_high) = bootstrap_median_diff(
            &base,
            &obs,
            opts.ci.level,
            opts.ci.resamples.max(1),
            opts.ci.seed,
        );

        let mut out = Frame::new();
        out.insert_column("cliffs_delta", Column::Float(vec![cliffs_delta]))?;
        out.insert_column("mann_whitney_u", Column::Float(vec![u_obs]))?;
        out.insert_column("p_value", Column::Float(vec![p_value]))?;
        out.insert_column("median_diff", Column::Float(vec![med_diff]))?;
        out.insert_column("ci_low", Column::Float(vec![ci_low]))?;
        out.insert_column("ci_high", Column::Float(vec![ci_high]))?;
        Ok(out)
    }
}

/// `scaling_fit`: OLS fit for strong (`T = a + b/P`) and weak (`T = a + b*ln(P)`) scaling (spec §12.3).
pub struct ScalingFitService;

impl Service for ScalingFitService {
    fn name(&self) -> &'static str {
        "scaling_fit"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["ranks", "wall_s"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["a", "b", "r2", "serial_fraction"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T0
    }

    fn run(&self, table: &Frame, opts: &Options) -> Result<Frame, ServiceError> {
        let p_vals = table
            .require_column(self.name(), &opts.scaling_over)?
            .to_f64_vec(&opts.scaling_over)?;
        let t_vals = table
            .require_column(self.name(), &opts.metric)?
            .to_f64_vec(&opts.metric)?;

        if p_vals.len() < 2 {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "scaling_fit requires at least 2 data points".to_string(),
            });
        }

        let xs: Vec<f64> = p_vals
            .iter()
            .map(|&p| match opts.scaling_kind {
                ScalingKind::Strong => 1.0 / p.max(1e-12),
                ScalingKind::Weak => p.max(1e-12).ln(),
            })
            .collect();

        let (a, b, r2) = ols_fit(&xs, &t_vals);
        let serial_fraction = match opts.scaling_kind {
            ScalingKind::Strong => {
                if (a + b).abs() > 1e-15 {
                    a / (a + b)
                } else {
                    0.0
                }
            }
            ScalingKind::Weak => {
                if a.abs() > 1e-15 {
                    b / a
                } else {
                    0.0
                }
            }
        };

        let mut out = Frame::new();
        out.insert_column("a", Column::Float(vec![a]))?;
        out.insert_column("b", Column::Float(vec![b]))?;
        out.insert_column("r2", Column::Float(vec![r2]))?;
        out.insert_column("serial_fraction", Column::Float(vec![serial_fraction]))?;
        Ok(out)
    }
}

/// `region_breakdown`: inclusive and exclusive share of wall time per region (spec §12.3).
pub struct RegionBreakdownService;

impl Service for RegionBreakdownService {
    fn name(&self) -> &'static str {
        "region_breakdown"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["region", "incl_s", "excl_s", "wall_s"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["region", "incl_share", "excl_share"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_T2
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let regions = match table.require_column(self.name(), "region")? {
            Column::Str(v) => v,
            other => {
                return Err(ServiceError::ColumnType {
                    column: "region".to_string(),
                    expected: "str",
                    actual: other.type_name(),
                })
            }
        };
        let incl = table
            .require_column(self.name(), "incl_s")?
            .to_f64_vec("incl_s")?;
        let excl = table
            .require_column(self.name(), "excl_s")?
            .to_f64_vec("excl_s")?;
        let wall = table
            .require_column(self.name(), "wall_s")?
            .to_f64_vec("wall_s")?;

        if regions.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty region table".to_string(),
            });
        }

        let mut acc: BTreeMap<String, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
        for i in 0..regions.len() {
            if wall[i] <= 0.0 {
                return Err(ServiceError::InvalidInput {
                    service: self.name().to_string(),
                    reason: "wall_s must be > 0".to_string(),
                });
            }
            let entry = acc.entry(regions[i].clone()).or_default();
            entry.0.push(incl[i] / wall[i]);
            entry.1.push(excl[i] / wall[i]);
        }

        let mut out_reg = Vec::with_capacity(acc.len());
        let mut out_incl = Vec::with_capacity(acc.len());
        let mut out_excl = Vec::with_capacity(acc.len());
        for (reg, (is, es)) in acc {
            out_reg.push(reg);
            out_incl.push(is.iter().sum::<f64>() / (is.len() as f64));
            out_excl.push(es.iter().sum::<f64>() / (es.len() as f64));
        }

        let mut out = Frame::new();
        out.insert_column("region", Column::Str(out_reg))?;
        out.insert_column("incl_share", Column::Float(out_incl))?;
        out.insert_column("excl_share", Column::Float(out_excl))?;
        Ok(out)
    }
}

/// `comm_fraction`: MPI time fraction from M1 (`mpi_time_s / wall_s`) or M0 (`mpi_samples / total_samples`, spec §12.3).
pub struct CommFractionService;

impl Service for CommFractionService {
    fn name(&self) -> &'static str {
        "comm_fraction"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["wall_s"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["comm_fraction"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_M0
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let frac = if let (Some(mpi_t), Some(wall)) =
            (table.column("mpi_time_s"), table.column("wall_s"))
        {
            let m = mpi_t.to_f64_vec("mpi_time_s")?;
            let w = wall.to_f64_vec("wall_s")?;
            if m.is_empty() {
                return Err(ServiceError::InvalidInput {
                    service: self.name().to_string(),
                    reason: "empty input".to_string(),
                });
            }
            m.into_iter()
                .zip(w)
                .map(|(mi, wi)| if wi > 0.0 { mi / wi } else { 0.0 })
                .collect()
        } else {
            let mpi_s = table
                .require_column(self.name(), "mpi_samples")?
                .to_f64_vec("mpi_samples")?;
            let tot_s = table
                .require_column(self.name(), "total_samples")?
                .to_f64_vec("total_samples")?;
            if mpi_s.is_empty() {
                return Err(ServiceError::InvalidInput {
                    service: self.name().to_string(),
                    reason: "empty sample counts".to_string(),
                });
            }
            mpi_s
                .into_iter()
                .zip(tot_s)
                .map(|(mi, ti)| if ti > 0.0 { mi / ti } else { 0.0 })
                .collect()
        };

        let mut out = Frame::new();
        out.insert_column("comm_fraction", Column::Float(frac))?;
        Ok(out)
    }
}

/// `message_size_profile`: summarizes message counts and byte buckets across MPI transfers (spec §12.3).
pub struct MessageSizeProfileService;

impl Service for MessageSizeProfileService {
    fn name(&self) -> &'static str {
        "message_size_profile"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["bytes"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["bucket", "msgs", "total_bytes"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_M1
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let bytes = table
            .require_column(self.name(), "bytes")?
            .to_f64_vec("bytes")?;
        if bytes.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty bytes column".to_string(),
            });
        }
        let msgs: Vec<f64> = match table.column("msgs") {
            Some(c) => c.to_f64_vec("msgs")?,
            None => vec![1.0; bytes.len()],
        };

        let buckets = ["<=64B", "<=1KB", "<=64KB", "<=1MB", ">1MB"];
        let mut counts = [0i64; 5];
        let mut tot_bytes = [0i64; 5];

        for (b, m) in bytes.into_iter().zip(msgs) {
            let m_count = m.max(1.0) as i64;
            let avg_size = b / (m.max(1.0));
            let idx = if avg_size <= 64.0 {
                0
            } else if avg_size <= 1024.0 {
                1
            } else if avg_size <= 65536.0 {
                2
            } else if avg_size <= 1_048_576.0 {
                3
            } else {
                4
            };
            counts[idx] += m_count;
            tot_bytes[idx] += b as i64;
        }

        let mut out = Frame::new();
        out.insert_column(
            "bucket",
            Column::Str(buckets.iter().map(|s| s.to_string()).collect()),
        )?;
        out.insert_column("msgs", Column::Int(counts.to_vec()))?;
        out.insert_column("total_bytes", Column::Int(tot_bytes.to_vec()))?;
        Ok(out)
    }
}

/// `comm_matrix`: dense rank-by-rank communication matrix (`src_rank`, `dst_rank`, `msgs`, `bytes`, spec §12.3).
pub struct CommMatrixService;

impl Service for CommMatrixService {
    fn name(&self) -> &'static str {
        "comm_matrix"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["src_rank", "peer_rank", "msgs", "bytes"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["src_rank", "dst_rank", "msgs", "bytes"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_M1
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let src = table
            .require_column(self.name(), "src_rank")?
            .to_f64_vec("src_rank")?;
        let peer = table
            .require_column(self.name(), "peer_rank")?
            .to_f64_vec("peer_rank")?;
        let msgs = table
            .require_column(self.name(), "msgs")?
            .to_f64_vec("msgs")?;
        let bytes = table
            .require_column(self.name(), "bytes")?
            .to_f64_vec("bytes")?;

        if src.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty peer table".to_string(),
            });
        }

        let dir_opt = table
            .column("direction")
            .and_then(|c| c.to_f64_vec("direction").ok());

        let mut max_rank = 0i64;
        let mut pairs: BTreeMap<(i64, i64), (i64, i64)> = BTreeMap::new();

        for i in 0..src.len() {
            let s = src[i] as i64;
            let p = peer[i] as i64;
            max_rank = max_rank.max(s).max(p);
            let is_recv = dir_opt.as_ref().is_some_and(|d| (d[i] as u8) == 1);
            let key = if is_recv { (p, s) } else { (s, p) };
            let entry = pairs.entry(key).or_insert((0, 0));
            entry.0 += msgs[i] as i64;
            entry.1 += bytes[i] as i64;
        }

        let n_ranks = (max_rank + 1).max(1);
        let mut out_src = Vec::with_capacity((n_ranks * n_ranks) as usize);
        let mut out_dst = Vec::with_capacity((n_ranks * n_ranks) as usize);
        let mut out_msgs = Vec::with_capacity((n_ranks * n_ranks) as usize);
        let mut out_bytes = Vec::with_capacity((n_ranks * n_ranks) as usize);

        for s in 0..n_ranks {
            for d in 0..n_ranks {
                let (m, b) = pairs.get(&(s, d)).copied().unwrap_or((0, 0));
                out_src.push(s);
                out_dst.push(d);
                out_msgs.push(m);
                out_bytes.push(b);
            }
        }

        let mut out = Frame::new();
        out.insert_column("src_rank", Column::Int(out_src))?;
        out.insert_column("dst_rank", Column::Int(out_dst))?;
        out.insert_column("msgs", Column::Int(out_msgs))?;
        out.insert_column("bytes", Column::Int(out_bytes))?;
        Ok(out)
    }
}

/// `kernel_hotspots`: aggregates GPU kernel durations (`start_ns`, `end_ns`) per `name`, sorted descending (spec §12.3).
pub struct KernelHotspotsService;

impl Service for KernelHotspotsService {
    fn name(&self) -> &'static str {
        "kernel_hotspots"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["name", "start_ns", "end_ns"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["name", "calls", "total_s", "mean_s", "min_s", "max_s"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_G1
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let names = match table.require_column(self.name(), "name")? {
            Column::Str(v) => v,
            other => {
                return Err(ServiceError::ColumnType {
                    column: "name".to_string(),
                    expected: "str",
                    actual: other.type_name(),
                })
            }
        };
        let start = table
            .require_column(self.name(), "start_ns")?
            .to_f64_vec("start_ns")?;
        let end = table
            .require_column(self.name(), "end_ns")?
            .to_f64_vec("end_ns")?;

        if names.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty gpu.kernels table".to_string(),
            });
        }

        let mut grouped: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for i in 0..names.len() {
            let dur_s = ((end[i] - start[i]).max(0.0)) * 1e-9;
            grouped.entry(names[i].clone()).or_default().push(dur_s);
        }

        let mut rows: Vec<(String, i64, f64, f64, f64, f64)> = grouped
            .into_iter()
            .map(|(k, durs)| {
                let calls = durs.len() as i64;
                let total_s: f64 = durs.iter().sum();
                let mean_s = total_s / (durs.len() as f64);
                let min_s = durs.iter().copied().fold(f64::INFINITY, f64::min);
                let max_s = durs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                (k, calls, total_s, mean_s, min_s, max_s)
            })
            .collect();

        rows.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        let mut out = Frame::new();
        out.insert_column(
            "name",
            Column::Str(rows.iter().map(|r| r.0.clone()).collect()),
        )?;
        out.insert_column("calls", Column::Int(rows.iter().map(|r| r.1).collect()))?;
        out.insert_column("total_s", Column::Float(rows.iter().map(|r| r.2).collect()))?;
        out.insert_column("mean_s", Column::Float(rows.iter().map(|r| r.3).collect()))?;
        out.insert_column("min_s", Column::Float(rows.iter().map(|r| r.4).collect()))?;
        out.insert_column("max_s", Column::Float(rows.iter().map(|r| r.5).collect()))?;
        Ok(out)
    }
}

/// `transfer_summary`: aggregates `gpu.memcpy` bytes, duration, and effective GB/s per `kind` (spec §12.3).
pub struct TransferSummaryService;

impl Service for TransferSummaryService {
    fn name(&self) -> &'static str {
        "transfer_summary"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["kind", "bytes", "start_ns", "end_ns"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["kind", "count", "total_bytes", "total_s", "bandwidth_gbs"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_G1
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let kinds = table
            .require_column(self.name(), "kind")?
            .to_f64_vec("kind")?;
        let bytes = table
            .require_column(self.name(), "bytes")?
            .to_f64_vec("bytes")?;
        let start = table
            .require_column(self.name(), "start_ns")?
            .to_f64_vec("start_ns")?;
        let end = table
            .require_column(self.name(), "end_ns")?
            .to_f64_vec("end_ns")?;

        if kinds.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty gpu.memcpy table".to_string(),
            });
        }

        let mut grouped: BTreeMap<u8, (i64, i64, f64)> = BTreeMap::new();
        for i in 0..kinds.len() {
            let k = kinds[i] as u8;
            let dur_s = ((end[i] - start[i]).max(0.0)) * 1e-9;
            let entry = grouped.entry(k).or_insert((0, 0, 0.0));
            entry.0 += 1;
            entry.1 += bytes[i] as i64;
            entry.2 += dur_s;
        }

        let mut out_kind = Vec::new();
        let mut out_count = Vec::new();
        let mut out_bytes = Vec::new();
        let mut out_s = Vec::new();
        let mut out_bw = Vec::new();

        for (k, (cnt, tot_b, tot_s)) in grouped {
            let label = match k {
                0 => "H2D",
                1 => "D2H",
                2 => "D2D",
                3 => "P2P",
                4 => "memset",
                _ => "other",
            };
            let bw = if tot_s > 0.0 {
                (tot_b as f64) / (tot_s * 1e9)
            } else {
                0.0
            };
            out_kind.push(label.to_string());
            out_count.push(cnt);
            out_bytes.push(tot_b);
            out_s.push(tot_s);
            out_bw.push(bw);
        }

        let mut out = Frame::new();
        out.insert_column("kind", Column::Str(out_kind))?;
        out.insert_column("count", Column::Int(out_count))?;
        out.insert_column("total_bytes", Column::Int(out_bytes))?;
        out.insert_column("total_s", Column::Float(out_s))?;
        out.insert_column("bandwidth_gbs", Column::Float(out_bw))?;
        Ok(out)
    }
}

/// `gpu_utilization`: time-weighted mean and percentiles (`p50`, `p95`, `p99`) from `gpu.state` (spec §12.3).
pub struct GpuUtilizationService;

impl Service for GpuUtilizationService {
    fn name(&self) -> &'static str {
        "gpu_utilization"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["t_ns", "util_pct"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["mean_util_pct", "p50", "p95", "p99"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_G0
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let t_ns = table
            .require_column(self.name(), "t_ns")?
            .to_f64_vec("t_ns")?;
        let util = table
            .require_column(self.name(), "util_pct")?
            .to_f64_vec("util_pct")?;

        let valid: Vec<(f64, f64)> = t_ns
            .into_iter()
            .zip(util)
            .filter(|(_, u)| u.is_finite())
            .collect();

        if valid.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "no valid gpu.state utilization samples".to_string(),
            });
        }

        let mean_util = if valid.len() == 1 {
            valid[0].1
        } else {
            let mut weighted_sum = 0.0;
            let mut total_dt = 0.0;
            for win in valid.windows(2) {
                let dt = (win[1].0 - win[0].0).max(0.0);
                weighted_sum += win[0].1 * dt;
                total_dt += dt;
            }
            if total_dt > 0.0 {
                weighted_sum / total_dt
            } else {
                valid.iter().map(|(_, u)| *u).sum::<f64>() / (valid.len() as f64)
            }
        };

        let mut sorted_u: Vec<f64> = valid.iter().map(|(_, u)| *u).collect();
        sorted_u.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let mut out = Frame::new();
        out.insert_column("mean_util_pct", Column::Float(vec![mean_util]))?;
        out.insert_column("p50", Column::Float(vec![percentile(&sorted_u, 0.50)]))?;
        out.insert_column("p95", Column::Float(vec![percentile(&sorted_u, 0.95)]))?;
        out.insert_column("p99", Column::Float(vec![percentile(&sorted_u, 0.99)]))?;
        Ok(out)
    }
}

/// `launch_overhead`: summarizes GPU kernel launch latency (`launch_ns`, spec §12.3).
pub struct LaunchOverheadService;

impl Service for LaunchOverheadService {
    fn name(&self) -> &'static str {
        "launch_overhead"
    }

    fn inputs(&self) -> &'static [&'static str] {
        &["launch_ns"]
    }

    fn produces(&self) -> &'static [&'static str] {
        &["launches", "total_launch_s", "mean_launch_us", "max_launch_us"]
    }

    fn requires(&self) -> &'static [Tier] {
        TIERS_G1
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let launch_ns = table
            .require_column(self.name(), "launch_ns")?
            .to_f64_vec("launch_ns")?;
        if launch_ns.is_empty() {
            return Err(ServiceError::InvalidInput {
                service: self.name().to_string(),
                reason: "empty launch_ns column".to_string(),
            });
        }

        let count = launch_ns.len() as i64;
        let total_ns: f64 = launch_ns.iter().sum();
        let max_ns = launch_ns.iter().copied().fold(0.0, f64::max);

        let mut out = Frame::new();
        out.insert_column("launches", Column::Int(vec![count]))?;
        out.insert_column("total_launch_s", Column::Float(vec![total_ns * 1e-9]))?;
        out.insert_column(
            "mean_launch_us",
            Column::Float(vec![(total_ns / (count as f64)) * 1e-3]),
        )?;
        out.insert_column("max_launch_us", Column::Float(vec![max_ns * 1e-3]))?;
        Ok(out)
    }
}

fn ols_fit(xs: &[f64], ys: &[f64]) -> (f64, f64, f64) {
    let n = xs.len() as f64;
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;

    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syy = 0.0;
    for (&x, &y) in xs.iter().zip(ys.iter()) {
        let dx = x - mean_x;
        let dy = y - mean_y;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }

    let b = if sxx.abs() > 1e-18 { sxy / sxx } else { 0.0 };
    let a = mean_y - b * mean_x;
    let mut sse = 0.0;
    for (&x, &y) in xs.iter().zip(ys.iter()) {
        let err = y - (a + b * x);
        sse += err * err;
    }
    let r2 = if syy.abs() > 1e-18 {
        (1.0 - sse / syy).clamp(0.0, 1.0)
    } else {
        1.0
    };
    (a, b, r2)
}

fn median_f64(vals: &[f64]) -> f64 {
    let mut s = vals.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = s.len() / 2;
    if s.len() % 2 == 1 {
        s[mid]
    } else {
        0.5 * (s[mid - 1] + s[mid])
    }
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = q.clamp(0.0, 1.0) * ((sorted.len() - 1) as f64);
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    let frac = idx - (lo as f64);
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

fn bootstrap_median_diff(
    base: &[f64],
    obs: &[f64],
    level: f64,
    resamples: u32,
    seed: u64,
) -> (f64, f64) {
    let mut state = seed;
    let mut next_idx = |len: usize| -> usize {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z as usize) % len
    };

    let b = resamples as usize;
    let mut diffs = Vec::with_capacity(b);
    let mut draw_b = vec![0.0; base.len()];
    let mut draw_o = vec![0.0; obs.len()];

    for _ in 0..b {
        for slot in &mut draw_b {
            *slot = base[next_idx(base.len())];
        }
        for slot in &mut draw_o {
            *slot = obs[next_idx(obs.len())];
        }
        diffs.push(median_f64(&draw_o) - median_f64(&draw_b));
    }

    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let alpha = (1.0 - level).clamp(0.0, 1.0);
    let low_idx = ((alpha * 0.5) * ((b - 1) as f64)).floor() as usize;
    let high_idx = ((1.0 - alpha * 0.5) * ((b - 1) as f64)).ceil() as usize;
    (diffs[low_idx.min(b - 1)], diffs[high_idx.min(b - 1)])
}

/// Abramowitz & Stegun 7.1.26 complementary error function approximation.
fn erfc_approx(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let ans = t
        * (-z * z - 1.26551223
            + t * (1.00002368
                + t * (0.37409196
                    + t * (0.09678418
                        + t * (-0.18628806
                            + t * (0.27886807
                                + t * (-1.13520398
                                    + t * (1.48851587
                                        + t * (-0.82215223 + t * 0.17087277)))))))))
            .exp();
    if x >= 0.0 {
        ans
    } else {
        2.0 - ans
    }
}
