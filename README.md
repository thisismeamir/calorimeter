# Calorimeter Specification v0.1

> The Specification is tailored by Anthropic Claude, Until V1.0 this would be the case for bootstrapping purposes.


Keywords MUST, SHOULD, MAY follow RFC 2119. Sections marked *(informative)* are non-normative.

## 0. Conventions

### 0.1 YAML profile

- YAML 1.2, core schema, UTF-8, LF line endings.
- Generated files (`probe` and `calo` output) contain no anchors, aliases, merge keys, custom tags, or multi-document streams. They quote every string that could parse as another scalar (`"no"`, `"1e3"`, `"0755"`, `"null"`) and emit keys in schema-defined order, so files diff cleanly.
- User-authored `suite.yaml` and `inventory.yaml` MAY use anchors and merge keys. `calo` expands them before resolution, and the expanded form is stored in `manifest.yaml`.
- The Rust implementation MUST use a maintained YAML crate (not the archived `serde_yaml`) behind a single internal module, so it can be swapped.

### 0.2 Schemas

Every file type has a schema written in YAML as JSON Schema 2020-12, stored in `schema/*.schema.yaml`. `calo lint` validates against them. Typed Rust structures in `calo-schema` are the executable form of the same contract. Each file's first key declares its type and major version:

```yaml
schema: calo.inventory/1   # calo.inventory/1 | calo.suite/1 | calo.runspec/1 | calo.record/2
                           # calo.discovery/1 | calo.manifest/1 | calo.table/1
```

A mismatch is a typed error, never a panic.

### 0.3 Units and naming

- Raw data uses SI base units: seconds (`_s`, f64), bytes (`_bytes`), nanoseconds for device and trace timestamps (`_ns`, i64), `_hz`, `_w`, `_j`. Hardware descriptors may use `_kb`, `_mb`, `_gb`, `_mhz`, `_gbps`, `_gbs`, where the unit is in the name.
- A field name states its unit. Display units belong to reports.
- Unmeasured values are omitted, never `0` or `null`. A measured zero is written as zero.
- Host wall-clock instants are ISO 8601 with offset. Monotonic timestamps are int64 nanoseconds (§9.7).

## 1. Scope

Calorimeter is a benchmarking and profiling system for native programs on CPU, multi-node MPI, and GPU systems. It discovers machines, expands a suite into pinned runs, executes them, records what the OS, interconnect, and devices report, attributes cost to code, and produces reports.

**Goals**
- G1: Core tooling (inventory, matrix, execution, records, reports) never interprets the workload.
- G2: Record more than time: resources, counters, series, phases, workload-reported metrics.
- G3: Attribute cost to code (functions, call paths, regions) on host and device.
- G4: MPI is first-class: placement, communication cost, imbalance, scaling across nodes.
- G5: GPU is first-class: device state, kernel and transfer timelines, host-device attribution, counters. Vendor-neutral at the plugin level.
- G6: Machines are discovered, not only declared. Hardware facts carry provenance.
- G7: Every run is reproducible and traceable.
- G8: Every behavior is customizable through layered configuration and services.
- G9: No project-specific logic in the core (§19).
- G10: Everything a user reads or writes is YAML.
- G11: Every integration point supports all of its variants. Abstraction is preferred over choosing one (§9.3, §9.4, §9.8).

**Non-goals**: Python/JVM profiling, a web UI, a result database server, automatic tuning.

## 2. Capability tiers

Tiers are independent per domain. Each tier MUST work without the ones above it in its domain.

| Domain | Tier | Needs from workload | Provides |
|---|---|---|---|
| CPU | T0 | Nothing | Time, rusage, counters, series |
| | T1 | Symbols, frame pointers | Sampled stacks, hotspots, flame graphs |
| | T2 | Region markers | Exact region times, user metrics |
| MPI | M0 | Nothing | Placement, per-rank records, MPI time via stack classification |
| | M1 | PMPI shim loadable | Per-call counts, time, bytes, wait, peer matrix |
| | M2 | PMPI shim loadable | Event trace with aligned clocks (capped) |
| GPU | G0 | Nothing | Device inventory, utilization, memory, power, clocks, throttling |
| | G1 | Vendor tracing runtime | Kernel and memcpy timeline, launch latency, host correlation |
| | G2 | Counter access permission | Per-kernel counters via replay |

Reports declare `requires: [T1, M1, G1]`. `calo lint` rejects a report whose required tiers are not enabled for the selected benchmarks. `calo verify` reports the highest tier per domain per machine.

## 3. Architecture

The implementation is a single Rust workspace.

| Crate | Kind | Responsibility |
|---|---|---|
| `calo-schema` | lib | All shared serde types: runspec, discovery, record, manifest, table header, inventory, suite. Single source of truth. `#![forbid(unsafe_code)]`. |
| `calo-probe` | lib + bin `probe` | Discovery, launch, counters, sampling, channel, GPU state sampling, sidecar writer. Depends only on `calo-schema`. |
| `calo-shim-mpi` | cdylib | PMPI shim: aggregation, clock sync, optional trace. Built per MPI installation. |
| `calo-shim-gpu` | cdylib | GPU tracing backends behind one trait. |
| `calo-core` | lib | YAML loading, includes, layered configuration, expression language, selectors, expansion, symbolization, analysis, services, report tables. |
| `calo-cli` | bin `calo` | Command-line interface and orchestration. |

Dependency direction: `calo-schema` ← `calo-core` ← `calo-cli`; `calo-probe` ← `calo-schema`.

**Invariants**
- I1: The measurement path is `probe` and the optional shims only. `probe` is a dynamically linked binary against an old glibc baseline, with no runtime dependencies beyond libc. Vendor libraries (NVML, AMD SMI, Level Zero, CUPTI, rocprofiler) are loaded with `dlopen`, and their absence is never an error.
- I2: The control plane MUST NOT execute inside the measured process tree. Launcher affinity is disjoint from workload affinity when `isolate` is declared. `calo` always spawns `probe` as a separate process for real runs. The library form of `calo-probe` is for tests and tooling only.
- I3: After expansion, the executor sees only fully pinned benchmarks.
- I4: All resolved configuration is written to `manifest.yaml`.
- I5: `probe` writes the YAML record (scalars and metadata, small) and binary sidecars (bulk). Parquet conversion, symbolization, demangling, and clock alignment occur in the control plane, after the run.
- I6: Host timestamps use `CLOCK_MONOTONIC_RAW`. Device and cross-node timestamps are stored raw with the offset and drift needed to map them to a common timeline (§9.7).
- I7: `probe` never reads user YAML. It consumes one generated `calo.runspec/1` file and emits `calo.record/2` plus sidecars.
- I8: No `unsafe` outside the FFI and syscall layers of `calo-probe`, `calo-shim-mpi`, and `calo-shim-gpu`. Those layers are isolated in named modules with a documented safety contract per function.
- I9: Python is never required. It is an optional, out-of-process extension for user services and plotting (§12.4).

## 4. Project layout

```
calorimeter/                 repository
  Cargo.toml                 workspace
  crates/ ...                see §3
  schema/                    *.schema.yaml
  docs/                      this specification
  examples/cosmolattice/     reference suite (§19)

user project/
  inventory.yaml             generated by `calo spec`, then curated
  suite.yaml
  patches/                   *.patch, patchset.yaml
  templates/                 input-file templates
  actions/                   reusable step fragments
  context/                   read-only semantic documents
  overlays/                  per-environment overrides
  results/<suite-hash>/
    manifest.yaml
    records/<run-id>/rank<k>.yaml, run.yaml
    series/<run-id>/         host and device series (binary)
    profiles/<run-id>/       raw stacks, maps, symbol cache
    mpi/<run-id>/            shim summaries, optional traces
    gpu/<run-id>/            kernel tables, timelines, counter data
    analysis/                derived tables from services
    reports/
```

## 5. Configuration

### 5.1 Resolution and includes

Lowest to highest precedence: built-in defaults, `inventory.yaml`, `suite.yaml`, overlay, benchmark-level, CLI `--set key=value` (dotted paths, e.g. `benchmarks.0.repeat=3`). The resolved result is written to `manifest.yaml` with every value's origin layer.

`include:` takes a list of paths relative to the including file, merged in order. Maps merge recursively and lists replace. Include cycles are errors.

```yaml
include: [reports.yaml, services/cosmo.yaml]
```

### 5.2 Expression language

Used in `${{ ... }}`, `derive`, `expr`, `fail_if`, and selectors. It is a safe, side-effect-free subset implemented natively. Host code is never evaluated.

- Literals: numbers, strings, booleans, lists.
- Operators: `+ - * / // % **`, comparisons, `and or not`, `in`, conditional `a if cond else b`.
- Semantics: `/` is float division, `//` floor division. `**` with integer operands and a non-negative exponent stays integer. Division by zero is an error.
- References: `params.<k>`, `machine.<path>`, `node.<path>`, `env.<K>`, `files.<name>`, `run.<field>`, `phase.<name>` (seconds), `region.<name>.incl_s`, and metric names.
- Functions: `min max abs sqrt cbrt log log2 log10 exp floor ceil round int float len sum`.
- Unresolved references are a lint error when statically known and a run error otherwise. Errors carry a byte span.
- Template rendering for input files uses `{{ var }}` substitution only, with no logic blocks. Missing variables are errors (R-STP-8).

### 5.3 Selectors

```yaml
select:
  benchmark: scaling            # name or glob
  group: scaling
  params: {patch: fast-fft}     # equality on params
  where: "params.N >= 128 and machine.node.gpu.per_node >= 4"
  mode: measure
  tier: [T1, M1]
```

Keys within one selector intersect. A list of selectors is a union. `all` selects everything.

## 6. Machine discovery: `calo spec`

`inventory.yaml` is an observed artifact first and a curated file second. Discovery produces *observed* hardware, the same data R-INV-2 compares against at run time.

### 6.1 Command

```
calo spec --connect local|ssh://[user@]host|slurm://login
          [--machine NAME] [--out inventory.yaml] [--merge]
          [--list-partitions]
          [--partition NAME ...] [--sample N] [--account A] [--yes]
          [--deep] [--no-calibrate]
```

### 6.2 Partition handling (normative)

- R-SPC-1: For `slurm://`, Calorimeter MUST NOT submit any job to a partition the user has not named with `--partition` (or a `partitions:` entry in an existing inventory).
- R-SPC-2: `--list-partitions` is passive and read-only. It runs `sinfo`, `scontrol show partition`, and `scontrol show node` on the login node and prints suggestions only: partition name, node count, node-type groups (by CPU, memory, features, GRES), time limits, default flag, accessible accounts and QOS. Nothing is written. Nothing is submitted.
- R-SPC-3: The output includes a ready-to-run command, e.g. `calo spec --connect slurm://login --partition gpu --partition cpu`.
- R-SPC-4: For each named partition, discovery submits one short job per node class (default `--sample 1`), requesting the minimum allowed time limit, with the default account unless `--account` is given. The job runs `probe --discover` and exits. `calo spec` prints the exact job parameters and requires `--yes` or interactive confirmation on first use per partition.
- R-SPC-5: Login nodes are never the source of node hardware facts. They provide only scheduler metadata.
- R-SPC-6: With `--sample N > 1`, discovery runs on N nodes per class and warns when fingerprints differ within a class.

### 6.3 Discovery contract (`probe --discover`)

- B-D1: Outputs `calo.discovery/1` YAML, runs no workload, MUST NOT require root, MUST degrade per field, and MUST complete in under 10 seconds in passive mode.
- B-D2: Read-only. It MUST NOT change system state: no governor changes, no module loads (module queries are read-only), no jobs other than those in R-SPC-4.
- B-D3: Each field carries `source` and `confidence` (`measured`, `queried`, `inferred`, `unavailable`). Inference is never presented as fact.
- B-D4: Unresolvable fields are omitted and listed under `unresolved` with a suggested fix.

| Domain | Sources | Output |
|---|---|---|
| CPU | `/proc/cpuinfo`, `lscpu`, CPUID, sysfs `cpu/*/topology` and `cache` | model, microarchitecture, sockets, cores, threads, SMT, ISA (AVX2/512, AMX, SVE), cache sizes and sharing |
| Memory/NUMA | `/sys/devices/system/node`, distances, DMI where readable | NUMA nodes, per-node memory, distance matrix, channels (best effort), hugepage and THP state |
| Policy state | cpufreq sysfs, `intel_pstate`/`amd_pstate`, `perf_event_paranoid`, cgroup limits, SMT control | governor, turbo, paranoid level, cpuset constraints |
| Counters | test-open each alias via `perf_event_open` | working aliases, multiplexing limits, whether sampling and call chains work |
| GPU | NVML / AMD SMI / Level Zero via `dlopen` | vendor, model, architecture, SM/CU count, memory, clocks, driver and runtime versions, ECC, NUMA affinity, link topology, profiling permission |
| Network | `/sys/class/infiniband`, `/sys/class/net`, libfabric `fi_info` if present | NIC type, rate, port state, NIC-to-NUMA affinity |
| MPI | `ompi_info`, `mpichversion`, `srun --mpi=list` | library, version, launcher, CUDA/ROCm-awareness, thread levels |
| Toolchain | `PATH`, Lmod/Environment Modules queries | compilers, CMake, CUDA/ROCm toolkits, module names providing them |
| Scheduler | `sinfo`, `scontrol`, `sacctmgr` | partitions, limits, GRES, features, accounts |
| Storage | `/proc/mounts`, `df` | candidate `workdir` locations, filesystem type, free space |
| Identity | hostname, kernel, distribution | `fingerprint` |

**Fingerprint.** SHA-256 over a canonical YAML serialization of the stable discovered fields: CPU model/microarchitecture/topology, memory size and NUMA layout, GPU model/architecture/count/driver, NIC type and rate, kernel version, MPI library and version. Volatile state (governor, clocks, free space) is excluded.

### 6.4 Ownership and merge rules

1. Keys are *discovered* (hardware facts), *user* (`tags`, `workdir`, `connect`, `modules`, `policy`, `reference`, `launcher` overrides), or *derived* (`fingerprint`).
2. `--merge` overwrites discovered keys, never touches user keys, and adds new discovered keys. A hand-written value for a discovered key is kept, marked `override: true`, and warned about when it differs from the observed value.
3. A changed `fingerprint` is reported as a diff on `calo spec` or `calo verify`. Stored `reference` values bound to the old fingerprint are marked `stale: true`.
4. Discovery never writes secrets. SSH user and host come from the invocation.

### 6.5 Node classes and calibration

`--partition` discovery groups nodes by fingerprint and emits one **node class** per group. Benchmarks select a class with `machine: hpc-a/gpu-a100`.

`--deep` runs `calo calibrate`: CPU and GPU memory bandwidth, peak FLOPs, host-device bandwidth, MPI point-to-point latency and bandwidth between node pairs, collective latency. Results go into `reference` with the `probe` version and fingerprint. `--no-calibrate` is the default without `--deep`.

## 7. `inventory.yaml`

```yaml
schema: calo.inventory/1
machines:
  <name>:
    connect:
      type: local | ssh | slurm
      host: string
      user: string
      partitions: [string]
      account: string
    fingerprint: hex                     # derived
    tags: [string]
    node_classes:
      <class>:
        count: int
        partition: string
        features: [string]
        node:
          cpu: {model, microarch, sockets, cores, threads, smt, isa: [string]}
          cache: {l2_kb, l3_mb}
          memory: {gb, channels, numa_nodes}
          gpu: {vendor, model, arch, per_node, mem_gb, link: nvlink|pcie|xgmi|none}
    network: {type: infiniband|slingshot|ethernet, bw_gbps, topology}
    reference:
      cpu: {mem_bw_gbs, peak_gflops}
      gpu: {mem_bw_gbs, peak_gflops_fp64, peak_gflops_fp32}
      mpi: {p2p_latency_us, p2p_bw_gbs, allreduce_latency_us}
      host_device_bw_gbs: float
      measured: {probe: string, fingerprint: hex, at: timestamp, stale: bool}
    workdir: path
    modules: [string]
    env: {KEY: value}
    launcher:
      mpi: "srun -n {ranks} --ntasks-per-node {ranks_per_node}"
      bind: {cpu: core|numa|none, gpu: local_rank|explicit|none}
      gpu_aware_mpi: bool
    probe:
      counters: {<alias>: <vendor-event>}     # overrides
      power: rapl|nvml|rocm-smi|none
      max_sample_hz: int
    policy: {isolate: {cpu_list: "0-3"}, governor: performance, turbo: off, gpu_clocks: locked|default}
    discovered: {at, calo, fields: {<path>: {source, confidence}}, unresolved: [{field, fix}]}
```

A single-node machine MAY omit `node_classes` and put `node:` directly under the machine.

- R-INV-1: Declared and discovered hardware is used for selection and tagging.
- R-INV-2: At run time `probe` MUST record observed hardware per node and per device. `calo` MUST emit `hardware_mismatch` per differing field. `strict_hardware: true` makes it an error.
- R-INV-3: `policy` values are requests. Actual governor, turbo, GPU clock state, and ECC mode MUST be recorded and differences warned (`policy_mismatch`).
- R-INV-4: Counter aliases (`cycles`, `instructions`, `cache-misses`, `branch-misses`, `l1d-misses`, `llc-misses`, `stalled-backend`) resolve through a built-in table, overridable per machine. GPU counter names resolve per vendor in the backend plugin.
- R-INV-5: `calo verify` MUST report: fingerprint diff against the inventory, `perf_event_paranoid`, MPI launcher test, shim injection test, GPU visibility per rank, GPU profiling permission, highest enabled tier per domain.
- R-INV-6: Adapter interface: `stage`, `submit`, `poll`, `fetch`, `cancel`. A new scheduler is one adapter.

## 8. `suite.yaml`

```yaml
schema: calo.suite/1
suite: {name, version, description}
context: [path]
sources: {...}
patches: {...}
builds: {...}
steps: {...}
benchmarks: [...]
services: {...}
symbols: {fold: [...], classes: [...]}
reports: [...]
options: {strict_hardware, order, on_failure, redact, ...}
```

### 8.1 `sources`

```yaml
sources:
  <name>: {git: url, rev: tag|branch|sha, submodules: bool}
```

R-SRC-1: `rev` MUST be resolved to a commit sha at suite start and recorded.

### 8.2 `patches`

```yaml
patches:
  dir: patches/
  sets:
    none: []
    regions: [calo-regions.patch]
    vec+regions: [avx512-kernels.patch, calo-regions.patch]
  apply: {method: git-apply, strip: 1, on_conflict: fail}   # fail | skip
```

- R-PAT-1: Patches are unified diffs.
- R-PAT-2: A set is an ordered list applied to a clean worktree. Disabling means not applying. Reverse-apply MUST NOT be used.
- R-PAT-3: Records store set name, per-patch content hash, and resulting tree hash.
- R-PAT-4: Optional `patchset.yaml` metadata, checked by `calo lint`:
  ```yaml
  fft-plan-cache.patch: {requires: [kosmic >= 0.3], conflicts_with: [fft-rewrite.patch], description: string}
  ```
- R-PAT-5: Initial conditions and input files are parameters, not patches.
- R-PAT-6: Instrumentation SHOULD be a patch set, so its overhead is an axis.

### 8.3 `builds`

```yaml
builds:
  release:
    run: |
      cmake -B build -DCMAKE_BUILD_TYPE=Release ${{ params.cmake_flags }}
      cmake --build build -j
    flags: "-O3 -DNDEBUG"
    backend: cuda | hip | sycl | none
    modules: [string]
  profile:
    run: "..."
    flags: "-O3 -g -fno-omit-frame-pointer"
    debug_info: required
    device_lineinfo: true
```

- R-BLD-1: Default `cache_key`: source shas, patch set, machine CPU and GPU architecture, toolchain, MPI library identity, build flags.
- R-BLD-2: `rebuild: cache | fresh`, default `cache`. Artifact hashes are recorded so nondeterministic builds are detectable (`build_nondeterministic`).
- R-BLD-3: Profile-mode benchmarks MUST name a build with `debug_info: required`. `probe` verifies host symbols and fails the run with `no_symbols` otherwise. The GPU shim verifies kernel names.
- R-BLD-4: GPU architecture targets (`sm_80`, `gfx90a`, ...) are part of the cache key and recorded.
- R-BLD-5: MPI library and version are recorded. Static MPI builds use link-time shim inclusion (§9.8).
- R-BLD-6: `modules` and toolchain default from discovery results.
- R-BLD-7: Build time MAY be measured (`measured: true` on the build) as a separate benchmark.

### 8.4 `steps`

```yaml
steps:
  run:
    run: ./build/model input=${{ files.input }}
    measured: true
    kind: native
    uses: ./actions/<fragment>
    resources:
      nodes: "${{ params.nodes }}"
      ranks_per_node: "${{ params.rpn }}"
      threads_per_rank: "${{ params.threads }}"
      gpus_per_rank: 1
    files:
      input: {template: templates/model.in, vars: {N: "${{ params.N }}"}}
    channel: {phases: [init, main], regions: [field_update, fft], exclude_from_rates: [init]}
    timeout: "02:00:00"
```

- R-STP-1: Steps are opaque shell with `${{ expr }}` interpolation.
- R-STP-2: Exactly one measured step per benchmark unless `measure: [steps]` is set.
- R-STP-3: Rendered input files and their hashes are stored with the record.
- R-STP-4: With `resources`, the launcher wraps `probe`: `<launcher> probe --rank-aware -- <cmd>`. Total ranks = nodes × ranks_per_node.
- R-STP-5: `threads_per_rank` sets `OMP_NUM_THREADS` and affinity, recorded with placement.
- R-STP-6: `gpus_per_rank` and `launcher.bind.gpu` set device visibility per rank. `probe` records the actual rank-to-device map and flags oversubscription (`oversubscribed_gpu`).
- R-STP-7: A step failing (nonzero exit, timeout) marks the run `failed` or `timeout`. Failed runs are recorded and excluded from statistics by default (`options.on_failure: record | stop`).
- R-STP-8: Template rendering uses `{{ var }}` substitution only. Missing variables are errors.

### 8.5 `benchmarks`

```yaml
benchmarks:
  - name: string
    group: string
    mode: measure | profile | kernel
    machine: name | name/class | selector
    build: name
    steps: [name]
    params: {k: v}
    axes: {k: [v, ...]}
    derive: {k: expr}
    defaults: {k: v}
    expand: all | defaults | none
    include: [pattern]
    exclude: [pattern]
    scaling: {kind: strong|weak, over: nodes|ranks|gpus, baseline: pattern}
    repeat: int
    warmup: int
    probe:
      counters: [alias]
      sample_hz: int
      profile: {freq_hz: 499, callgraph: fp|dwarf, stack_bytes: int, attribute: [alias], max_loss: 0.01}
      mpi: off | summary | events
      gpu: {state: true, sample_hz: 10, trace: off|activity, source: in_process|external, counters: [name], kernel_filter: regex, kernel_limit: int, launch_stacks: "1/N", trace_max_events: int}
      omp: off | ompt
    from: benchmark-name
    where: expr
    concurrency: int
    batch: run | group
```

**Expansion (normative)**
1. No `axes`: already pinned.
2. `base`: `all` is the Cartesian product, `defaults` is one run with each axis at its default, `none` is empty.
3. Add each `include`. A partial pattern fills unnamed axes per `expand` (`all`: spans values, `defaults`: uses defaults, `none`: error if required).
4. Remove every run matching any `exclude` (all pattern keys equal).
5. Evaluate `derive` per run in dependency order, using a topological sort over expression references. A cycle is an error (e.g. `N: "128 * cbrt(ranks)"`).
6. `from`/`where` derives a benchmark from another's expanded set, optionally with a different `mode`.
7. Run ID = `<benchmark>.<sorted param values>.r<repeat>`, deterministic. IDs longer than 120 characters are truncated with a hash suffix.

- R-BEN-1: `repeat` and `warmup` are not axes. Warmup runs are flagged and excluded from statistics by default.
- R-BEN-2: `calo plan` MUST print expanded runs, node-hours, and GPU-hours (upper bound from `timeout`) before anything runs.
- R-BEN-3: Modes are distinct runs and MUST NOT be mixed.
  - `measure`: no shims, no sampling. T0, G0, M0 only. Source of headline metrics.
  - `profile`: CPU sampling, MPI shim, GPU activity tracing.
  - `kernel`: GPU counter replay, short problems, never used for headline timing.
- R-BEN-4: A profile or kernel benchmark MAY derive from a measure benchmark. The pair is linked in the manifest.
- R-BEN-5: `scaling` fixes the efficiency definition. Strong: `T(base)·P(base) / (T·P)`. Weak: `T(base) / T`. Baseline is a pattern selecting the reference run.

## 9. `probe` and shim contract

### 9.0 Rust interface

```rust
pub struct Probe;

impl Probe {
    /// Passive hardware discovery. No workload. (§6.3)
    pub fn discover(opts: &DiscoverOpts) -> Result<Discovery, ProbeError>;

    /// Execute one runspec as one rank; write record and sidecars.
    pub fn run(spec: &RunSpec) -> Result<RunOutcome, ProbeError>;
}

pub struct RunOutcome {
    pub record_path: PathBuf,
    pub exit: ExitStatus,        // the workload's, not probe's
    pub tiers: TiersActive,
}
```

- R-API-1: `ProbeError` covers only probe's own failures. A workload that exits nonzero still yields `Ok(RunOutcome)` with `status: failed`.
- R-API-2: A denied capability never returns `Err`. It degrades a tier and adds a `tier_degraded` warning.
- R-API-3: The `probe` binary is a thin wrapper: `probe --runspec <file>` runs `Probe::run`; `probe --discover [--out file]` runs `Probe::discover`; `probe --version` prints the version.

### 9.1 `Probe::run` state machine (normative)

1. **Prepare:** validate the runspec, create the output directory, create the channel FIFO, resolve counter aliases against discovery data.
2. **Spawn:** `fork`. The child applies affinity, NUMA policy, rlimits, and environment, then blocks reading a "go" pipe.
3. **Arm:** the parent opens counters and sampling events on the child's pid (`inherit`, `enable_on_exec`), maps ring buffers, and starts GPU state sampling. Then it writes to the go pipe.
4. **Exec:** the child `exec`s. Counters enable at exec, so no pre-exec setup is counted.
5. **Observe:** event loop until the child exits (§9.2).
6. **Reap:** `wait4` for rusage, a final counter read, a final ring buffer drain, channel closed.
7. **Finalize:** write sidecars, then write the record atomically (temp file and rename, R-REC-9).

### 9.2 Observe loop (normative)

- B-O1: A single-threaded `epoll` loop over the perf ring-buffer fds, the channel FIFO, a `timerfd` for series sampling, and a `pidfd` for child exit. All events are timestamped with `CLOCK_MONOTONIC_RAW` at read.
- B-O2: Sidecar writes are offloaded to a dedicated writer thread through a bounded queue, so the observe loop never blocks on disk. Queue overflow is counted and reported (`trace_overflow`).
- B-O3: Ring-buffer drain lag MUST be bounded: if the loop falls behind such that the kernel reports lost samples, the loss is counted (`sample_loss`).

### 9.3 CPU (T0-T2)

- B1: Apply affinity, NUMA binding, rlimits, environment, then `exec`. Counters open via `perf_event_open` before exec with `inherit`.
- B2: End-of-run scalars via `wait4` rusage and cgroup counters: wall, user/system CPU, peak RSS, page faults, context switches, block I/O.
- B3: Counter groups. Multiplexing MUST be detected, and values stored with `time_enabled_ns` and `time_running_ns` (`mux_detected`).
- B4: Time series at `sample_hz`: RSS, CPU utilization, power (RAPL).
- B5: Observed hardware and policy state recorded. Shares code with `--discover`.
- B6: Workload exit code and signal propagated.
- B7: Profile mode: sampling with `PERF_SAMPLE_IP|TID|TIME|CALLCHAIN|CPU` at `freq_hz` (default 499) or on a named counter's overflow period.
- B8: Callgraph `fp` (default) or `dwarf` (stack copy, `stack_bytes`), recorded. Both are supported.
- B9: Per-sample counter attribution via additional sampling events (`profile.attribute`).
- B10: Memory maps (`PERF_RECORD_MMAP2`) and build-ids recorded for symbolization.
- B11: Sample loss counted. Runs above `profile.max_loss` (default 1%) are flagged.
- B12: Raw output only. No symbolization in `probe`.
- B13: Channel via `CALO_CHANNEL`, timestamped on read with the same clock as samples.

### 9.4 Channel protocol (normative)

Line-oriented text, UTF-8, one event per line:

```
phase  <name> start|end
region <name> start|end            # nestable, per thread
metric <key>=<number> [unit]
tag    <key>=<string>
```

Library wrappers MUST prefix `tid=<n>` for region events. Unknown verbs are counted and ignored. The text protocol is the specification. Wrapper libraries (C header, Rust crate, Python, Kotlin) are conveniences over it.

For regions with very high event rates, `probe.regions: events | aggregate` selects per-event trees or in-probe aggregation (count, sum, min, max). Both are supported.

### 9.5 MPI (M0-M2)

- B14 (M0): Under `--rank-aware`, each rank's `probe` reads rank, size, local rank, and node from the environment (`OMPI_COMM_WORLD_*`, `PMI_*`, `SLURM_*`, configurable table). It records node name, CPU placement, NUMA domain, bound GPU, and NIC affinity. Outputs are suffixed `rank<k>`.
- B15: Each rank is measured independently. `probe` performs no cross-rank communication.
- B16 (M0): MPI time is estimated by classifying sampled frames when T1 is active.
- B17 (M1): The PMPI shim aggregates per (function, communicator): call count, total/min/max time, bytes sent/received, message-size histogram. Point-to-point records a sparse peer matrix. Nonblocking calls record post time and completion time separately.
- B18 (M1): Collectives log entry and exit timestamps per call instance, indexed by (communicator, sequence number).
- B19 (M2): Event trace in a bounded per-rank ring buffer written in chunks. Overflow counted. Cap set by `mpi.trace_max_events`. Aggregation-only operation is selectable.
- B20: Shim overhead MUST be self-measured at init and recorded.
- B21: `MPI_THREAD_MULTIPLE` MUST be supported. The provided thread level is recorded.
- B22: Sampled frames in the OpenMP runtime are classified `omp`. `omp: ompt` additionally records barrier and idle time per parallel region through the OMPT interface.
- B23: The shim is built per MPI installation. `MPI_Comm` and `MPI_Datatype` are pointers in some libraries and integers in others, so the shim is generated against the target `mpi.h` (via `bindgen` or a minimal C translation unit compiled by `mpicc`). Bulk logic lives in Rust behind a narrow C ABI.

### 9.6 GPU (G0-G2)

- B24 (G0, out-of-process in `probe`): device state via NVML, AMD SMI, or Level Zero sysman: utilization, memory, power, clocks, temperature, throttle reasons, ECC, link traffic counters where available. Series are per device with `owners` listing bound ranks.
- B25 (G1): Activity tracing via the vendor runtime (CUPTI Activity, rocprofiler, Level Zero tracing). Per kernel: name, stream, grid/block dimensions, registers, shared memory, device start and end. Per memory operation: kind (H2D, D2H, D2D, P2P, memset), bytes, duration. API calls: launch duration, synchronization wait. Two sources are supported, selected by `gpu.source`:
  - `in_process`: the shim loaded into the workload (default, enables region and host correlation).
  - `external`: `probe` wraps the vendor CLI profiler and imports its output into the same tables. Host correlation is coarser, and the record notes it.
- B26 (G1): Aggregation per kernel name (count, total/min/max, launch dimensions) plus a bounded timeline buffer. Overflow counted.
- B27 (G1): Every device activity carries a correlation ID linking it to the host launch. The shim records host thread and active `region` at launch. In profile mode a host stack is captured for a sampled fraction of launches.
- B28 (G2): Counter replay per kernel (DRAM read/write bytes, FP64/FP32 operations, occupancy, L2 hit rate, stall reasons) in `mode: kernel`, bounded by `kernel_filter` and `kernel_limit`.
- B29: Tracing overhead MUST be recorded.
- B30: Vendor backends are plugins behind one trait: `device_info`, `sample_state`, `trace_start`, `trace_stop`, `counters`. Backends for NVIDIA, AMD, and Intel are all in scope. A missing backend or denied permission degrades to the highest available tier with the reason recorded.
- B31: With GPU-aware MPI, transfers triggered inside MPI call intervals are classified (GPU-direct versus staged). Without it, the classification is skipped.

### 9.7 Resource accounting and clock alignment

- B32: The run-level aggregate lists nodes, ranks per node, devices, and bindings. Unrelated co-located jobs (when the scheduler exposes them) are recorded as a warning (`colocated_jobs`).
- B33: Cross-node alignment: the MPI shim estimates each rank's clock offset to rank 0 at `MPI_Init` and `MPI_Finalize` using repeated ping-pong with minimum-RTT selection, storing offset, drift, and RTT bound. Without the shim, cross-node timelines are not aligned and the record says so (`clock_unsynced`).
- B34: GPU timestamps are stored on the device clock together with host/device correlation samples at shim init, periodically, and at exit.

### 9.8 Shim injection

Both methods are supported and selected per machine or benchmark (`probe.shim_inject: preload | link | auto`):

- `preload`: `LD_PRELOAD` for MPI, and the vendor injection variable (e.g. `CUDA_INJECTION64_PATH`) for GPU.
- `link`: link-time inclusion through a build hook for statically linked binaries. The build step exposes `${{ calo.link_flags }}`.
- `auto`: `preload` unless the discovered binary is static.

R-SAF-4 applies to both: a shim failure never fails the workload.

### 9.9 Run input

```yaml
schema: calo.runspec/1
run_id: string
mode: measure | profile | kernel
command: [argv...]
cwd: path
env: {K: v}
affinity: {cpu_list: "4-67", numa: bind|interleave|none}
rlimits: {core: 0}
counters: [alias]
sample_hz: 10
profile: {...}
mpi: {mode: off|summary|events, trace_max_events: int}
gpu: {...}
shim_inject: preload|link|auto
regions: events|aggregate
channel: path
rank: {from_env: true}
outputs: {dir: path}
```

## 10. Run record: `calo.record/2`

One YAML file per rank (`rank<k>.yaml`) plus one run-level aggregate (`run.yaml`) produced by the control plane. Key order below is emission order.

```yaml
schema: calo.record/2
identity:
  run_id: string
  benchmark: string
  group: string                 # optional
  mode: measure | profile | kernel
  params: {k: scalar}
  repeat: int
  warmup: bool
  rank: int                     # absent in run.yaml
  ranks: int
  node: string
  local_rank: int
  suite_hash: hex
  linked_runs: [run_id]
  status: ok | failed | timeout | invalid

environment:
  machine: string
  node_class: string
  fingerprint: hex
  declared: {...}
  observed:
    cpu: {model, microarch, sockets, cores, threads, smt, isa: [..], cache: {l1d_kb, l2_kb, l3_mb}}
    memory: {total_bytes, numa_nodes, numa_distance: [[int]], thp: string, hugepages: int}
    kernel: string
    policy: {governor, turbo, smt, perf_event_paranoid, isolation, cgroup_cpus, cgroup_mem_bytes}
    counters_available: [alias]
    gpus: [{index, vendor, model, arch, sms, mem_bytes, driver, runtime, ecc, clocks: {sm_mhz, mem_mhz}, link: [..], numa}]
    nic: [{name, type, rate_gbps, numa}]
    mpi: {library, version, launcher, thread_level_provided}
  placement: {cpu_list, numa: [int], threads: int, gpus: [int], nic: string}
  devices: [{index, owners: [rank]}]
  revisions: {<source>: sha}
  patchset: {name, patches: [{file, sha256}], tree_hash: hex}
  build: {name, cache_key: hex, flags: string, backend: string, gpu_arch: [string], toolchain: string}
  artifact_hashes: {<path>: sha256}
  rendered_files: {<name>: {path, sha256}}
  mpi_lib: {name, version}
  modules: [string]

observations:                   # each block present iff its tier is in provenance.tiers_active
  scalars:
    wall_s: float
    user_cpu_s: float
    sys_cpu_s: float
    peak_rss_bytes: int
    minor_faults: int
    major_faults: int
    voluntary_ctx: int
    involuntary_ctx: int
    block_in_bytes: int
    block_out_bytes: int
    energy_j: {pkg: float, dram: float, gpu: float}
    counters:
      <alias>: {value: int, time_enabled_ns: int, time_running_ns: int, scaled: bool}
  phases: {<name>: {start_ns: int, end_ns: int}}
  regions:
    threads:
      - tid: int
        tree:
          - {name, calls: int, incl_s: float, excl_s: float, children: [...]}
  reported: {<key>: {value: float, unit: string}}
  tags: {<key>: string}
  series: {<name>: sidecar}
  profile:
    method: fp | dwarf
    freq_hz: int
    samples: int
    lost: int
    modules: [{path, build_id, base, size}]
    data: sidecar
  mpi:
    thread_level: string
    shim_overhead_ns_per_call: float
    functions: [{name, comm, calls, total_s, min_s, max_s, bytes_sent, bytes_recv, size_hist: [int]}]
    peers: sidecar
    collectives: sidecar
    trace: sidecar
    clock_sync: {offset_ns: int, drift_ppm: float, rtt_bound_ns: int, method: string}
  gpu:
    devices:
      - index: int
        owners: [rank]
        state_series: sidecar
        kernels: sidecar
        memcpy: sidecar
        timeline: sidecar
        counters: sidecar
        clock_correlation: sidecar
    source: in_process | external
    tracing_overhead_s: float

provenance:
  probe_version: string
  shim_versions: {mpi: string, gpu: string}
  counters: [alias]
  clock: CLOCK_MONOTONIC_RAW
  callgraph: fp | dwarf
  start: timestamp
  end: timestamp
  start_ns: int                 # monotonic anchor for all *_ns fields
  exit: {code: int, signal: string}
  sample_loss: float
  trace_overflow: {mpi: int, gpu: int, writer_queue: int}
  tiers_active: {cpu: T0|T1|T2, mpi: M0|M1|M2, gpu: G0|G1|G2}
  warnings: [{code, message}]
  errors: [{code, message}]
```

**Sidecar reference type**
```yaml
sidecar: {path: relative, format: calo.table/1, schema: string, rows: int, sha256: hex}
```

- R-REC-1: Required fields: `schema`, `identity.{run_id,benchmark,mode,status}`, `environment.{machine,observed}`, `provenance.{probe_version,tiers_active,start,end,exit}`.
- R-REC-2: Sidecars are binary (§11). The control plane converts them to Parquet in `analysis/` without modifying originals.
- R-REC-3: Profile data is a sidecar directory keyed by build-id for symbols.
- R-REC-4: Raw records are immutable. Services write derived fields to `analysis/`, never into records.
- R-REC-5: `scalars.counters` stores raw values with enabled and running times so scaling can be redone.
- R-REC-6: A measured zero is written as zero. An unmeasured value is omitted.
- R-REC-7: `run.yaml` aggregates per-rank records: min, mean, max, and imbalance (`max/mean`) for each scalar, region, and class, and lists all rank record paths.
- R-REC-8: `provenance.tiers_active` is authoritative. An observation block whose tier is not active MUST be absent, and a reader MUST reject a record that violates this.
- R-REC-9: `probe` writes the record atomically (temp file then rename), so a record's existence means the run completed.

## 11. Sidecar format: `calo.table/1`

A binary columnar file for bulk data.

```
magic        8 bytes   "CALOTBL1"
header_len   uint32 LE
header       YAML (calo.table/1 header), UTF-8
blocks       repeated: {rows uint32 LE, column data, crc32}
```

Header:
```yaml
schema: calo.table/1
table: gpu.kernels
columns:
  - {name: start_ns, type: int64}
  - {name: end_ns, type: int64}
  - {name: name_id, type: string_id}
  - {name: stream, type: int32}
strings: {name_id: [string, ...]}
meta: {device: 0, clock: device}
```

- Types: `int32 int64 float32 float64 uint8 string_id`. Little-endian. Columns within a block are contiguous.
- Blocks are written in chunks so traces stream to disk during the run. A crash loses at most one block. A reader returns all complete blocks and reports the truncated tail as a distinguishable condition, not a total failure.
- Each block carries a CRC32, verified on read.
- Defined tables: `series.host`, `series.gpu_state`, `profile.samples`, `profile.maps`, `mpi.peers`, `mpi.collectives`, `mpi.trace`, `gpu.kernels`, `gpu.memcpy`, `gpu.timeline`, `gpu.counters`, `gpu.clock_correlation`. Column lists are in `schema/tables.schema.yaml`.

## 12. Post-processing and services

### 12.1 Pipeline

1. **Collect** raw files via adapters.
2. **Symbolize** host addresses by build-id (`llvm-symbolizer` or `libdw`, both supported). GPU kernel names are demangled with the same machinery.
3. **Demangle and normalize**: `symbols.fold` rules collapse template arguments, lambdas, allocator noise, portability-layer wrappers.
4. **Classify** frames and kernels via `symbols.classes`.
5. **Align timelines**: apply clock offsets, drift, and device-to-host correlation.
6. **Match collectives** by (communicator, sequence number). Wait per rank = own duration − minimum duration over ranks. Late arrival = own entry − latest entry (needs aligned clocks).
7. **Aggregate** per run, rank, node, device. Rank statistics: min, mean, max, imbalance.
8. **Attribute GPU time to host** via correlation IDs, assigned to the host region and call site active at launch.

```yaml
symbols:
  fold:
    - {match: 'Kokkos::Impl::[^<]*<.*>', as: 'Kokkos::<impl>'}
    - {match: '\(anonymous namespace\)::', as: ''}
  classes:
    - {match: '^(P?MPI_)', class: mpi}
    - {match: 'fftw|cufft|rocfft', class: fft}
    - {match: '^(memcpy|memset|malloc|free)', class: memory}
```

Rules apply in order. The first matching `classes` rule wins. Built-in classes: `mpi`, `omp`, `memory`, `io`, and for GPU `compute`, `memcpy`, `sync`.

- R-PIPE-1: Stages 2-8 are services and MAY be reconfigured or replaced.
- R-PIPE-2: Symbolization failures are reported per module with the unresolved sample fraction. Unmatched collective instances are counted and reported.
- R-PIPE-3: Pipeline output goes to `analysis/` as sidecar tables keyed by `run_id`. Records are not modified.

### 12.2 Services

A service is a named transformation `inputs → produces` with options. Layers, lowest to highest: built-in, suite `services:` block, report-local override. Customization levels: configure, override, define.

```yaml
services:
  stats.summary:                       # configure
    estimator: median
    ci: {method: bootstrap, level: 0.95, resamples: 10000, seed: 1}
    outliers: {method: mad, threshold: 3.5}
  throughput:                          # define: expression
    expr: "reported.events / phase.main"
    unit: events/s
  sites_per_second:
    expr: "params.N**3 * reported.steps / phase.main"
    unit: sites/s
  my.fit:                              # define: external Python
    python: pkg.module:function
    inputs: [params.N, wall_s]
    produces: [alpha, r2]
```

- R-SVC-1: Built-in services are implemented in Rust in `calo-core` and operate on a minimal typed column frame.
- R-SVC-2: Services declare `inputs`, `produces`, and `requires` (tiers). `calo lint` builds the dependency graph and rejects cycles and unknown names.
- R-SVC-3: Outputs are written to `analysis/<service>` with `run_id` keys and the service options hash. Bootstrap seeds are recorded.
- R-SVC-4: Services run in dependency order and only on records that have the tiers they declare.
- R-SVC-5: Expression-defined services are evaluated per row with the expression language (§5.2).

### 12.3 Built-in library

| Group | Services |
|---|---|
| Statistics | `stats.summary` (median/mean, bootstrap CI, MAD outlier filter), `compare` (effect size, significance test between two record sets) |
| Derived | `ipc`, `cache_miss_rate`, `speedup`, `parallel_efficiency` (per `scaling`), `scaling_fit` |
| MPI | `comm_fraction`, `imbalance`, `collective_wait`, `comm_matrix`, `message_size_profile`, `overlap` |
| GPU | `gpu_utilization`, `kernel_hotspots`, `transfer_summary`, `launch_overhead`, `gpu_idle_gaps`, `host_device_overlap`, `occupancy` |
| Profile | `hotspots`, `region_breakdown`, `profile_diff` |
| Roofline | `boundness` (CPU and GPU, against `reference` bandwidth and peak, GPU needs G2) |
| Cross-domain | `time_budget`: wall time split into CPU compute, GPU compute, MPI wait, MPI transfer, host-device transfer, idle, from the linked profile run |

### 12.4 External Python (optional)

- R-PY-1: Services with a `python:` entry run out-of-process. `calo` writes the selected input columns to a table file, invokes the declared function through a small runner, and reads the declared `produces` columns back. Without Python installed, such services fail with `python_unavailable`, and everything else works.
- R-PY-2: Plot types MAY be implemented externally in Python under the same contract. Built-in plotting is specified in §13 and its renderer is implementation-defined.
- R-PY-3: Embedding Python in-process is not part of this version.

## 13. Reports

```yaml
reports:
  - name: string
    requires: [T1, M1, G1]
    select: all | selector | [selector]
    plots:
      - type: line
        x: params.N
        y: sites_per_second
        line_by: params.patch
        facet_by: machine
        scale: {x: log2, y: log}
        errorbars: ci95
        title: string
    tables:
      - {rows: params.patch, cols: params.N, value: speedup}
    gates:
      - {metric: wall_s.median, compare: {benchmark: baseline}, fail_if: "> +5%"}
    output: [md, html, pdf, csv, svg]
```

- R-REP-1: Reports reference axes, metrics, regions, classes, ranks, and devices by name only.
- R-REP-2: A report nested in a benchmark is shorthand for a top-level report with `select: {benchmark: <name>}`.
- R-REP-3: Built-in plot types: `line`, `bar`, `heatmap`, `roofline`, `timeline`, `flamegraph`, `diff_flamegraph`, `region_tree`, `rank_heatmap`, `comm_matrix`, `gpu_timeline`, `time_budget_bar`, `scaling_plot`.
- R-REP-4: Tables (CSV and Markdown) and gates are core. A failed gate makes `calo report` exit nonzero. Gates compare to a stored baseline (`--baseline <dir>`) or an in-suite benchmark. `fail_if` accepts `"> +5%"`, `"< -10%"`, or an expression.
- R-REP-5: `context` files are attached verbatim and never parsed for execution.
- R-REP-6: Output is static files. Flame graphs render as static SVG.
- R-REP-7: The plotting backend is not fixed by this specification. Plot types are services, so any renderer (native or external) can implement them.

## 14. CLI

```
calo spec [--list-partitions] [--partition ...]   discover machines, write inventory.yaml
calo lint                                          validate files against schemas and cross-references
calo plan [--format yaml]                          expanded runs, node-hours, GPU-hours
calo calibrate                                     measure reference values
calo verify                                        fingerprint diff, permissions, tiers per machine
calo run [--select ...] [--force]                  execute (resumable)
calo status | cancel
calo analyze                                       symbolize, align, aggregate, run services
calo report [--baseline dir]                       reports and gates
calo diff <dirA> <dirB>                            compare two result sets
```

Global flags: `--suite`, `--inventory`, `--set key=value`, `--results dir`, `--quiet`, `--verbose`.

`calo lint` checks: schema headers, unknown expression references, unknown machine/build/step/patch-set names, service dependency cycles, report `requires` against benchmark modes, profile-mode builds with `debug_info: required` (R-BLD-3), and exactly one measured step per benchmark (R-STP-2). Output is `file:line: message`.

`calo run` skips runs with a valid record unless `--force`. A record is valid when it exists, its schema version is supported, and its `suite_hash` and run parameters match.

## 15. Execution model

1. Resolve configuration, expand benchmarks, write `manifest.yaml`.
2. Per machine: stage `probe`, shims, and sources. Create worktrees. Build under cache policy for the declared CPU and GPU architectures.
3. Render input files, then schedule. Runs on a node set execute serially by default. Concurrency is flagged in records.
4. Order: `interleaved` (default), `random`, or `sequential`.
5. Slurm: one job per run or per group (`batch: run | group`), requesting nodes, tasks, CPUs, and GPUs from `resources`, launching `<launcher> probe --rank-aware -- <cmd>`. Exclusive allocation is the default for `measure` mode and is recorded.
6. Within a `batch: group` job, run order and placement are preserved. Node names are recorded per run so node-specific outliers can be found.
7. Scaling runs at different scales SHOULD be interleaved within comparable allocation windows.
8. Fetch raw outputs, then `analyze`, then `report`.

**Manifest**
```yaml
schema: calo.manifest/1
suite: {name, version, hash}
created: timestamp
calo: string
resolved: {...}                         # full resolved config with origin layer per key
inventory_snapshot: {...}
runs:
  - {run_id, benchmark, mode, machine, params, status, linked_runs: [..], record: path}
sources: {<name>: sha}
```

## 16. Reproducibility and safety

- R-SAF-1: Each record traces to suite hash, source shas, patch hashes, build and artifact hashes, rendered input hashes, MPI library version, driver and runtime versions, GPU architecture, machine fingerprint, observed hardware, probe and shim versions.
- R-SAF-2: `calo` MUST NOT modify the user's source checkout. Work happens in managed worktrees under `workdir`.
- R-SAF-3: `probe` MUST NOT require root. Denied capabilities degrade to the highest available tier, with the reason in `provenance.warnings`.
- R-SAF-4: A shim failure MUST NOT fail the workload. The shim disables itself and records the error (`shim_disabled`).
- R-SAF-5: Secrets are never written to manifests or inventories. Variables under `options.redact` are masked.
- R-SAF-6: Discovery is read-only and submits jobs only under R-SPC-1 to R-SPC-6.
- R-SAF-7: Generated YAML never executes anything on read. Shell appears only in `run:` fields of user-authored steps and builds.

## 17. Errors and exit codes

Warning and error codes appear in `provenance.warnings` and `provenance.errors` as `{code, message}`.

| Code | Meaning |
|---|---|
| `hardware_mismatch` | observed differs from declared |
| `policy_mismatch` | actual governor, turbo, or clocks differ from requested |
| `tier_degraded` | a requested tier was unavailable (reason in message) |
| `no_symbols` | profile build has no symbols |
| `sample_loss` | loss above `max_loss` |
| `trace_overflow` | a ring buffer or writer queue overflowed |
| `mux_detected` | counter multiplexing occurred |
| `oversubscribed_gpu` | ranks share a device without MPS |
| `colocated_jobs` | other jobs on the node |
| `concurrency` | runs overlapped on the node set |
| `shim_disabled` | a shim failed and disabled itself |
| `clock_unsynced` | cross-node alignment unavailable |
| `build_nondeterministic` | artifact hash differed across rebuilds |
| `python_unavailable` | an external Python service could not run |

`calo` exit codes: `0` success, `1` generic error, `2` lint failure, `3` run failures present, `4` gate failure, `5` discovery or permission failure.

## 18. Versioning and compatibility

- Each file type has `schema: <type>/<major>`. A major bump is a breaking change. Additive changes (new optional fields, new tables) do not bump the major.
- Readers MUST ignore unknown fields. Writers MUST NOT emit fields they did not define. Unknown-field rejection (`deny_unknown_fields`) MUST NOT be used on record, runspec, or discovery types.
- `calo` reads the current and previous major of `calo.record`. `calo analyze` runs on older result directories.
- `probe` and shims report their version in `provenance`. `calo` refuses to mix records from incompatible probe majors in one comparison unless `--allow-mixed`.

## 19. Reference target: CosmoLattice *(informative)*

The core contains no CosmoLattice logic. `examples/cosmolattice/` is a suite that expresses, as configuration only:

- `sources` and `builds` (`release`, `profile`; GPU backend and architecture per build).
- A run step with `resources` (nodes, ranks per node, threads per rank, GPUs per rank) and an input file rendered from a template.
- Axes: lattice `N`, nodes, ranks per node, threads, GPU on/off or backend, FFT backend, patch set.
- `derive` for weak scaling (`N` from ranks).
- `calo-regions.patch`: regions around field update, gradient/Laplacian, FFT, measurements/output, halo exchange.
- Services: `sites_per_second`, scaling efficiencies, `boundness`.
- `symbols.fold` and `symbols.classes` for template-heavy names, FFT backend symbols, MPI frames, device kernel names.
- Reports: `scaling_plot`, `time_budget_bar`, `gpu_timeline`, `comm_matrix`, `rank_heatmap`.

The GPU backend is set by `builds.backend` and the discovered `node.gpu.vendor`. No specification change is needed per vendor.

## 20. Implementation phases

| Phase | Content |
|---|---|
| 1 | `calo-schema` (all types, schema checks), YAML loading with includes and layers, expression language, selectors and expansion, `calo lint` and `calo plan`, sidecar reader |
| 2 | `calo-probe` T0: discovery, spawn/arm/exec handshake, rusage and counters, observe loop, channel, rank-aware placement (M0), G0 device state, sidecar writer, atomic record, `local` adapter, `calo spec` (local, passive), `calo run` (local), `stats.summary`, table reports |
| 3 | T1 sampling, symbolization pipeline, `hotspots`, `flamegraph`, build variants with symbol checks, MPI shim M1, `comm_fraction`, `imbalance`, `comm_matrix` |
| 4 | `slurm` and `ssh` adapters, `calo spec --list-partitions` and partition-scoped discovery with node classes, `calo verify`, patch sets |
| 5 | GPU shim G1 (all vendor backends behind the trait, in-process and external sources), kernel and transfer tables, host-device correlation, clock alignment, collective matching, `time_budget`, `gpu_timeline` |
| 6 | T2 channel and regions, `region_breakdown`, GPU counters G2 and GPU `roofline`, MPI event trace M2, OMPT, `profile_diff`, gates, `diff`, `calo spec --deep` and `calibrate` |
| 7 | External Python services and plot types, channel wrapper libraries, additional schedulers, plotting backends |

## 21. Decisions recorded

1. **Language:** all Rust. C and C++ appear only where an MPI header or vendor ABI requires a translation unit (§9.5 B23).
2. **Probe form:** library plus thin binary. `calo` spawns the binary for real runs (I2).
3. **Probe linkage:** dynamic against an old glibc baseline. Vendor libraries by `dlopen` (I1).
4. **Observe loop:** single-threaded `epoll` with a writer thread (§9.2).
5. **Variants supported, not chosen:** GPU vendors and backends, GPU tracing source (in-process and external), shim injection (preload and link), call-graph method (fp and dwarf), symbolizer (`llvm-symbolizer` and `libdw`), region storage (events and aggregate), trace volume (full and aggregation-only), GPU-aware MPI (classified when present).
6. **Python:** optional, out-of-process, for user services and plotting only (§12.4).
7. **Defaults:** `fp` call graph, 499 Hz sampling, `--sample 1` per node class for discovery at minimum time limit, `rebuild: cache` with artifact hashing, `interleaved` run order.
8. **Plotting:** deferred. Tables and gates are core.
