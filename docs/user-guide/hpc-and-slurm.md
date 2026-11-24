# Multi-Node HPC, Slurm, MPI & GPU Benchmarking

## Slurm Partition Discovery (Safe & Read-Only First)

Calorimeter never submits jobs to unnamed partitions (Spec §6.2 R-SPC-1). Start with passive inspection of the login node:

```bash
calo spec --connect slurm://login.cluster.edu --list-partitions
```

This runs read-only `sinfo` queries and prints a table of partitions, node counts, time limits, features, and GRES, along with a ready-to-run discovery command.

## Slurm Execution (`SlurmAdapter`)

When a machine in `inventory.yaml` has `connect.type: slurm`:

```yaml
schema: calo.inventory/1
machines:
  hpc-cluster:
    connect:
      type: slurm
      host: login.cluster.edu
      partitions: [gpu]
      account: my_alloc
    launcher:
      mpi: "srun -n {ranks} --ntasks-per-node {ranks_per_node}"
      bind: { cpu: core, gpu: local_rank }
      gpu_aware_mpi: true
```

`calo run` automatically selects `SlurmAdapter`, renders `sbatch` scripts with your partition/account/resources, wraps the workload with `<launcher> probe --rank-aware --runspec <file>`, polls job completion, fetches per-rank `rank<k>.yaml` files, and aggregates them into `run.yaml`.

Preview every generated launcher command before submitting anything:

```bash
calo --suite suite.yaml --inventory inventory.yaml run --dry-run
```

## Multi-Rank MPI Isolation (`--rank-aware`, M0)

Under `--rank-aware` (or `runspec.rank.from_env: true`), each rank's `probe` process independently detects `(rank, ranks, local_rank)` from:
- OpenMPI: `OMPI_COMM_WORLD_RANK`, `OMPI_COMM_WORLD_SIZE`, `OMPI_COMM_WORLD_LOCAL_RANK`
- MPICH / Intel MPI / PMI: `PMI_RANK`, `PMI_SIZE`, `PMI_LOCAL_RANK`
- PMIx: `PMIX_RANK`, `PMIX_SIZE`
- Slurm: `SLURM_PROCID`, `SLURM_NTASKS`, `SLURM_LOCALID`

Each rank writes isolated files (`rank<k>.yaml`, `series.host.rank<k>.calotbl`, `channel.rank<k>.fifo`) so concurrent ranks sharing an output directory never collide. Afterward, `calo run` (and `calo analyze`) aggregates all ranks into `run.yaml` with `min`, `mean`, `max`, and `imbalance = max / mean`.

## M1 PMPI Interposition Shim (`calo-shim-mpi`)

To capture per-call MPI counts, timings, message-size histograms, sparse peer matrices, and collective sequence timestamps:

1. Build the shim on your cluster against the cluster's MPI module:
   ```bash
   module load openmpi   # or mpich / intel-mpi
   ./crates/calo-shim-mpi/build_shim.sh release
   ```
2. Run with `LD_PRELOAD` and `CALO_MPI_OUT`:
   ```bash
   CALO_MPI_OUT=$PWD/mpi_out    LD_PRELOAD=$PWD/target/release/libcalo_pmpi.so    mpirun -np 4 ./my_mpi_app
   ```
   Each rank writes `mpi_out/mpi_rank<k>.yaml` (`RecordMpi`) atomically at `MPI_Finalize`. Any shim error disables the shim without failing the workload (`R-SAF-4`).

## Out-of-Process GPU G0 Sampling (NVML)

Enable G0 GPU state sampling in any benchmark's `probe:` block:

```yaml
probe:
  gpu:
    state: true
    sample_hz: 10
```

`probe` loads `libnvidia-ml.so.1` via `dlopen`, polls utilization, VRAM usage, power (W), SM/memory clocks, temperature, and throttle masks on a dedicated `timerfd` inside the `epoll` loop, writes `gpu.state.dev<k>.calotbl`, and integrates GPU power into `observations.scalars.energy_j.gpu` (Joules).
