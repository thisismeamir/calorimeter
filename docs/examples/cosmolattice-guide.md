# Case Study: Benchmarking CosmoLattice End-to-End

`examples/cosmolattice/` in the Calorimeter repository is the canonical reference suite (Spec §19). It demonstrates **zero project-specific logic in Calorimeter**: every CosmoLattice concept—lattice size `N`, weak scaling `N = 128 * cbrt(ranks)`, patch ablations, input templates, MPI+OpenMP+GPU placement, and custom `sites_per_second` throughput—is expressed purely as YAML.

## Directory Structure

```text
examples/cosmolattice/
├── inventory.yaml          # Cluster node class (EPYC 7763 + 4x A100-SXM4-80GB, srun launcher)
├── suite.yaml              # Sources, patches, release/profile builds, 5 benchmarks, services, reports
├── patches/
│   └── patchset.yaml       # Metadata for calo-regions.patch (requires / conflicts_with)
└── templates/
    └── model.in            # Parameter file template ({{ N }}, {{ dt }}, {{ steps }}, {{ output_cadence }})
```

## Benchmarks Defined in `suite.yaml`

1. **`baseline` (`measure`)**: Single-node, single-rank reference (`N=128`, `warmup: 1`, `repeat: 3`) used as the gate baseline.
2. **`strong-scaling` (`measure`)**: Sweeps `nodes: [1, 2, 4, 8]` at fixed `N=256`, `rpn=4`, `threads=16`, `gpus_per_rank=1`.
3. **`weak-scaling` (`measure`)**: Sweeps `ranks: [1, 8, 64]` with derived expressions:
   ```yaml
   derive:
     nodes: "ranks"
     N: "128 * cbrt(ranks)"
   ```
   producing `N = 128, 256, 512` automatically!
4. **`patch-effect` (`measure`)**: Sweeps `patch: [none, regions]` to quantify instrumentation overhead as a first-class axis.
5. **`strong-scaling-profile` (`profile`)**: Derived from `strong-scaling` via `from: strong-scaling` + `where: "params.nodes <= 4"`, using `build: profile` (`debug_info: required`), `mpi: summary`, and `gpu: { state: true, trace: activity }`.

## Verify Locally Before Deploying to a Cluster

You can lint, plan, and dry-run the full CosmoLattice suite locally without a Slurm cluster:

```bash
calo --suite examples/cosmolattice/suite.yaml      --inventory examples/cosmolattice/inventory.yaml lint

calo --suite examples/cosmolattice/suite.yaml      --inventory examples/cosmolattice/inventory.yaml plan

calo --suite examples/cosmolattice/suite.yaml      --inventory examples/cosmolattice/inventory.yaml run --dry-run
```

## Adapting for Your Cluster

1. Run `calo spec --connect slurm://<your-login> --list-partitions` to inspect partitions.
2. Replace `host`, `partitions`, `account`, and `node_classes` in `inventory.yaml` with your cluster's values (or generate it via `calo spec --connect local` for a workstation).
3. Pin `sources.cosmolattice.git` and `rev` to your fork/commit SHA, and place your `calo-regions.patch` in `patches/`.
4. Adjust `templates/model.in` keys to match your CosmoLattice model parameter file.
5. Run `calo run` → `calo status` → `calo analyze` → `calo report`!
