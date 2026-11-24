# Reporting, Plotting & Regression Gates

Calorimeter includes **native Rust reporting and static SVG plotting** directly inside `calo report`—no Python, matplotlib, or external runtime is required.

## Output Formats

Declare the formats you want in each `reports:` entry in `suite.yaml`:

```yaml
reports:
  - name: scaling-summary
    select:
      group: scaling
      mode: measure
    tables:
      - { rows: benchmark, cols: params.nodes, value: wall_s.median }
    plots:
      - type: line
        x: params.nodes
        y: wall_s
        line_by: params.patch
        scale: { x: log2, y: log }
        errorbars: ci95
        title: "Strong Scaling Wall Time"
    gates:
      - { metric: wall_s.median, compare: { benchmark: baseline }, fail_if: "> +5%" }
    output: [md, html, csv, svg]
```

Running `calo report` writes artifacts into `results/<suite-hash>/reports/<report-name>/`:

| Format | File(s) Generated | Description |
|---|---|---|
| `md` | `report.md` | Markdown report with formatted tables, gate outcomes, and attached `context:` files |
| `html` | `report.html` | Standalone HTML page with tables, gates, and inline SVG plots |
| `csv` | `table_0.csv`, `table_1.csv`, ... | Raw numeric pivot tables ready for spreadsheets or papers |
| `svg` | `plot_0.svg`, `plot_1.svg`, ... | Static `viewBox` SVG figures (monochrome base, `#7017FF` accent, IBM Plex Sans, no scripts) |

## Built-In Native SVG Plot Types

All built-in plot renderers live in `calo-core/src/plot/renderers.rs` (and `calo-core/src/flame/render.rs`):

| `type` | Purpose | Key Options |
|---|---|---|
| `line` | Scaling curves, parameter sweeps, multi-series comparisons | `x`, `y`, `line_by`, `facet_by`, `scale: {x, y}` (`linear`/`log`/`log2`), `errorbars: ci95` |
| `bar` | Categorical comparisons across benchmarks or patches | `x`, `y`, `line_by`, `title` |
| `scaling_plot` | Strong/weak scaling speedup & efficiency curves with ideal reference line | `x`, `y`, `title` |
| `heatmap` | 2D parameter grid heatmaps | `x`, `y`, `value`, `title` |
| `rank_heatmap` | Per-rank performance & straggler visualization | `title` |
| `comm_matrix` | Sparse MPI peer-to-peer byte/message heatmap | `title` |
| `time_budget_bar` | Stacked breakdown of CPU compute, MPI wait/transfer, GPU, and idle time | `title` |
| `region_tree` | Hierarchical T2 channel region call tree | `title` |
| `flamegraph` / `diff_flamegraph` | Static SVG CPU call-stack flame graphs and differential flame graphs | `calo-core::flame` |

## Pivot Tables & Derived Metrics

Tables aggregate across repetitions automatically (supporting `.median`, `.mean`, `.min`, `.max`, or custom expression services defined under `services:`):

```yaml
services:
  sites_per_second:
    expr: "params.N**3 * reported.steps / phase.main"
    unit: sites/s

reports:
  - name: throughput
    select: all
    tables:
      - { rows: params.patch, cols: params.N, value: sites_per_second.median }
    output: [md, csv]
```

## Regression Gates & `calo diff`

Gates compare observed metrics against either an in-suite benchmark (`compare: { benchmark: baseline }`) or a stored baseline directory (`calo report --baseline /path/to/old_results`).

Supported `fail_if` syntax:
- Relative percentage threshold: `"> +5%"`, `"< -10%"`
- Arbitrary expression: `"observed > baseline * 1.05"`

If any gate fails, `calo report` prints the failed gate details to `stderr` and exits with normative exit code **`4`** (`GATE_FAILURE`), making it directly usable in CI pipelines.

To compare two result directories side-by-side without writing a report config:

```bash
calo diff results_before results_after
# Add --allow-mixed if comparing across incompatible probe major versions
```

## Optional External Python Plots & Services

If you need a domain-specific curve fit or custom matplotlib/seaborn figure, you can register an out-of-process Python function under `services:` (Spec §12.4):

```yaml
services:
  my_fit:
    python: my_pkg.analysis:fit_scaling
    inputs: [params.N, wall_s]
    produces: [alpha, r2]
```

Calorimeter exchanges typed CSVs with the Python runner out-of-process (`calo_runner.py`). If Python is unavailable, only that service fails (`python_unavailable`); all native Rust services and SVG plots continue working.
