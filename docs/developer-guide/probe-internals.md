# `calo-probe` Internals

## The 8-Step `Probe::run` State Machine (`src/probe.rs`)

1. **Prepare:** Validate `RunSpec::check()`, create `outputs.dir`, resolve `RankContext` (M0 `--rank-aware` / `from_env`), compute per-rank filenames (`channel.rank<k>.fifo`, `series.host.rank<k>.calotbl`, `rank<k>.yaml`).
2. **Signals:** Block `SIGTERM`, `SIGINT`, `SIGHUP` via `SignalGuard` (`signalfd`) **before** spawning the writer thread or forking, so termination signals are delivered exclusively through the `epoll` loop.
3. **Spawn (`src/spawn.rs`, `src/sys/spawn.rs`):** `fork()` a child that configures `setpgid`, `PR_SET_PDEATHSIG`, CPU affinity (`sched_setaffinity`), NUMA policy (`set_mempolicy`), `rlimits`, `cwd`, and `CALO_CHANNEL`, then blocks on a "go" pipe.
4. **Arm:** While the child is paused pre-`exec`, the parent opens:
   - T0 PMU counters (`CounterSet` with `disabled = 1`, `enable_on_exec = 1`, `inherit = 1`)
   - T1 perf ring samplers (`ProfileCollector` with FP or DWARF `sample_type`)
   - G0 NVML collector (`GpuStateCollector` via `dlopen`)
5. **Exec:** Parent writes to the go-pipe and reads the `O_CLOEXEC` exec-error pipe to capture `execvpe` errno accurately.
6. **Observe (`src/observe.rs`):** Single-threaded level-triggered `epoll` loop monitoring:
   - `TOK_PIDFD` (`pidfd_open`, with `SIGCHLD` self-pipe fallback on kernels `< 5.3`)
   - `TOK_CHANNEL` (`CALO_CHANNEL` non-blocking FIFO)
   - `TOK_SERIES_TIMER` (`timerfd` for host RSS/CPU/RAPL series)
   - `TOK_SIGNAL` (`signalfd` forwarding signals to the child's process group)
   - `TOK_TIMEOUT` (graceful `SIGTERM` followed by `SIGKILL` after 5 s)
   - `TOK_GPU_TIMER` (`timerfd` for NVML G0 polling + trapezoidal energy integration)
   - `TOK_PROFILE_BASE + i` (perf ring-buffer fds drained into `ProfileCollector`)
7. **Reap:** Final host/GPU sample while the child is still readable in `/proc`, `wait4` for `ExitStatus` + `Rusage`, final counter read, final FIFO & perf ring drains.
8. **Finalize:** Flush and close all `WriterHandle` threads, run ELF `.symtab`/`.debug_info` check in `Mode::Profile` (R-BLD-3), validate `Record::check()`, and write `rank<k>.yaml` atomically (`rank<k>.yaml.tmp` → `fsync` → `rename` → `fsync(dir)`).

## Safety Boundaries

- `src/sys/*.rs`: Linux syscalls (`perf_event_open`, `mmap`/`munmap`, `epoll`, `timerfd`, `signalfd`, `fork`/`execvpe`/`wait4`, `clock_gettime`, `mkfifo`).
- `src/gpu/ffi.rs`: `libc::dlopen` / `dlsym` / `dlclose` for NVML v2.
- Every other module in `calo-probe` carries `#![forbid(unsafe_code)]` (or `#![deny(unsafe_code)]` on `gpu/mod.rs` with `#[allow(unsafe_code)]` solely on `pub mod ffi;`).
