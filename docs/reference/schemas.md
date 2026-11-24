# File Formats & Schemas

Every YAML document carries a versioned `schema:` header validated by `calo-schema` (and JSON Schema 2020-12 files in `schema/*.schema.yaml`). Readers ignore unknown fields (`deny_unknown_fields` is forbidden on record, runspec, and discovery types per Spec §18).

| File | Schema Header | Purpose |
|---|---|---|
| `inventory.yaml` | `calo.inventory/1` | Machines, node classes, launchers, modules, reference ceilings, discovered provenance |
| `suite.yaml` | `calo.suite/1` | Sources, patches, builds, steps, benchmarks, services, symbol rules, reports |
| `manifest.yaml` | `calo.manifest/1` | Resolved config with origin layer per key, inventory snapshot, source SHAs, run list |
| `runspec.yaml` | `calo.runspec/1` | Generated single-run input consumed by `probe` |
| `rank<k>.yaml`, `run.yaml` | `calo.record/2` | Immutable per-rank observations and control-plane aggregated run record |
| `discovery.yaml` | `calo.discovery/1` | Passive hardware & capability snapshot from `probe --discover` |
| `*.calotbl` | `calo.table/1` | Binary columnar sidecar format |

## Units Convention (Spec §0.3)

- Time: seconds (`_s`, `f64`) or monotonic nanoseconds (`_ns`, `i64` on `CLOCK_MONOTONIC_RAW`)
- Memory / I/O: bytes (`_bytes`, `u64`/`i64`)
- Energy / Power: Joules (`_j`), Watts (`_w`)
- Unmeasured values are omitted (`None` in Rust, `NaN` in binary float columns)—never `0`.

## Binary Sidecar Format (`calo.table/1`, Spec §11)

```text
+------------------+------------+------------------------+--------------------------------------+
| Magic (8 bytes)  | header_len | Header (UTF-8 YAML)    | Repeated Blocks:                     |
| "CALOTBL1"       | u32 LE     | schema: calo.table/1   |   rows: u32 LE                       |
|                  |            | columns + string dicts |   contiguous LE column payloads      |
|                  |            | + meta                 |   crc32: u32 LE (IEEE 802.3)         |
+------------------+------------+------------------------+--------------------------------------+
```

Column types: `int32`, `uint32`, `int64`, `uint64`, `float32`, `float64`, `uint8`, `bool`, `string_id`.

Blocks stream to disk via a dedicated writer thread (`WriterHandle`) so the `epoll` observe loop never blocks on disk I/O. If a process crashes mid-write, `calo-core::table` returns all intact preceding blocks and reports the incomplete tail as `TableError::TruncatedTail`.

When compiled with `--features calo-core/parquet`, `calo_core::export::export_sidecar_to_parquet` converts any `.calotbl` sidecar into Apache Parquet without modifying the original file.
