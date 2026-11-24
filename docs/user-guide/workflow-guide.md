# Step-by-Step Guide: Building Your Benchmarking Process

This guide walks through designing a rigorous, reproducible benchmarking suite for a real scientific or HPC codebase (C/C++/Fortran/Rust) from scratch.

## Step 1: Discover and Curate `inventory.yaml`

Start by observing the machine rather than hand-typing specs:

```bash
# Local workstation
calo spec --connect local --out inventory.yaml

# Slurm cluster (passive, read-only partition inspection first)
calo spec --connect slurm://login.cluster.edu --list-partitions
```

After discovery writes `inventory.yaml`, you can safely add **user-owned keys** (`tags`, `workdir`, `modules`, `launcher`, `policy`, `reference`). Running `calo spec --merge` later refreshes discovered hardware facts without overwriting your user keys, and marks any old `reference` benchmarks `stale: true` if the hardware `fingerprint` changes.

## Step 2: Pin Sources, Patches, and Build Variants

Never benchmark an untracked dirty checkout. Declare `sources:`, `patches:`, and `builds:` in `suite.yaml`:

```yaml
sources:
  mycode:
    git: https://github.com/org/mycode.git
    rev: v2.1.0        # resolved to exact commit SHA in manifest.yaml
    submodules: true

patches:
  dir: patches/
  sets:
    none: []
    instrumented: [calo-regions.patch]
    optimized: [simd-kernel.patch, calo-regions.patch]

builds:
  release:
    run: |
      cmake -B build -DCMAKE_BUILD_TYPE=Release
      cmake --build build -j
    flags: "-O3 -DNDEBUG"
    artifacts: ["build/mycode"]
  profile:
    run: |
      cmake -B build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-fno-omit-frame-pointer"
      cmake --build build -j
    flags: "-O3 -g -fno-omit-frame-pointer"
    debug_info: required
    artifacts: ["build/mycode"]
```

When `calo run` executes, Calorimeter:
1. Resolves `rev` to an exact commit SHA and records it in `manifest.yaml`.
2. Creates an isolated git worktree under `workdir/worktrees/` (never touching your working checkout, `R-SAF-2`).
3. Applies each patch set in order and hashes the resulting git tree.
4. Builds once per unique `(source SHA, patch tree, CPU, GPU arch, flags)` cache key and hashes the resulting binaries to detect nondeterministic builds (`R-BLD-2`).

## Step 3: Define Parameterized Steps and Templates

Keep input files as templates (`{{ var }}`) rather than checked-in duplicates:

```yaml
steps:
  run_sim:
    run: ./build/mycode --input ${{ files.cfg }}
    measured: true
    resources:
      nodes: "${{ params.nodes }}"
      ranks_per_node: "${{ params.rpn }}"
      threads_per_rank: "${{ params.threads }}"
    files:
      cfg:
        template: templates/sim.in
        vars:
          N: "${{ params.N }}"
          steps: "${{ params.steps }}"
    timeout: "00:30:00"
```

## Step 4: Design Clean Benchmark Axes (`measure` + Derived `profile`)

Separate clean timing (`measure`) from profiling (`profile`), and use `derive:` for weak scaling:

```yaml
benchmarks:
  - name: strong_scaling
    group: scaling
    mode: measure
    machine: local
    build: release
    steps: [run_sim]
    params: { N: 256, rpn: 4, threads: 4, steps: 200, patch: none }
    axes:
      nodes: [1, 2, 4]
    scaling:
      kind: strong
      over: nodes
      baseline: { nodes: 1 }
    warmup: 1
    repeat: 3

  - name: strong_scaling_profile
    group: scaling
    mode: profile
    machine: local
    build: profile
    steps: [run_sim]
    from: strong_scaling
    where: "params.nodes <= 2"
    probe:
      profile: { freq_hz: 499, callgraph: fp }
```

## Step 5: Lint, Plan, Dry-Run, Execute, Report

Always run the three zero-cost verification commands before submitting jobs:

```bash
calo --suite suite.yaml --inventory inventory.yaml lint
calo --suite suite.yaml --inventory inventory.yaml plan
calo --suite suite.yaml --inventory inventory.yaml run --dry-run
```

Then execute and generate reports:

```bash
calo --suite suite.yaml --inventory inventory.yaml --results results run
calo --suite suite.yaml --inventory inventory.yaml --results results report
```

## Step 6: Guard Against Regressions Across Commits

Store a blessed baseline results directory and enforce automated gates in CI or before merging optimizations:

```bash
calo --suite suite.yaml --inventory inventory.yaml --results results_new   report --baseline results_baseline
# Exits with code 4 if any gate fails (e.g. fail_if: "> +5%")

calo diff results_baseline results_new
```
