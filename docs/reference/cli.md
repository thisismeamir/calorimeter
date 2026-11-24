# CLI Reference (`calo` & `probe`)

## Global Flags (`calo`)

| Flag | Default | Description |
|---|---|---|
| `--suite <PATH>` | `suite.yaml` | Path to suite definition |
| `--inventory <PATH>` | `inventory.yaml` | Path to machine inventory |
| `--set <key.path=val>` | `[]` | Highest-precedence layered config override (repeatable) |
| `--results <DIR>` | `results` | Root output directory |
| `-q, --quiet` | `false` | Suppress non-error output |
| `-v, --verbose` | `false` | Verbose diagnostic output |

## `calo` Subcommands

| Command | Description |
|---|---|
| `calo spec [--connect local\|slurm://login] [--machine NAME] [--out FILE] [--merge] [--list-partitions] [--partition NAME...] [--yes] [--probe PATH]` | Passive hardware discovery or read-only Slurm partition listing; writes/merges `inventory.yaml` |
| `calo verify [--machine NAME] [--probe PATH]` | Compares live machine hardware fingerprint against `inventory.yaml` |
| `calo lint` | Validates schema headers, expressions, names, service DAG cycles, `debug_info: required`, report tier requirements, and single measured step |
| `calo plan [--format table\|yaml]` | Prints expanded `PinnedRun`s, upper-bound node-hours, and GPU-hours |
| `calo run [--select PATTERN] [--force] [--dry-run] [--probe PATH]` | Resolves sources/worktrees/builds and executes expanded runs (resumable unless `--force`) |
| `calo status [--format table\|yaml]` | Prints run completion summary (`done` / `failed` / `running` / `pending`) |
| `calo cancel [--select PATTERN]` | Cancels matching runs in the results directory |
| `calo analyze` | Aggregates per-rank `rank<k>.yaml` files into `run.yaml` and reports incomplete runs |
| `calo report [--baseline DIR]` | Renders `md`/`html`/`csv`/`svg` reports and evaluates regression gates |
| `calo diff <dirA> <dirB> [--allow-mixed]` | Compares two result directories side-by-side |

## Normative Exit Codes (Spec §17)

| Code | Constant | Meaning |
|---:|---|---|
| `0` | `SUCCESS` | Completed cleanly |
| `1` | `GENERIC_ERROR` | Unexpected I/O, parse, or runtime error |
| `2` | `LINT_FAILURE` | `calo lint` reported one or more errors |
| `3` | `RUN_FAILURE` | One or more benchmark runs failed or timed out |
| `4` | `GATE_FAILURE` | One or more report regression gates failed |
| `5` | `DISCOVERY_FAILURE` | Discovery or permission failure in `calo spec` / `calo verify` |

## `probe` Binary Flags (Spec §9.0)

```text
probe --runspec <FILE> [--rank-aware]   Execute one runspec, write rank<k>.yaml + sidecars
probe --discover [--out <FILE>]         Emit calo.discovery/1 YAML (stdout if --out omitted)
probe --version                         Print probe version
```

`probe` propagates the workload's exit code directly (`0`–`255`) and reserves exit code `70` for probe-side fatal errors.
