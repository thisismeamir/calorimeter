//! Multi-rank run aggregation producing `run.yaml` (spec §10 R-REC-7, §12.1 step 7).

use calo_schema::common::{CpuTier, GpuTier, MpiTier, Status, TiersActive, Warning, WarningCode};
use calo_schema::record::{
    ExitInfo, RankAggregateStats, Record, RecordEnvironment, RecordIdentity, RecordObservations,
    RecordProvenance, RegionNode, RunLevelAggregate,
};
use calo_schema::SchemaDoc;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Errors produced during multi-rank run aggregation (spec §10 R-REC-7).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AggregateError {
    /// Cannot aggregate an empty slice of rank records.
    #[error("cannot aggregate run: no rank records provided")]
    EmptyRankRecords,
}

/// Aggregates per-rank `calo.record/2` records into a single run-level `Record` (`run.yaml`, spec §10 R-REC-7).
///
/// - Computes `min`, `mean`, `max`, and `imbalance = max / mean` (omitted when `mean <= 0.0`)
///   for every scalar, phase, and region across the ranks that report it.
/// - `tiers_active` is the minimum tier achieved per domain across ranks, emitting
///   `WarningCode::TierDegraded` if ranks differ.
/// - `status` is `Failed` if any rank failed/invalid, `Timeout` if any timed out, else `Ok`.
pub fn aggregate_run(rank_records: &[Record]) -> Result<Record, AggregateError> {
    let Some(first) = rank_records.first() else {
        return Err(AggregateError::EmptyRankRecords);
    };

    // 1. Status propagation: Failed/Invalid > Timeout > Ok
    let mut agg_status = Status::Ok;
    for r in rank_records {
        match r.identity.status {
            Status::Failed | Status::Invalid => {
                agg_status = Status::Failed;
                break;
            }
            Status::Timeout => agg_status = Status::Timeout,
            Status::Ok => {}
        }
    }

    // 2. Minimum active tiers per domain + tier_degraded warning if ranks differ
    let min_cpu = min_opt_tier(rank_records.iter().map(|r| r.provenance.tiers_active.cpu));
    let min_mpi = min_opt_tier(rank_records.iter().map(|r| r.provenance.tiers_active.mpi));
    let min_gpu = min_opt_tier(rank_records.iter().map(|r| r.provenance.tiers_active.gpu));

    let cpu_differs = rank_records
        .iter()
        .any(|r| r.provenance.tiers_active.cpu != min_cpu);
    let mpi_differs = rank_records
        .iter()
        .any(|r| r.provenance.tiers_active.mpi != min_mpi);
    let gpu_differs = rank_records
        .iter()
        .any(|r| r.provenance.tiers_active.gpu != min_gpu);

    // 3. Collect scalar, counter, energy, and phase values across ranks (missing blocks tolerated)
    let mut scalar_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut region_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut rank_paths = Vec::with_capacity(rank_records.len());

    let mut warnings_seen: BTreeSet<(WarningCode, String)> = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut errors_seen: BTreeSet<(WarningCode, String)> = BTreeSet::new();
    let mut errors = Vec::new();

    for (idx, r) in rank_records.iter().enumerate() {
        let rank_num = r.identity.rank.unwrap_or(idx as u32);
        rank_paths.push(format!("rank{rank_num}.yaml"));

        for w in &r.provenance.warnings {
            if warnings_seen.insert((w.code, w.message.clone())) {
                warnings.push(w.clone());
            }
        }
        for e in &r.provenance.errors {
            if errors_seen.insert((e.code, e.message.clone())) {
                errors.push(e.clone());
            }
        }

        if let Some(s) = &r.observations.scalars {
            if let Some(v) = s.wall_s {
                scalar_samples
                    .entry("wall_s".to_string())
                    .or_default()
                    .push(v);
            }
            if let Some(v) = s.user_cpu_s {
                scalar_samples
                    .entry("user_cpu_s".to_string())
                    .or_default()
                    .push(v);
            }
            if let Some(v) = s.sys_cpu_s {
                scalar_samples
                    .entry("sys_cpu_s".to_string())
                    .or_default()
                    .push(v);
            }
            if let Some(v) = s.peak_rss_bytes {
                scalar_samples
                    .entry("peak_rss_bytes".to_string())
                    .or_default()
                    .push(v as f64);
            }
            if let Some(v) = s.minor_faults {
                scalar_samples
                    .entry("minor_faults".to_string())
                    .or_default()
                    .push(v as f64);
            }
            if let Some(v) = s.major_faults {
                scalar_samples
                    .entry("major_faults".to_string())
                    .or_default()
                    .push(v as f64);
            }
            if let Some(v) = s.voluntary_ctx {
                scalar_samples
                    .entry("voluntary_ctx".to_string())
                    .or_default()
                    .push(v as f64);
            }
            if let Some(v) = s.involuntary_ctx {
                scalar_samples
                    .entry("involuntary_ctx".to_string())
                    .or_default()
                    .push(v as f64);
            }
            if let Some(v) = s.block_in_bytes {
                scalar_samples
                    .entry("block_in_bytes".to_string())
                    .or_default()
                    .push(v as f64);
            }
            if let Some(v) = s.block_out_bytes {
                scalar_samples
                    .entry("block_out_bytes".to_string())
                    .or_default()
                    .push(v as f64);
            }
            if let Some(e) = &s.energy_j {
                if let Some(v) = e.pkg {
                    scalar_samples
                        .entry("energy_j.pkg".to_string())
                        .or_default()
                        .push(v);
                }
                if let Some(v) = e.dram {
                    scalar_samples
                        .entry("energy_j.dram".to_string())
                        .or_default()
                        .push(v);
                }
                if let Some(v) = e.gpu {
                    scalar_samples
                        .entry("energy_j.gpu".to_string())
                        .or_default()
                        .push(v);
                }
            }
            for (c_name, c_val) in &s.counters {
                scalar_samples
                    .entry(format!("counters.{c_name}"))
                    .or_default()
                    .push(c_val.value as f64);
            }
        }

        if let Some(phases) = &r.observations.phases {
            for (p_name, span) in phases {
                let dur_s = ((span.end_ns - span.start_ns) as f64) * 1e-9;
                scalar_samples
                    .entry(format!("phase.{p_name}"))
                    .or_default()
                    .push(dur_s);
            }
        }

        if let Some(regions) = &r.observations.regions {
            let mut per_rank_regions: BTreeMap<String, f64> = BTreeMap::new();
            for t in &regions.threads {
                accumulate_region_tree(&t.tree, "", &mut per_rank_regions);
            }
            for (reg_path, incl_s) in per_rank_regions {
                region_samples.entry(reg_path).or_default().push(incl_s);
            }
        }
    }

    if cpu_differs || mpi_differs || gpu_differs {
        warnings.push(Warning {
            code: WarningCode::TierDegraded,
            message: "ranks achieved different capability tiers; aggregated to minimum common tier"
                .to_string(),
        });
    }

    let scalars_agg: BTreeMap<String, RankAggregateStats> = scalar_samples
        .into_iter()
        .filter_map(|(k, vals)| compute_rank_stats(&vals).map(|st| (k, st)))
        .collect();

    let regions_agg: BTreeMap<String, RankAggregateStats> = region_samples
        .into_iter()
        .filter_map(|(k, vals)| compute_rank_stats(&vals).map(|st| (k, st)))
        .collect();

    let mut identity = RecordIdentity {
        rank: None,
        ranks: Some(rank_records.len() as u32),
        node: None,
        local_rank: None,
        status: agg_status,
        ..first.identity.clone()
    };
    identity.linked_runs = first.identity.linked_runs.clone();

    let environment = RecordEnvironment {
        placement: None,
        ..first.environment.clone()
    };

    let provenance = RecordProvenance {
        tiers_active: TiersActive {
            cpu: min_cpu,
            mpi: min_mpi,
            gpu: min_gpu,
        },
        warnings,
        errors,
        exit: ExitInfo {
            code: first.provenance.exit.code,
            signal: first.provenance.exit.signal.clone(),
        },
        ..first.provenance.clone()
    };

    Ok(Record {
        schema: Record::SCHEMA.to_string(),
        identity,
        environment,
        observations: RecordObservations::default(),
        provenance,
        aggregate: Some(RunLevelAggregate {
            rank_records: rank_paths,
            scalars: scalars_agg,
            regions: regions_agg,
            classes: BTreeMap::new(),
        }),
    })
}

fn min_opt_tier<T: Ord + Copy>(iter: impl Iterator<Item = Option<T>>) -> Option<T> {
    iter.reduce(|acc, item| match (acc, item) {
        (Some(a), Some(b)) => Some(a.min(b)),
        _ => None,
    })
    .flatten()
}

fn accumulate_region_tree(nodes: &[RegionNode], prefix: &str, out: &mut BTreeMap<String, f64>) {
    for n in nodes {
        let path = if prefix.is_empty() {
            n.name.clone()
        } else {
            format!("{prefix}/{}", n.name)
        };
        *out.entry(path.clone()).or_insert(0.0) += n.incl_s;
        accumulate_region_tree(&n.children, &path, out);
    }
}

fn compute_rank_stats(vals: &[f64]) -> Option<RankAggregateStats> {
    if vals.is_empty() {
        return None;
    }
    let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mean = vals.iter().sum::<f64>() / (vals.len() as f64);

    // Zero-mean guard: omit imbalance rather than emitting NaN or Inf (Task 15).
    let imbalance = if mean > 0.0 && mean.is_finite() {
        Some(max / mean)
    } else {
        None
    };

    Some(RankAggregateStats {
        min,
        mean,
        max,
        imbalance,
    })
}

// Keep CpuTier/MpiTier/GpuTier in scope for type-checking clarity
const _: (CpuTier, MpiTier, GpuTier) = (CpuTier::T0, MpiTier::M0, GpuTier::G0);
