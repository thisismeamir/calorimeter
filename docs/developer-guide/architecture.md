# Workspace Architecture & Invariants

Calorimeter is a 7-crate Rust workspace with a strict acyclic dependency graph:

```text
calo-schema  <---  calo-core  <---  calo-cli        (control plane)
     ^
     +-----------  calo-probe                       (Linux measurement binary + lib)
     +-----------  calo-shim-mpi                    (M1 PMPI cdylib + C wrapper)

calo-channel                                        (standalone workload instrumentation crate + C header)
calo-shim-gpu                                       (GPU tracing shim stub)
```

## Crate Responsibilities

| Crate | Kind | Unsafe Policy | Responsibility |
|---|---|---|---|
| `calo-schema` | lib | `#![forbid(unsafe_code)]` | Single source of truth for all serde/JSON-Schema types + isolated `calo_schema::yaml` wrapper over `serde_yaml_ng` |
| `calo-core` | lib | `#![forbid(unsafe_code)]` | Layered YAML/includes, expression engine, matrix expansion, selectors, `calo.table/1` reader, Parquet export, services, SVG plots, flame graphs, report pipeline, `LocalAdapter`/`SlurmAdapter`/`SshAdapter`, git worktrees & cached builds |
| `calo-cli` | bin (`calo`) | `#![forbid(unsafe_code)]` | Clap CLI, `calo lint` diagnostics, command orchestration (`anyhow` only here) |
| `calo-probe` | lib + bin (`probe`) | `unsafe` confined to `src/sys/*` & `src/gpu/ffi.rs` | Passive discovery, gated `fork`/`execvpe`, `perf_event_open` counters & T1 sampling, `epoll` observe loop, `CALO_CHANNEL` FIFO, NVML `dlopen` G0 sampling, streaming `calo.table/1` writer, atomic record emission |
| `calo-channel` | lib | 1 syscall (`SYS_gettid` on Linux) | Lightweight Rust & C (`include/calo_channel.h`) client for emitting `CALO_CHANNEL` events |
| `calo-shim-mpi` | cdylib + rlib | Confined to `extern "C"` + `clock_gettime` | Thread-safe M1 MPI call/peer/collective aggregator + portable `c/pmpi_wrap.c` |
| `calo-shim-gpu` | cdylib | — | Placeholder crate for future G1/G2 GPU tracing backends |

## Key Engineering Invariants

1. **No `calo-core` in `calo-probe` runtime dependencies:** `calo-probe` depends only on `calo-schema` (with `calo-core` in `[dev-dependencies]` for sidecar round-trip tests), keeping the `probe` binary lean and dynamically linked only against libc.
2. **Isolated YAML backend:** All `serde_yaml_ng` usage is routed through `calo_schema::yaml` so the YAML crate can be swapped in one place.
3. **Deterministic serialization:** `BTreeMap` is used throughout serialized types so YAML outputs and SHA-256 hashes are reproducible across runs.
4. **Testable subprocesses:** `calo-core` executes external commands (`git`, `sbatch`, `sinfo`, `ssh`, `probe`, `python3`) through the `CommandRunner` trait so unit tests mock them deterministically without spawning cluster jobs.
