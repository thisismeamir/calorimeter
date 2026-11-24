# Services & Warning/Error Codes

## Built-In Services (`calo-core::services`)

Services transform a typed columnar `Frame` (`inputs -> produces`) in topological dependency order (`calo lint` rejects cycles and unknown names):

- **Statistics:** `stats.summary` (median/mean, seeded bootstrap CI, MAD outlier filter), `compare` (effect size & significance test between two record sets)
- **Derived:** `ipc`, `cache_miss_rate`, `speedup`, `parallel_efficiency`, `scaling_fit`
- **MPI:** `comm_fraction`, `imbalance`, `comm_matrix`, `message_size_profile`
- **GPU:** `gpu_utilization`, `kernel_hotspots`, `transfer_summary`, `launch_overhead`
- **Custom expression services:** defined directly in `suite.yaml` under `services: <name>: { expr: "...", unit: "..." }`
- **Custom Python services:** defined via `services: <name>: { python: "pkg.mod:fn", inputs: [...], produces: [...] }`

## Normative Warning & Error Codes (Spec §17)

Structured `{code, message}` entries appear in `provenance.warnings` and `provenance.errors`:

| Code | Meaning |
|---|---|
| `hardware_mismatch` | Observed hardware differs from declared `inventory.yaml` |
| `policy_mismatch` | Actual CPU governor, turbo, or GPU clocks differ from requested `policy:` |
| `tier_degraded` | A requested tier or counter was unavailable (e.g. `perf_event_paranoid` or missing NVML) |
| `no_symbols` | Profile-mode executable lacks `.symtab` and `.debug_info` (R-BLD-3) |
| `sample_loss` | Perf ring-buffer sample loss exceeded `profile.max_loss` |
| `trace_overflow` | Ring buffer or sidecar writer queue overflowed |
| `mux_detected` | PMU counter multiplexing occurred (`time_running_ns < time_enabled_ns`) |
| `oversubscribed_gpu` | Multiple ranks share a GPU without MPS |
| `colocated_jobs` | Unrelated co-located jobs detected on the node |
| `concurrency` | Multiple runs overlapped on the same node set |
| `shim_disabled` | A shim encountered an internal error and disabled itself safely (R-SAF-4) |
| `clock_unsynced` | Cross-node clock alignment was unavailable |
| `build_nondeterministic` | Artifact SHA-256 hash changed across rebuilds |
| `python_unavailable` | An optional external Python service could not execute |
