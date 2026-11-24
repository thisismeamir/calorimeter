# Quickstart (5 Minutes)

This walkthrough runs the complete local Calorimeter loop on your machine:

`calo spec` → `calo verify` → `calo lint` → `calo plan` → `calo run` → `calo status` → `calo report`

## 1. Discover Your Machine

Generate `inventory.yaml` by running passive, read-only hardware discovery:

```bash
calo spec --connect local --out inventory.yaml
```

Verify that your live hardware fingerprint matches the saved inventory:

```bash
calo verify --inventory inventory.yaml
```

## 2. Write a Minimal `suite.yaml`

Create `suite.yaml` in the same directory:

```yaml
schema: calo.suite/1
suite:
  name: quickstart
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
      N: [20000, 40000]
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
        title: "Spin Loop Wall Time"
    output: [md, html, csv, svg]
```

## 3. Lint and Plan Before Running

Validate schema headers, expressions, references, and tier requirements:

```bash
calo --suite suite.yaml --inventory inventory.yaml lint
```

Inspect the expanded parameter matrix and upper-bound resource cost:

```bash
calo --suite suite.yaml --inventory inventory.yaml plan
```

## 4. Execute (Resumable)

```bash
calo --suite suite.yaml --inventory inventory.yaml --results results run
```

Run the exact same command a second time—all 4 runs have valid `run.yaml` records matching `suite_hash` and `params`, so Calorimeter skips them immediately unless you pass `--force`.

Check run states:

```bash
calo --results results status
```

## 5. Generate Reports & Plots

```bash
calo --suite suite.yaml --inventory inventory.yaml --results results report
```

Inspect the generated files under `results/<suite-hash>/reports/summary/`:

- `report.md` — Markdown report with pivot tables and gate summary
- `report.html` — Self-contained HTML report with embedded SVGs
- `table_0.csv` — Machine-readable pivot table
- `plot_0.svg` — Static vector line plot
