# MWE 1: Local T0 Benchmark, Resume & Report

This minimal working example requires **only `calo` and `probe`** (no compiler, no MPI, no GPU). It verifies T0 timing, PMU counters, host time-series sidecars, resumability, pivot tables, and SVG plots.

## 1. Setup

```bash
mkdir -p /tmp/calo_mwe_t0 && cd /tmp/calo_mwe_t0
calo spec --connect local --out inventory.yaml
calo verify --inventory inventory.yaml
```

## 2. Create `suite.yaml`

```yaml
schema: calo.suite/1
suite:
  name: mwe_local_t0
  version: "0.1.0"

steps:
  spin:
    run: "sh -c 'i=0; while [ $i -lt ${{ params.N }} ]; do i=$((i + 1)); done'"
    measured: true
    timeout: "00:05:00"

benchmarks:
  - name: spin_loop
    mode: measure
    machine: local
    steps: [spin]
    axes:
      N: [20000, 40000, 80000]
    warmup: 1
    repeat: 2

reports:
  - name: summary
    select: all
    tables:
      - { rows: benchmark, cols: params.N, value: wall_s.median }
    plots:
      - type: line
        x: params.N
        y: wall_s
        title: "Spin Loop Wall Time vs Iterations"
    output: [md, html, csv, svg]
```

## 3. Run & Verify

```bash
calo --suite suite.yaml --inventory inventory.yaml lint
calo --suite suite.yaml --inventory inventory.yaml plan
calo --suite suite.yaml --inventory inventory.yaml --results results run

# Resume test: re-running skips all completed runs immediately
calo --suite suite.yaml --inventory inventory.yaml --results results run

calo --results results status
calo --suite suite.yaml --inventory inventory.yaml --results results report
cat results/*/reports/summary/table_0.csv
```

## Expected Outputs

- `results/<suite-hash>/records/<run-id>/rank0.yaml` (immutable raw record)
- `results/<suite-hash>/records/<run-id>/run.yaml` (stamped aggregate record)
- `results/<suite-hash>/records/<run-id>/series.host.calotbl` (binary columnar host series)
- `results/<suite-hash>/reports/summary/{report.md,report.html,table_0.csv,plot_0.svg}`
