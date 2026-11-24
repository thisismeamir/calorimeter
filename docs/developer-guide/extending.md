# Extending Calorimeter

## Adding a New Scheduler Adapter

Implement the `Adapter` trait in `crates/calo-core/src/exec/adapter.rs`:

```rust
pub trait Adapter: Send + Sync {
    fn stage(&self, staged: StagedRun) -> Result<StagedRun, ExecError>;
    fn submit(&self, staged: &StagedRun) -> Result<JobHandle, ExecError>;
    fn poll(&self, handle: &JobHandle) -> Result<JobState, ExecError>;
    fn fetch(&self, handle: &JobHandle, dest_dir: &Path) -> Result<(), ExecError>;
    fn cancel(&self, handle: &JobHandle) -> Result<(), ExecError>;
}
```

Then register a new `ConnectType` variant in `calo-schema/src/inventory.rs` and dispatch it in `crates/calo-cli/src/run_cmd.rs::select_adapter`.

## Adding a Built-In Analysis Service

1. Implement the `Service` trait in `crates/calo-core/src/services/` operating on `Frame` (typed columnar table).
2. Declare `name()`, `inputs()`, `produces()`, and `requires()` (tiers) so `calo lint` automatically includes it in cycle and tier validation.
3. Register it in `ServiceRegistry::with_builtins()` in `crates/calo-core/src/services/registry.rs`.

## Adding a Native SVG Plot Renderer

1. Add a renderer function in `crates/calo-core/src/plot/renderers.rs` using the shared `SvgCanvas` builder in `crates/calo-core/src/plot/svg.rs`.
2. Follow Calorimeter's figure style: static `viewBox` SVG with no `<script>`, monochrome base (`#161616` axes/text, `#525252`/`#8d8d8d`/`#c6c6c6`/`#e0e0e0` series/grids), single `#7017FF` highlight accent, 1px strokes, flat rectangular legends.
3. Wire the new `plot_type` string inside `render_plot()`.

## Adding Another GPU Vendor Backend (AMD ROCm SMI / Intel Level Zero)

Follow the exact pattern of `crates/calo-probe/src/gpu/ffi.rs`:
1. Dynamically `dlopen` `libamd_smi.so` or `libze_loader.so.1` inside a dedicated submodule under `crates/calo-probe/src/gpu/`.
2. Return `Vec<ObservedGpu>` for discovery and `GpuStateRow` for G0 polling.
3. Combine results in `crates/calo-probe/src/gpu/mod.rs::discover_gpus()` and `GpuStateCollector::open()`.
