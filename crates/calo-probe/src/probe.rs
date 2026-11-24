//! `Probe::run` 8-step state machine for T0/T1/G0/M0 execution, record assembly, and atomic write
//! (Spec §9.0–§9.2, §10).

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use calo_schema::{
    common::{
        CpuTier, GpuTier, Mode, MpiTier, RegionStorageMode, Status, TiersActive, Warning,
        WarningCode,
    },
    discovery::ObservedHardware,
    record::{
        Counter as SchemaCounter, EnergyJoules, ExitInfo, PhaseSpan, Record, RecordEnvironment,
        RecordIdentity, RecordObservations, RecordPlacement, RecordProvenance, RecordScalars,
        RegionNode as SchemaRegionNode, Regions, ReportedMetric as SchemaReportedMetric,
        ThreadRegions, TraceOverflow,
    },
    runspec::{NumaPolicy as SchemaNumaPolicy, RunSpec},
    SchemaDoc,
};

use crate::{
    channel::{ChannelAccumulator, ChannelFifo, RegionMode, RegionNode},
    clock::{now_ns, realtime_rfc3339},
    counters::{check_mux_warning, CounterAlias, CounterScope, CounterSet},
    gpu::GpuStateCollector,
    observe::run_observe_loop,
    profile::{ProfileCollector, ProfileConfig},
    scalars::{merge_peak_rss, CgroupSnapshot},
    series::HostSampler,
    spawn::{spawn_gated, ChildSetup, ExecError, ExitStatus, NumaPolicy, Redirect, Rusage},
    sys::poll::SignalGuard,
    table_writer::{TableWriter, WriterHandle},
};

/// Unrecoverable probe-side errors (Spec §9.0 R-API-2).
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("invalid runspec: {0}")]
    InvalidRunSpec(String),
    #[error("output directory error: {0}")]
    OutputIo(#[from] io::Error),
    #[error("record validation failed: {0}")]
    RecordValidation(String),
}

/// Optional overrides not yet present in `calo.runspec/1` (or populated from RunSpec).
#[derive(Debug, Clone, Default)]
pub struct ProbeOptions {
    pub timeout_s: Option<u64>,
    pub counters_override: BTreeMap<String, String>,
    pub counters_scope: CounterScope,
    pub max_sample_hz: Option<u32>,
    pub rank_aware: bool,
}

/// Outcome returned by [`Probe::run`] for all workload completions (ok, failed, timeout, invalid).
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub record_path: PathBuf,
    pub record: Record,
    pub exit_code: i32,
}

pub struct Probe;

impl Probe {
    /// Executes a `RunSpec` through the 8-step state machine:
    /// Prepare → Signals → Spawn → Arm → Exec → Observe → Reap → Finalize.
    pub fn run(spec: &RunSpec) -> Result<RunOutcome, ProbeError> {
        Self::run_with_options(spec, &ProbeOptions::default())
    }

    /// Executes a `RunSpec` with explicit [`ProbeOptions`] (timeout, counter overrides/scope).
    pub fn run_with_options(spec: &RunSpec, opts: &ProbeOptions) -> Result<RunOutcome, ProbeError> {
        // 1. Prepare
        spec.check()
            .map_err(|e| ProbeError::InvalidRunSpec(e.to_string()))?;
        let out_dir = PathBuf::from(&spec.outputs.dir);
        fs::create_dir_all(&out_dir)?;

        let rank_ctx = resolve_rank_context(spec, opts);
        let multi_rank = rank_ctx.ranks > 1 || rank_ctx.rank > 0;
        let fifo_name = if multi_rank {
            format!("channel.rank{}.fifo", rank_ctx.rank)
        } else {
            "channel.fifo".to_string()
        };
        let series_rel = if multi_rank {
            format!("series.host.rank{}.calotbl", rank_ctx.rank)
        } else {
            "series.host.calotbl".to_string()
        };

        // 2. Signals: block SIGTERM/SIGINT/SIGHUP before spawning any threads (including
        // the writer thread) or forking, so process-directed signals go only to signalfd.
        let sig_guard = SignalGuard::block_termination_signals()?;

        let mut channel = ChannelFifo::create_named(&out_dir, &fifo_name)?;
        let series_path = out_dir.join(&series_rel);
        let writer = WriterHandle::spawn(
            TableWriter::create_host_series(&series_path, &series_rel)?,
            4096,
        );
        let region_mode = match spec.regions {
            Some(RegionStorageMode::Aggregate) => RegionMode::Aggregate,
            _ => RegionMode::Events,
        };
        let mut acc = ChannelAccumulator::new(region_mode);

        let requested_aliases = resolve_counter_aliases(spec)?;
        let start_rfc3339 = realtime_rfc3339();

        // 3. Spawn (from the same thread that runs epoll, since PDEATHSIG is thread-bound).
        let setup = build_child_setup(spec, channel.path())?;
        let mut child = spawn_gated(&setup)?;

        // 4. Arm counters, host sampler, optional T1 profile collector, and optional G0 GPU collector.
        let mut counter_set = CounterSet::open_with_overrides(
            child.pid(),
            &requested_aliases,
            &opts.counters_override,
            opts.counters_scope,
        );
        let mut sampler = HostSampler::new(child.pid());
        let mut profile_collector = if spec.mode == Mode::Profile {
            let mut cfg = ProfileConfig::default();
            if let Some(ref p) = spec.profile {
                if let Some(hz) = p.freq_hz {
                    cfg.freq_hz = hz;
                }
                if let Some(cg) = p.callgraph {
                    cfg.callgraph = cg;
                }
                if let Some(sb) = p.stack_bytes {
                    cfg.stack_bytes = sb;
                }
                cfg.attribute = p.attribute.clone();
                if let Some(ml) = p.max_loss {
                    cfg.max_loss = ml;
                }
            }
            Some(ProfileCollector::open(child.pid(), cfg, &out_dir)?)
        } else {
            None
        };

        let raw_hz = spec.sample_hz.unwrap_or(10).max(1);
        let sample_hz = opts
            .max_sample_hz
            .map_or(raw_hz, |cap| raw_hz.min(cap.max(1)));

        // 5. Exec (`wall_s` includes `execvpe` latency from `go_ns` to `exit_ns`).
        let go_ns = now_ns();
        let mut warnings: Vec<Warning> = counter_set.warnings().to_vec();
        let gpu_requested = spec.gpu.as_ref().and_then(|g| g.state) == Some(true);
        let gpu_sample_hz = spec
            .gpu
            .as_ref()
            .and_then(|g| g.sample_hz)
            .unwrap_or(10)
            .max(1);
        let mut gpu_collector = if gpu_requested {
            match GpuStateCollector::open(&out_dir, rank_ctx.rank)? {
                Some(g) => Some(g),
                None => {
                    warnings.push(Warning {
                        code: WarningCode::TierDegraded,
                        message: "GPU G0 state requested but NVML/GPU unavailable".to_string(),
                    });
                    None
                }
            }
        } else {
            None
        };
        let mut errors: Vec<Warning> = Vec::new();

        let (
            status_val,
            exit_info,
            process_exit_code,
            rusage,
            exit_ns,
            cgroup_start,
            cgroup_end,
            placement,
        ) = match child.go() {
            Err(ExecError::Exec(errno)) => {
                let exit_ns = now_ns();
                let _ = child.wait4();
                errors.push(Warning {
                    code: WarningCode::ExecFailed,
                    message: format!("execvpe failed with errno {errno}"),
                });
                (
                    Status::Failed,
                    ExitInfo {
                        code: Some(127),
                        signal: None,
                    },
                    127,
                    Rusage::default(),
                    exit_ns,
                    None,
                    None,
                    None,
                )
            }
            Err(ExecError::Io(e)) => {
                return Err(ProbeError::OutputIo(e));
            }
            Ok(()) => {
                let placement = read_placement_from_proc(child.pid());
                let cg_start = CgroupSnapshot::read(child.pid());
                sampler.rebase();

                // 6. Observe
                if let Some(ref mut g) = gpu_collector {
                    g.sample_all(now_ns());
                }
                let obs = run_observe_loop(
                    &child,
                    &mut channel,
                    &mut acc,
                    &mut sampler,
                    &writer,
                    &sig_guard,
                    sample_hz,
                    opts.timeout_s,
                    profile_collector.as_mut(),
                    gpu_collector.as_mut(),
                    gpu_sample_hz,
                )?;
                if let Some(ref mut g) = gpu_collector {
                    g.sample_all(obs.exit_ns);
                }

                // 7. Reap: final sample while zombie is still in /proc, then wait4.
                if let Some(row) = sampler.sample() {
                    writer.send_host_series(row);
                }
                let cg_end = CgroupSnapshot::read(child.pid());
                let (exit_status, ru) = child.wait4()?;

                let (status_val, exit_info, proc_code) =
                    classify_exit(exit_status, obs.timed_out, obs.signalled_by_parent);
                (
                    status_val,
                    exit_info,
                    proc_code,
                    ru,
                    obs.exit_ns,
                    cg_start,
                    cg_end,
                    placement,
                )
            }
        };

        let raw_counts = counter_set.read_all();
        warnings.extend(counter_set.warnings().iter().skip(warnings.len()).cloned());
        if let Some(mux_w) = check_mux_warning(&raw_counts) {
            warnings.push(mux_w);
        }

        let mut tail_events = Vec::new();
        let _ = channel.close_and_drain(exit_ns, &mut tail_events);
        for ev in tail_events {
            acc.apply(ev);
        }
        acc.finalize_at_exit(exit_ns);
        warnings.extend(acc.warnings.clone());

        let energy = sampler.finish_energy_j();
        let _ = writer.flush_now_blocking(Duration::from_secs(1));
        let writer_overflow = writer.overflow();
        let writer_failed = writer.failed();
        let writer_outcome = writer.finish();

        let (series_sidecar, writer_dead) = match writer_outcome {
            Ok(out) => {
                warnings.extend(out.warnings);
                (Some(out.sidecar), false)
            }
            Err(e) => {
                errors.push(Warning {
                    code: WarningCode::TraceOverflow,
                    message: format!("series.host writer thread failed: {e}"),
                });
                (None, true)
            }
        };

        let (profile_obs, profile_callgraph, profile_sample_loss, profile_invalid) =
            if let Some(mut prof) = profile_collector {
                let exe_path = Path::new(&spec.command[0]);
                let has_symbols = if exe_path.is_file() {
                    prof.check_main_executable_symbols(exe_path)
                } else {
                    true
                };
                match prof.finish(has_symbols) {
                    Ok(summary) => {
                        warnings.extend(summary.warnings);
                        errors.extend(summary.errors);
                        let cg = Some(summary.record_profile.method);
                        (
                            Some(summary.record_profile),
                            cg,
                            summary.sample_loss,
                            summary.invalid,
                        )
                    }
                    Err(e) => {
                        errors.push(Warning {
                            code: WarningCode::TraceOverflow,
                            message: format!("profile sidecar finalization failed: {e}"),
                        });
                        (None, None, None, true)
                    }
                }
            } else {
                (None, None, None, false)
            };

        let (gpu_obs, gpu_energy_j) = if let Some(g) = gpu_collector {
            match g.finish() {
                Ok(summary) => {
                    warnings.extend(summary.warnings);
                    (Some(summary.record_gpu), summary.total_energy_j)
                }
                Err(e) => {
                    errors.push(Warning {
                        code: WarningCode::TraceOverflow,
                        message: format!("gpu.state sidecar finalization failed: {e}"),
                    });
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        let final_status = if writer_dead || profile_invalid {
            Status::Invalid
        } else {
            status_val
        };
        let end_rfc3339 = realtime_rfc3339();
        let wall_s = (exit_ns.saturating_sub(go_ns) as f64) * 1e-9;

        // Build counters map (raw values preserved; `scaled: false`).
        let mut schema_counters = BTreeMap::new();
        for (name, c) in &raw_counts {
            schema_counters.insert(
                name.clone(),
                SchemaCounter {
                    value: c.value,
                    time_enabled_ns: c.time_enabled_ns,
                    time_running_ns: c.time_running_ns,
                    scaled: false,
                },
            );
        }

        let energy_j = {
            let ej = EnergyJoules {
                pkg: energy.pkg,
                dram: energy.dram,
                gpu: gpu_energy_j,
            };
            (!ej.is_empty()).then_some(ej)
        };

        let scalars = RecordScalars {
            wall_s: Some(wall_s),
            user_cpu_s: Some((rusage.utime_ns as f64) * 1e-9),
            sys_cpu_s: Some((rusage.stime_ns as f64) * 1e-9),
            peak_rss_bytes: Some(merge_peak_rss(rusage.maxrss, sampler.peak_rss_bytes())),
            minor_faults: Some(rusage.minflt),
            major_faults: Some(rusage.majflt),
            voluntary_ctx: Some(rusage.nvcsw),
            involuntary_ctx: Some(rusage.nivcsw),
            block_in_bytes: Some(rusage.inblock_bytes),
            block_out_bytes: Some(rusage.oublock_bytes),
            energy_j,
            counters: schema_counters,
        };

        // Tags: workload tags + host.note.* + channel.* + counter_scope + cgroup_scope.
        let mut tags = acc.tags.clone();
        for (idx, note) in sampler.notes().iter().enumerate() {
            tags.insert(format!("host.note.{idx}"), note.clone());
        }
        let cstats = acc.combined_stats(channel.stats());
        tags.insert("channel.lines".into(), cstats.lines.to_string());
        tags.insert(
            "channel.partial_joins".into(),
            cstats.partial_joins.to_string(),
        );
        tags.insert("channel.too_long".into(), cstats.too_long.to_string());
        tags.insert("channel.bad_utf8".into(), cstats.bad_utf8.to_string());
        tags.insert(
            "channel.unknown_verbs".into(),
            cstats.unknown_verbs.to_string(),
        );
        tags.insert(
            "channel.protocol_errors".into(),
            cstats.protocol_errors.to_string(),
        );

        let scope_label = if counter_set.any_open()
            && counter_set.kernel_excluded().len() == counter_set.open_names().len()
        {
            "user"
        } else {
            "user+kernel"
        };
        tags.insert("counter_scope".into(), scope_label.into());

        if let (Some(s), Some(e)) = (cgroup_start, cgroup_end) {
            let delta = s.delta(&e);
            if delta.nr_throttled.is_some()
                || delta.throttled_us.is_some()
                || delta.cpu_usage_us.is_some()
            {
                tags.insert("cgroup_scope".into(), "shared".into());
                if let Some(v) = delta.nr_throttled {
                    tags.insert("cgroup.nr_throttled".into(), v.to_string());
                }
                if let Some(v) = delta.throttled_us {
                    tags.insert("cgroup.throttled_us".into(), v.to_string());
                }
            }
        }

        let phases = if acc.phases.is_empty() {
            None
        } else {
            Some(
                acc.phases
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            PhaseSpan {
                                start_ns: v.start_ns,
                                end_ns: v.end_ns,
                            },
                        )
                    })
                    .collect(),
            )
        };

        let regions = if acc.region_trees.is_empty() {
            None
        } else {
            Some(Regions {
                threads: acc
                    .region_trees
                    .iter()
                    .map(|(&tid, roots)| ThreadRegions {
                        tid: tid as i64,
                        tree: convert_region_tree(roots),
                    })
                    .collect(),
            })
        };

        let reported = if acc.reported.is_empty() {
            None
        } else {
            Some(
                acc.reported
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            SchemaReportedMetric {
                                value: v.value,
                                unit: v.unit.clone(),
                            },
                        )
                    })
                    .collect(),
            )
        };

        let series = series_sidecar.map(|sc| {
            let mut m = BTreeMap::new();
            m.insert("host".to_string(), sc);
            m
        });

        let trace_overflow = {
            let wq = writer_overflow.saturating_add(writer_failed);
            (wq > 0).then_some(TraceOverflow {
                mpi: None,
                gpu: None,
                writer_queue: Some(wq),
            })
        };

        // Tiers active: T0 always active; T2 enabled when channel/tags observations are populated
        // so `Record::tier_authoritative_check` succeeds.
        let tiers_active = TiersActive {
            cpu: Some(CpuTier::T2),
            mpi: rank_ctx.rank_aware.then_some(MpiTier::M0),
            gpu: gpu_obs.as_ref().map(|_| GpuTier::G0),
        };

        let record = Record {
            schema: Record::SCHEMA.to_string(),
            identity: RecordIdentity {
                run_id: spec.run_id.clone(),
                benchmark: spec
                    .run_id
                    .split('.')
                    .next()
                    .unwrap_or(&spec.run_id)
                    .to_string(),
                group: None,
                mode: spec.mode,
                params: BTreeMap::new(),
                repeat: None,
                warmup: None,
                rank: Some(rank_ctx.rank),
                ranks: Some(rank_ctx.ranks),
                node: read_hostname(),
                local_rank: Some(rank_ctx.local_rank),
                suite_hash: None,
                linked_runs: Vec::new(),
                status: final_status,
            },
            environment: RecordEnvironment {
                machine: read_hostname().unwrap_or_else(|| "localhost".to_string()),
                node_class: None,
                fingerprint: None,
                declared: BTreeMap::new(),
                observed: read_cheap_observed_hardware(),
                placement,
                devices: Vec::new(),
                revisions: BTreeMap::new(),
                patchset: None,
                build: None,
                artifact_hashes: BTreeMap::new(),
                rendered_files: BTreeMap::new(),
                mpi_lib: None,
                modules: Vec::new(),
            },
            observations: RecordObservations {
                scalars: Some(scalars),
                phases,
                regions,
                reported,
                tags: Some(tags),
                series,
                profile: profile_obs,
                mpi: None,
                gpu: gpu_obs,
            },
            provenance: RecordProvenance {
                probe_version: env!("CARGO_PKG_VERSION").to_string(),
                shim_versions: None,
                counters: requested_aliases
                    .iter()
                    .map(|a| a.as_str().to_string())
                    .collect(),
                clock: Some("CLOCK_MONOTONIC_RAW".to_string()),
                callgraph: profile_callgraph,
                start: start_rfc3339,
                end: end_rfc3339,
                start_ns: Some(go_ns),
                exit: exit_info,
                sample_loss: profile_sample_loss,
                trace_overflow,
                tiers_active,
                warnings,
                errors,
            },
            aggregate: None,
        };

        // 8. Finalize: validate and write atomically (`rank<k>.yaml.tmp` -> fsync -> rename -> fsync dir).
        record
            .check()
            .map_err(|e| ProbeError::RecordValidation(e.to_string()))?;

        let record_path = out_dir.join(format!("rank{}.yaml", rank_ctx.rank));
        let yaml = calo_schema::yaml::to_string(&record)
            .map_err(|e| ProbeError::RecordValidation(e.to_string()))?;
        write_atomic(&record_path, yaml.as_bytes())?;

        Ok(RunOutcome {
            record_path,
            record,
            exit_code: process_exit_code,
        })
    }

    /// Passive hardware discovery without running a workload (Spec §6.3, §9.0).
    pub fn discover(
        opts: &crate::discover::DiscoverOpts,
    ) -> Result<calo_schema::discovery::Discovery, ProbeError> {
        let disc = crate::discover::discover_local(opts);
        disc.check()
            .map_err(|e| ProbeError::RecordValidation(e.to_string()))?;
        Ok(disc)
    }
}

fn resolve_counter_aliases(spec: &RunSpec) -> Result<Vec<CounterAlias>, ProbeError> {
    if spec.counters.is_empty() {
        return Ok(match spec.mode {
            Mode::Measure | Mode::Profile | Mode::Kernel => vec![
                CounterAlias::Cycles,
                CounterAlias::Instructions,
                CounterAlias::TaskClock,
            ],
        });
    }
    let mut out = Vec::with_capacity(spec.counters.len());
    for name in &spec.counters {
        let alias = CounterAlias::from_str(name).map_err(|e| {
            ProbeError::InvalidRunSpec(format!("unknown counter alias '{name}': {e}"))
        })?;
        out.push(alias);
    }
    Ok(out)
}

fn build_child_setup(spec: &RunSpec, fifo_path: &Path) -> Result<ChildSetup, ProbeError> {
    let mut argv = Vec::with_capacity(spec.command.len());
    for arg in &spec.command {
        argv.push(
            CString::new(arg.as_str())
                .map_err(|e| ProbeError::InvalidRunSpec(format!("invalid argv: {e}")))?,
        );
    }

    let mut env_map: BTreeMap<String, String> = std::env::vars().collect();
    for (k, v) in &spec.env {
        env_map.insert(k.clone(), v.clone());
    }
    env_map.insert(
        "CALO_CHANNEL".to_string(),
        fifo_path.to_string_lossy().into_owned(),
    );

    let mut env = Vec::with_capacity(env_map.len());
    for (k, v) in env_map {
        if let Ok(c) = CString::new(format!("{k}={v}")) {
            env.push(c);
        }
    }

    let cwd = if spec.cwd.is_empty() {
        None
    } else {
        Some(
            CString::new(spec.cwd.as_str())
                .map_err(|e| ProbeError::InvalidRunSpec(format!("invalid cwd: {e}")))?,
        )
    };

    let (cpu_list, numa) = if let Some(ref aff) = spec.affinity {
        let cpus = aff
            .cpu_list
            .as_deref()
            .map(parse_cpu_list)
            .transpose()
            .map_err(ProbeError::InvalidRunSpec)?;
        let numa = match aff.numa {
            Some(SchemaNumaPolicy::Interleave) => NumaPolicy::Interleave,
            Some(SchemaNumaPolicy::Bind) => NumaPolicy::Bind(vec![0]),
            Some(SchemaNumaPolicy::None) | None => NumaPolicy::None,
        };
        (cpus, numa)
    } else {
        (None, NumaPolicy::None)
    };

    let mut rlimits = Vec::new();
    if let Some(ref rl) = spec.rlimits {
        if let Some(core) = rl.core {
            rlimits.push((libc::RLIMIT_CORE as i32, core, core));
        }
    }

    Ok(ChildSetup {
        argv,
        env,
        cwd,
        cpu_list,
        numa,
        rlimits,
        stdout: Redirect::Inherit,
        stderr: Redirect::Inherit,
    })
}

fn parse_cpu_list(s: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let start: usize = a
                .trim()
                .parse()
                .map_err(|_| format!("bad cpu range '{part}'"))?;
            let end: usize = b
                .trim()
                .parse()
                .map_err(|_| format!("bad cpu range '{part}'"))?;
            if start > end {
                return Err(format!("inverted cpu range '{part}'"));
            }
            for cpu in start..=end {
                out.push(cpu);
            }
        } else {
            let cpu: usize = part
                .parse()
                .map_err(|_| format!("bad cpu index '{part}'"))?;
            out.push(cpu);
        }
    }
    Ok(out)
}

fn classify_exit(
    exit: ExitStatus,
    timed_out: bool,
    signalled_by_parent: Option<i32>,
) -> (Status, ExitInfo, i32) {
    if let Some(sig) = exit.signal {
        let info = ExitInfo {
            code: None,
            signal: Some(signal_name(sig)),
        };
        let status = if timed_out {
            Status::Timeout
        } else {
            Status::Failed
        };
        return (status, info, 128 + sig);
    }

    let code = exit.code.unwrap_or(1);
    let info = ExitInfo {
        code: Some(code),
        signal: None,
    };
    let status = if timed_out {
        Status::Timeout
    } else if signalled_by_parent.is_some() || code != 0 {
        Status::Failed
    } else {
        Status::Ok
    };
    (status, info, code)
}

fn signal_name(sig: i32) -> String {
    match sig {
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        libc::SIGQUIT => "SIGQUIT".to_string(),
        libc::SIGKILL => "SIGKILL".to_string(),
        libc::SIGSEGV => "SIGSEGV".to_string(),
        libc::SIGPIPE => "SIGPIPE".to_string(),
        libc::SIGTERM => "SIGTERM".to_string(),
        libc::SIGABRT => "SIGABRT".to_string(),
        other => format!("SIG{other}"),
    }
}

fn read_placement_from_proc(pid: libc::pid_t) -> Option<RecordPlacement> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let mut placement = RecordPlacement::default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Cpus_allowed_list:") {
            placement.cpu_list = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Mems_allowed_list:") {
            if let Ok(nodes) = parse_cpu_list(rest.trim()) {
                placement.numa = nodes.into_iter().map(|n| n as u32).collect();
            }
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            placement.threads = rest.trim().parse::<u32>().ok();
        }
    }
    Some(placement)
}

fn read_hostname() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_cheap_observed_hardware() -> ObservedHardware {
    let mut obs = ObservedHardware {
        kernel: fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string()),
        ..Default::default()
    };
    if let Some(paranoid) = fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
    {
        obs.policy
            .get_or_insert_with(Default::default)
            .perf_event_paranoid = Some(paranoid);
    }
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        let mut threads = 0u32;
        let mut model: Option<String> = None;
        for line in cpuinfo.lines() {
            if line.starts_with("processor") {
                threads += 1;
            } else if model.is_none() {
                if let Some(rest) = line.strip_prefix("model name") {
                    if let Some((_, val)) = rest.split_once(':') {
                        model = Some(val.trim().to_string());
                    }
                }
            }
        }
        if model.is_some() || threads > 0 {
            let cpu = obs.cpu.get_or_insert_with(Default::default);
            cpu.model = model;
            if threads > 0 {
                cpu.threads = Some(threads);
            }
        }
    }
    obs
}

fn convert_region_tree(roots: &BTreeMap<String, RegionNode>) -> Vec<SchemaRegionNode> {
    roots
        .iter()
        .map(|(name, node)| SchemaRegionNode {
            name: name.clone(),
            calls: node.calls,
            incl_s: node.incl_s,
            excl_s: node.excl_s,
            children: convert_region_tree(&node.children),
        })
        .collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("yaml.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        let dir_path = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(dir) = File::open(dir_path) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct RankContext {
    rank: u32,
    ranks: u32,
    local_rank: u32,
    rank_aware: bool,
}

fn read_env_u32(names: &[&str]) -> Option<u32> {
    for &key in names {
        if let Ok(val) = std::env::var(key) {
            if let Ok(n) = val.trim().parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

fn resolve_rank_context(spec: &RunSpec, opts: &ProbeOptions) -> RankContext {
    let rank_aware = opts.rank_aware || spec.rank.as_ref().is_some_and(|r| r.from_env);
    if !rank_aware {
        return RankContext {
            rank: 0,
            ranks: 1,
            local_rank: 0,
            rank_aware: false,
        };
    }
    let rank = read_env_u32(&[
        "OMPI_COMM_WORLD_RANK",
        "PMI_RANK",
        "SLURM_PROCID",
        "PMIX_RANK",
    ])
    .unwrap_or(0);
    let ranks = read_env_u32(&[
        "OMPI_COMM_WORLD_SIZE",
        "PMI_SIZE",
        "SLURM_NTASKS",
        "PMIX_SIZE",
    ])
    .unwrap_or(1)
    .max(1);
    let local_rank = read_env_u32(&[
        "OMPI_COMM_WORLD_LOCAL_RANK",
        "PMI_LOCAL_RANK",
        "SLURM_LOCALID",
    ])
    .unwrap_or(rank);
    RankContext {
        rank,
        ranks,
        local_rank,
        rank_aware: true,
    }
}
