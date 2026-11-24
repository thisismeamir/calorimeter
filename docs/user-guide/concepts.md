# Core Concepts

## Capability Tiers

Calorimeter organizes observability into independent tiers across three domains. Each tier works without higher tiers in its domain, and unavailable capabilities degrade cleanly with a `tier_degraded` warning.

| Domain | Tier | Workload Requirement | Observations Provided |
|---|---|---|---|
| **CPU** | `T0` | None | Wall/user/sys time, `rusage`, PMU counters, host time series (`RSS`, CPU%, RAPL energy) |
| | `T1` | Debug symbols (`-g`), frame pointers (`-fno-omit-frame-pointer`) | Sampled stacks (`profile.{samples,chains,maps}.calotbl`), hotspots, flame graphs |
| | `T2` | `CALO_CHANNEL` markers | Exact phase timestamps, nested per-thread region trees, reported metrics & tags |
| **MPI** | `M0` | None (`--rank-aware`) | Rank/size/local-rank from launcher env, per-rank records (`rank<k>.yaml`), rank aggregation (`run.yaml`) |
| | `M1` | `libcalo_pmpi.so` (`LD_PRELOAD`) | Per-(function, comm) counts, min/max/total time, bytes, size histograms, peer matrix, collectives |
| | `M2` | PMPI shim | Bounded per-rank event trace |
| **GPU** | `G0` | None (`dlopen` NVML) | Device inventory, utilization, VRAM, power (W) & integrated energy (J), SM/mem clocks, temperature, throttling |
| | `G1` | Vendor tracing runtime | Kernel/memcpy timelines, host correlation |
| | `G2` | Counter replay permission | Per-kernel roofline hardware counters |

## Execution Modes (`measure` vs `profile` vs `kernel`)

Per Spec §8.5 R-BEN-3, Calorimeter **never mixes** timing runs with intrusive profiling:

- **`measure`**: Clean headline timing (`T0`, `M0`, `G0` only). `calo` rejects `probe.profile`, `probe.mpi: summary|events`, and `probe.gpu.trace: activity` in `measure` mode.
- **`profile`**: Enables CPU stack sampling (`T1`), PMPI aggregation (`M1`), and/or GPU activity tracing (`G1`). Requires a build with `debug_info: required` (verified by `calo lint` and enforced at runtime via ELF symbol inspection, `R-BLD-3`).
- **`kernel`**: Short-problem GPU counter replay (`G2`); never used for headline wall-time statistics.

You can derive a `profile` benchmark directly from a `measure` benchmark using `from:` and `where:` so both stay linked in `manifest.yaml`.

## Reproducibility & File Layout

Every `calo run` creates a content-addressed directory `results/<suite-hash>/`:

```text
results/<suite-hash>/
├── manifest.yaml              # Full resolved config with origin layers + source SHAs + run list
├── records/<run-id>/
│   ├── runspec.yaml           # Exact calo.runspec/1 fed to `probe`
│   ├── inputs/                # Rendered template files (with SHA-256)
│   ├── rank0.yaml             # Immutable raw per-rank calo.record/2
│   ├── run.yaml               # Control-plane aggregated & stamped record
│   ├── series.host.calotbl    # Binary columnar host time-series sidecar
│   ├── gpu.state.dev0.calotbl # Binary G0 GPU state sidecar (when GPU G0 active)
│   └── profile.*.calotbl      # Binary T1 profile sidecars (when mode: profile)
├── analysis/                  # Derived service outputs & Parquet exports
└── reports/<report-name>/     # report.md, report.html, table_*.csv, plot_*.svg
```

Raw `rank<k>.yaml` files are immutable. All control-plane aggregation, identity stamping, services, and reports write to `run.yaml`, `analysis/`, and `reports/`.
