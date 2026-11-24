# Installation & Prerequisites

Calorimeter ships as two binaries—**`calo`** (the cross-platform control plane) and **`probe`** (the Linux measurement binary)—plus optional shims (`libcalo_shim_mpi.so` / `libcalo_pmpi.so`) and the C instrumentation header (`calo_channel.h`).

## Platform Support

| Component | Linux (`x86_64`, `aarch64`) | macOS (`arm64`, `x86_64`) | Windows (`x64`, `arm64`) |
|---|:---:|:---:|:---:|
| `calo` CLI (lint, plan, analyze, report, diff, Slurm) | Yes | Yes | Yes |
| `probe` measurement binary (T0–T2, M0, G0) | Yes | — | — |
| `calo-shim-mpi` (M1 PMPI shim) | Yes | — | — |

## Option 1: Prebuilt Release Archives

Download the archive for your platform from the GitHub Releases page and add `bin/` to your `PATH`:

```bash
tar -xzf calorimeter-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
export PATH="$PWD/calorimeter-v0.1.0-x86_64-unknown-linux-gnu/bin:$PATH"
calo --version
probe --version
```

## Option 2: Build from Source

Requires Rust `stable` (edition 2021).

```bash
git clone https://github.com/thisismeamir/calorimeter.git
cd calorimeter

# Build control plane, probe, and MPI shim core
cargo build --release -p calo-cli -p calo-probe -p calo-shim-mpi

# Optional: enable Parquet sidecar export in calo-core
cargo build --release -p calo-cli --features calo-core/parquet
```

The binaries are placed in `target/release/calo` and `target/release/probe`. When `probe` sits next to `calo` in the same directory, `calo` locates it automatically; otherwise pass `--probe /path/to/probe` or set `CALO_PROBE=/path/to/probe`.

## Linux Measurement Permissions (Optional)

Calorimeter never requires `root`. However, Linux restricts hardware performance counters (`perf_event_open`) via `/proc/sys/kernel/perf_event_paranoid`:

| `perf_event_paranoid` value | What works |
|---|---|
| `<= 1` (or `CAP_PERFMON`) | Full hardware PMU counters (`cycles`, `instructions`, `cache-misses`) + T1 stack sampling |
| `2` | User-space PMU counters (`exclude_kernel = 1`) and software events (`task-clock`) |
| `>= 3` | Hardware counters denied; `probe` degrades cleanly to T0 timing + `rusage` + `tier_degraded` warning |

To check your current setting:

```bash
cat /proc/sys/kernel/perf_event_paranoid
```

## Optional: Building the M1 PMPI Shim

If you want per-call MPI statistics (`M1`) on an OpenMPI or MPICH cluster, compile the thin C wrapper against your cluster's `mpicc`:

```bash
./crates/calo-shim-mpi/build_shim.sh release
# Produces target/release/libcalo_pmpi.so linked against libcalo_shim_mpi.so
```

## Optional: NVIDIA GPU G0 Support

No CUDA toolkit or compile-time flag is needed for G0 GPU discovery and state sampling. `probe` dynamically loads `libnvidia-ml.so.1` via `dlopen` whenever an NVIDIA driver is present, and degrades cleanly on CPU-only machines.
