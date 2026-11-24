# MWE 4: Multi-Rank MPI (`--rank-aware` M0 + PMPI Shim M1)

This example tests multi-rank isolation (`M0`) and per-call PMPI aggregation (`M1`) on any machine with OpenMPI or MPICH installed.

## 1. Build `probe` and the M1 PMPI Shim

```bash
cargo build --release -p calo-probe -p calo-shim-mpi
./crates/calo-shim-mpi/build_shim.sh release
# Produces target/release/libcalo_pmpi.so
```

## 2. Compile a Minimal MPI Ring / Allreduce Program

```bash
mkdir -p /tmp/calo_mwe_mpi && cd /tmp/calo_mwe_mpi

cat > mpi_ring.c <<'EOF'
#include <mpi.h>
#include <stdio.h>

int main(int argc, char **argv) {
    MPI_Init(&argc, &argv);
    int rank = 0, size = 1;
    MPI_Comm_rank(MPI_COMM_WORLD, &rank);
    MPI_Comm_size(MPI_COMM_WORLD, &size);

    double local = (double)(rank + 1), global = 0.0;
    for (int i = 0; i < 200; ++i) {
        MPI_Allreduce(&local, &global, 1, MPI_DOUBLE, MPI_SUM, MPI_COMM_WORLD);
    }
    if (size > 1) {
        int next = (rank + 1) % size;
        int prev = (rank - 1 + size) % size;
        double recv_val = 0.0;
        MPI_Sendrecv(&local, 1, MPI_DOUBLE, next, 0,
                     &recv_val, 1, MPI_DOUBLE, prev, 0,
                     MPI_COMM_WORLD, MPI_STATUS_IGNORE);
    }
    MPI_Finalize();
    return 0;
}
EOF

mpicc -O2 mpi_ring.c -o mpi_ring
```

## 3. Run M0 (`probe --rank-aware`) + M1 (`libcalo_pmpi.so`)

Create a minimal `runspec.yaml`:

```yaml
schema: calo.runspec/1
run_id: mpi_ring.np4.r0
mode: profile
command: ["./mpi_ring"]
cwd: "/tmp/calo_mwe_mpi"
rank:
  from_env: true
outputs:
  dir: "/tmp/calo_mwe_mpi/out"
```

Launch 4 ranks under `mpirun`:

```bash
CALO_MPI_OUT=/tmp/calo_mwe_mpi/out LD_PRELOAD=/path/to/calorimeter/target/release/libcalo_pmpi.so mpirun -np 4 /path/to/calorimeter/target/release/probe --rank-aware --runspec runspec.yaml
```

## 4. Inspect Per-Rank & Aggregated Outputs

```bash
ls /tmp/calo_mwe_mpi/out
# rank0.yaml .. rank3.yaml
# series.host.rank0.calotbl .. series.host.rank3.calotbl
# mpi_rank0.yaml .. mpi_rank3.yaml (M1 per-call summaries + shim_overhead_ns_per_call)

cat /tmp/calo_mwe_mpi/out/mpi_rank0.yaml
```

Running `calo --results /tmp/calo_mwe_mpi analyze` aggregates `rank0.yaml..rank3.yaml` into `run.yaml` with cross-rank `min`, `mean`, `max`, and `imbalance = max / mean`.
