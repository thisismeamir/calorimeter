#!/usr/bin/env bash
set -euo pipefail

PROFILE="${1:-release}"
if [[ "$PROFILE" == "release" ]]; then
  cargo build --release -p calo-shim-mpi
  OUT_DIR="target/release"
else
  cargo build -p calo-shim-mpi
  OUT_DIR="target/debug"
fi

mpicc -shared -fPIC -O2 \
  crates/calo-shim-mpi/c/pmpi_wrap.c \
  -L "$OUT_DIR" -lcalo_shim_mpi \
  -Wl,-rpath,'$ORIGIN' \
  -o "$OUT_DIR/libcalo_pmpi.so"

echo "Built $OUT_DIR/libcalo_pmpi.so"
