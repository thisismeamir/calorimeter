# MWE 2: T1 CPU Stack Profiling & Symbol Checks (R-BLD-3)

This example tests `mode: profile` (`T1`) on a real C workload compiled with debug symbols and frame pointers, and demonstrates how Calorimeter rejects stripped binaries (`no_symbols`).

## 1. Create and Compile a Test Binary

```bash
mkdir -p /tmp/calo_mwe_t1 && cd /tmp/calo_mwe_t1
calo spec --connect local --out inventory.yaml

cat > busy.c <<'EOF'
#include <stdint.h>
#include <stdlib.h>

__attribute__((noinline)) uint64_t stencil_step(uint64_t n) {
    volatile uint64_t acc = 0;
    for (uint64_t i = 0; i < n; ++i) {
        acc += (i * 2654435761ULL) ^ (i >> 3);
    }
    return acc;
}

int main(int argc, char **argv) {
    uint64_t n = argc > 1 ? strtoull(argv[1], NULL, 10) : 60000000ULL;
    return stencil_step(n) == 0 ? 1 : 0;
}
EOF

cc -O1 -g -fno-omit-frame-pointer -o busy_prof busy.c
cp busy_prof busy_stripped && strip --strip-all busy_stripped
```

## 2. Create `suite.yaml`

Notice that `mode: profile` benchmarks **must** reference a build with `debug_info: required` (enforced by `calo lint` per Spec §8.3 R-BLD-3).

```yaml
schema: calo.suite/1
suite:
  name: mwe_profile_t1
  version: "0.1.0"

builds:
  profile_build:
    run: "cc -O1 -g -fno-omit-frame-pointer -o busy_prof busy.c"
    flags: "-O1 -g -fno-omit-frame-pointer"
    debug_info: required

steps:
  run_prof:
    run: "./busy_prof ${{ params.iters }}"
    measured: true
    timeout: "00:05:00"

benchmarks:
  - name: stencil_profile
    mode: profile
    machine: local
    build: profile_build
    steps: [run_prof]
    params:
      iters: 60000000
    probe:
      counters: [cycles, instructions, task-clock]
      profile:
        freq_hz: 199
        callgraph: fp
        max_loss: 0.05
```

## 3. Run and Inspect T1 Sidecars

```bash
calo --suite suite.yaml --inventory inventory.yaml lint
calo --suite suite.yaml --inventory inventory.yaml --results results run
ls results/*/records/stencil_profile.*/
```

You will see:
- `profile.samples.calotbl` — sampled IPs, TIDs, CPUs, timestamps, callchain spans
- `profile.chains.calotbl` — raw call-stack instruction pointers
- `profile.maps.calotbl` — `PERF_RECORD_MMAP2` memory maps with ELF `.note.gnu.build-id`
- `observations.profile` inside `rank0.yaml` with `method: fp`, `samples`, `lost`, and `modules`.

:::{note}
If you point the step at `./busy_stripped`, `probe` detects the missing `.symtab`/`.debug_info`, marks the record `status: invalid`, and records `WarningCode::NoSymbols` in `provenance.errors`.
:::
