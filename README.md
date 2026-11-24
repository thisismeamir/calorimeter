# Calorimeter (`calo`)

[[https://github.com/thisismeamir/calorimeter/actions/workflows/ci.yml/badge.svg]](https://github.com/thisismeamir/calorimeter/actions/workflows/ci.yml)
[[https://img.shields.io/github/v/release/thisismeamir/calorimeter?color=7017FF]](https://github.com/thisismeamir/calorimeter/releases/latest)
[[https://readthedocs.org/projects/calorimeter/badge/?version=latest]](https://calorimeter.readthedocs.io/en/latest/)
[[https://img.shields.io/badge/rust-stable-black?logo=rust]](https://www.rust-lang.org/)
[[https://img.shields.io/github/license/thisismeamir/calorimeter]](LICENSE)

**Calorimeter** is a reproducible benchmarking and profiling system for native CPU, multi-node MPI, and GPU programs. It discovers your hardware, expands declarative YAML suites into pinned runs, executes them locally or on Slurm clusters, records OS/PMU/GPU/MPI observations into immutable records and binary sidecars, and produces static reports, SVG plots, and regression gates.

---

## Why Calorimeter?

- **Hardware-aware & reproducible:** Passive discovery (`calo spec`) fingerprints your CPU, memory/NUMA, perf counters, and NVIDIA GPUs; `manifest.yaml` pins every config layer, source commit SHA, patch tree, and binary hash.
- **Independent capability tiers:**
  - **CPU (`T0`–`T2`):** wall/CPU time, `rusage`, `perf_event_open` counters, host time series, FP/DWARF stack sampling (`T1`), and `CALO_CHANNEL` phases/regions/metrics (`T2`).
  - **MPI (`M0`–`M1`):** `--rank-aware` multi-rank isolation (`M0`), cross-rank aggregation (`min`/`mean`/`max`/`imbalance`), and a portable `LD_PRELOAD` PMPI shim (`M1`).
  - **GPU (`G0`):** out-of-process NVML discovery, utilization, VRAM, clocks, temperature, throttling, and integrated energy (J) loaded dynamically via `dlopen`.
- **No root, no Python required:** Denied permissions degrade cleanly with structured `tier_degraded` warnings; built-in statistics, pivot tables, static SVG plots, HTML/Markdown reports, and regression gates are 100% native Rust.
- **Workload-agnostic:** Built with [CosmoLattice](examples/cosmolattice/) as the reference target, with zero solver-specific code in the core.

## Installation

### Prebuilt Binaries

Download the archive for your platform from the [latest GitHub Release](https://github.com/thisismeamir/calorimeter/releases/latest):

```bash
tar -xzf calorimeter-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
export PATH="$PWD/calorimeter-v0.1.0-x86_64-unknown-linux-gnu/bin:$PATH"
calo --version

```

> **Note:** `calo` (control plane, lint, plan, analyze, report, Slurm) runs on Linux, macOS, and Windows. `probe` (measurement binary) and `calo-shim-mpi` are Linux-only.

### Build from Source

```bash
git clone [https://github.com/thisismeamir/calorimeter.git](https://github.com/thisismeamir/calorimeter.git)
cd calorimeter
cargo build --release -p calo-cli -p calo-probe -p calo-shim-mpi

```

To build the optional M1 PMPI wrapper against your cluster's MPI installation (`mpicc`):

```bash
./crates/calo-shim-mpi/build_shim.sh release

```

## Quickstart

```bash
# 1. Discover local hardware and verify fingerprint
calo spec --connect local --out inventory.yaml
calo verify --inventory inventory.yaml

# 2. Validate suite and inspect expanded runs & cost upper bounds
calo --suite suite.yaml --inventory inventory.yaml lint
calo --suite suite.yaml --inventory inventory.yaml plan

# 3. Execute (resumable by default) and check status
calo --suite suite.yaml --inventory inventory.yaml --results results run
calo --results results status

# 4. Generate Markdown/HTML/CSV reports, SVG plots, and check regression gates
calo --suite suite.yaml --inventory inventory.yaml --results results report

```

See [`examples/cosmolattice/`](https://www.google.com/search?q=examples/cosmolattice/) for a complete HPC scaling, patching, and profiling suite.

## Workspace Layout

| Crate | Role |
| --- | --- |
| `calo-schema` | Shared serde & JSON-Schema types (`calo.inventory/1`, `calo.suite/1`, `calo.runspec/1`, `calo.record/2`, `calo.table/1`) |
| `calo-core` | Layered YAML/includes, expression engine, matrix expansion, services, native SVG plots, reports, gates, Slurm/local adapters, git worktrees |
| `calo-cli` | `calo` CLI binary (`spec`, `verify`, `lint`, `plan`, `run`, `status`, `cancel`, `analyze`, `report`, `diff`) |
| `calo-probe` | Linux `probe` measurement binary (T0–T2, M0, G0 NVML via `dlopen`, passive discovery) |
| `calo-channel` | Standalone Rust crate & single-header C library (`calo_channel.h`) for `CALO_CHANNEL` instrumentation |
| `calo-shim-mpi` | M1 PMPI aggregation library + portable `mpicc` wrapper (`libcalo_pmpi.so`) |
| `calo-shim-gpu` | GPU tracing shim crate (G1/G2 backend hook) |

## Documentation

Full user guides, step-by-step tutorials, minimal working examples (T0, T1, T2, MPI, CosmoLattice), schema references, and developer internals are hosted on [Read the Docs](https://www.google.com/url?sa=E&source=gmail&q=https://calorimeter.readthedocs.io/en/latest/) (source in [`docs/`](https://www.google.com/search?q=docs/)).

## License

Licensed under the terms in [`LICENSE`](https://www.google.com/search?q=LICENSE).

```

### Commit & Push

```bash
# Optional: preserve the original spec before replacing [invalid URL]
cp [invalid URL] docs/SPEC.md

# After replacing [invalid URL] with the above:
git add [invalid URL] docs/SPEC.md
git commit -m "[docs] add concise project README with badges and archive v0.1 spec"
git push origin main
```
