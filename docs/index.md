# Calorimeter Documentation

**Calorimeter** (`calo`) is a reproducible benchmarking and profiling system for native CPU, multi-node MPI, and GPU programs. It discovers your hardware, expands declarative YAML suites into pinned runs, executes them locally or on Slurm clusters, records OS/PMU/GPU/MPI observations into immutable records and binary sidecars, and produces static reports, SVG plots, and regression gates.

:::{tip}
New to Calorimeter? Start with the [Quickstart](user-guide/quickstart.md) to run your first benchmark in 5 minutes, or jump straight to the [Minimal Working Examples](examples/mwe-local-t0.md) to test every tier hands-on.
:::

## Core Design Principles

- **Workload-agnostic core:** Inventory, expansion, execution, records, and reports never hardcode domain or solver logic.
- **Layered YAML everything:** Human-authored `inventory.yaml` and `suite.yaml` with includes, overlays, and `--set` overrides; machine-generated `manifest.yaml`, `runspec.yaml`, and `calo.record/2`.
- **Independent capability tiers:** CPU (`T0`–`T2`), MPI (`M0`–`M2`), and GPU (`G0`–`G2`) degrade gracefully without failing your workload.
- **No root required:** Denied kernel or GPU permissions emit structured `tier_degraded` warnings instead of crashing runs.
- **Zero Python required:** Built-in statistics, analysis services, and SVG plot renderers are implemented natively in Rust; out-of-process Python is strictly optional.

```{toctree}
:maxdepth: 2
:caption: User Guide

user-guide/installation
user-guide/quickstart
user-guide/concepts
user-guide/workflow-guide
user-guide/reporting-and-plots
user-guide/hpc-and-slurm
```

```{toctree}
:maxdepth: 2
:caption: Minimal Working Examples

examples/mwe-local-t0
examples/mwe-profile-t1
examples/mwe-channel-t2
examples/mwe-mpi-m0-m1
examples/cosmolattice-guide
```

```{toctree}
:maxdepth: 2
:caption: Reference

reference/cli
reference/schemas
reference/expressions-and-selectors
reference/services-and-warnings
```

```{toctree}
:maxdepth: 2
:caption: Developer Guide

developer-guide/architecture
developer-guide/probe-internals
developer-guide/extending
developer-guide/contributing
```
