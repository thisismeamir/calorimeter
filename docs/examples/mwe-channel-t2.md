# MWE 3: T2 Channel Phases, Nested Regions & Custom Metrics

The `CALO_CHANNEL` text protocol (Spec §9.4) lets your workload report exact phase boundaries (`init`, `main`), nested per-thread regions (`fft`, `field_update`), scalar metrics, and string tags without linking against Calorimeter internals.

## Option A: C / C++ Workload Using `calo_channel.h`

Include the single header `crates/calo-channel/include/calo_channel.h` (zero external dependencies; becomes a no-op when `CALO_CHANNEL` is unset):

```c
#include "calo_channel.h"
#include <math.h>
#include <stdio.h>

int main(void) {
    calo_phase_start("init");
    volatile double acc = 0.0;
    for (int i = 0; i < 1000000; ++i) acc += sin((double)i);
    calo_phase_end("init");

    calo_phase_start("main");
    for (int step = 0; step < 5; ++step) {
        calo_region_start("timestep");
        calo_region_start("field_update");
        for (int i = 0; i < 500000; ++i) acc += cos((double)i);
        calo_region_end("field_update");
        calo_region_end("timestep");
    }
    calo_phase_end("main");

    calo_metric("steps", 5.0, "count");
    calo_tag("integrator", "leapfrog");
    return acc == 1e300 ? 1 : 0;
}
```

Compile and run under `calo`:

```bash
cc -O2 -I /path/to/calorimeter/crates/calo-channel/include demo.c -lm -o demo
```

## Option B: Pure Shell / Any Language (Direct FIFO Text Protocol)

Because `CALO_CHANNEL` is a plain UTF-8 line-oriented FIFO, any language can emit events directly:

```yaml
schema: calo.suite/1
suite:
  name: mwe_channel_t2
  version: "0.1.0"

steps:
  run_channel:
    run: |
      sh -c '
        echo "phase init start" > "$CALO_CHANNEL"
        sleep 0.05
        echo "phase init end" > "$CALO_CHANNEL"
        echo "phase main start" > "$CALO_CHANNEL"
        echo "tid=1 region compute start" > "$CALO_CHANNEL"
        sleep 0.10
        echo "tid=1 region compute end" > "$CALO_CHANNEL"
        echo "phase main end" > "$CALO_CHANNEL"
        echo "metric steps=100 count" > "$CALO_CHANNEL"
        echo "tag solver=demo" > "$CALO_CHANNEL"
      '
    measured: true
    timeout: "00:02:00"

benchmarks:
  - name: channel_demo
    mode: measure
    machine: local
    steps: [run_channel]

services:
  steps_per_second:
    expr: "reported.steps / phase.main"
    unit: steps/s
```

Run `calo run` and inspect `observations.{phases,regions,reported,tags}` in `rank0.yaml`!
