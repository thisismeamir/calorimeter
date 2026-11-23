//! Slurm batch script generation and `sbatch`/`squeue`/`sacct`/`scancel` adapter (spec §15 step 5).

use crate::adapters::error::AdapterError;
use crate::exec::{Adapter, ExecError, JobHandle, JobState, StagedRun};
use crate::proc::{CommandRunner, CommandSpec};
use calo_schema::Mode;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Parameters used to render a Slurm `sbatch` script (spec §15 step 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbatchRequest {
    /// Slurm job name (`--job-name`).
    pub job_name: String,
    /// Target partition (`--partition`).
    pub partition: Option<String>,
    /// Target account (`--account`).
    pub account: Option<String>,
    /// Number of nodes (`--nodes`).
    pub nodes: u32,
    /// Tasks per node (`--ntasks-per-node`).
    pub tasks_per_node: u32,
    /// CPUs per task (`--cpus-per-task`).
    pub cpus_per_task: Option<u32>,
    /// GPUs per task (`--gpus-per-task`) or GRES.
    pub gpus_per_task: Option<u32>,
    /// Optional explicit GRES string (e.g. `"gpu:4"`).
    pub gres: Option<String>,
    /// Wall-clock time limit (`--time=HH:MM:SS`).
    pub time_limit: Option<String>,
    /// Request exclusive node allocation (`--exclusive`, default for `Mode::Measure`).
    pub exclusive: bool,
    /// Environment modules to load before launching `srun`.
    pub modules: Vec<String>,
    /// Optional output log path (`--output`).
    pub output_log: Option<String>,
}

/// Slurm execution adapter implementing `Adapter` via `sbatch`, `squeue`/`sacct`, and `scancel` (spec §7 R-INV-6, §15).
pub struct SlurmAdapter {
    runner: Arc<dyn CommandRunner>,
    /// Default Slurm partition if not specified per run.
    pub default_partition: Option<String>,
    /// Default Slurm account.
    pub default_account: Option<String>,
}

impl SlurmAdapter {
    /// Creates a new `SlurmAdapter` backed by `runner`.
    pub fn new(
        runner: Arc<dyn CommandRunner>,
        default_partition: Option<String>,
        default_account: Option<String>,
    ) -> Self {
        Self {
            runner,
            default_partition,
            default_account,
        }
    }

    /// Generates an `sbatch` shell script for a single run (spec §15 step 5).
    pub fn render_sbatch_script(req: &SbatchRequest, launch_lines: &[String]) -> String {
        let mut s = String::from("#!/bin/bash\n");
        let _ = writeln!(s, "#SBATCH --job-name={}", req.job_name);
        if let Some(part) = &req.partition {
            let _ = writeln!(s, "#SBATCH --partition={part}");
        }
        if let Some(acct) = &req.account {
            let _ = writeln!(s, "#SBATCH --account={acct}");
        }
        let _ = writeln!(s, "#SBATCH --nodes={}", req.nodes.max(1));
        let _ = writeln!(
            s,
            "#SBATCH --ntasks-per-node={}",
            req.tasks_per_node.max(1)
        );
        if let Some(cpt) = req.cpus_per_task {
            let _ = writeln!(s, "#SBATCH --cpus-per-task={cpt}");
        }
        if let Some(gpt) = req.gpus_per_task {
            if gpt > 0 {
                let _ = writeln!(s, "#SBATCH --gpus-per-task={gpt}");
            }
        } else if let Some(gres) = &req.gres {
            let _ = writeln!(s, "#SBATCH --gres={gres}");
        }
        if let Some(time) = &req.time_limit {
            let _ = writeln!(s, "#SBATCH --time={time}");
        }
        if req.exclusive {
            let _ = writeln!(s, "#SBATCH --exclusive");
        }
        if let Some(out) = &req.output_log {
            let _ = writeln!(s, "#SBATCH --output={out}");
        }

        s.push('\n');
        for m in &req.modules {
            let _ = writeln!(s, "module load {m}");
        }
        if !req.modules.is_empty() {
            s.push('\n');
        }

        for line in launch_lines {
            let _ = writeln!(s, "{line}");
        }
        s
    }

    /// Generates an `sbatch` script for a `batch: group` allocation containing multiple `srun` invocations (spec §15 step 6).
    pub fn render_group_sbatch_script(
        req: &SbatchRequest,
        run_commands: &[Vec<String>],
    ) -> String {
        let lines: Vec<String> = run_commands.iter().map(|argv| argv.join(" ")).collect();
        Self::render_sbatch_script(req, &lines)
    }

    /// Parses the job ID from `sbatch --parsable` stdout (`"12345"` or `"12345;cluster"`).
    pub fn parse_sbatch_job_id(stdout: &str) -> Result<String, AdapterError> {
        let trimmed = stdout.trim();
        let id_part = trimmed.split(';').next().unwrap_or("").trim();
        if id_part.is_empty() || !id_part.chars().all(|c| c.is_ascii_digit()) {
            return Err(AdapterError::InvalidJobId {
                raw: stdout.to_string(),
            });
        }
        Ok(id_part.to_string())
    }
}

/// Parses Slurm `squeue` / `sacct` state strings into `JobState`.
pub fn parse_slurm_job_state(state_str: &str) -> JobState {
    // Strip trailing `+` (e.g. `CANCELLED+` or `CANCELLED by 1000`)
    let norm = state_str
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches('+')
        .to_ascii_uppercase();

    match norm.as_str() {
        "PENDING" | "PD" | "CONFIGURING" | "CF" | "RESIZING" | "RS" | "SUSPENDED" | "S" => {
            JobState::Pending
        }
        "RUNNING" | "R" | "COMPLETING" | "CG" | "STAGE_OUT" | "ST" => JobState::Running,
        "COMPLETED" | "CD" => JobState::Completed,
        "TIMEOUT" | "TO" => JobState::TimedOut,
        "CANCELLED" | "CA" | "PREEMPTED" | "PR" | "REVOKED" | "RV" => JobState::Cancelled,
        _ => JobState::Failed { code: None },
    }
}

impl Adapter for SlurmAdapter {
    fn stage(&self, staged: StagedRun) -> Result<StagedRun, ExecError> {
        if let Some(parent) = staged.runspec_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ExecError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let yaml = calo_schema::to_yaml_string(&staged.runspec)?;
        fs::write(&staged.runspec_path, yaml).map_err(|source| ExecError::Io {
            path: staged.runspec_path.clone(),
            source,
        })?;

        let req = SbatchRequest {
            job_name: format!("calo-{}", staged.run_id),
            partition: self.default_partition.clone(),
            account: self.default_account.clone(),
            nodes: 1,
            tasks_per_node: 1,
            cpus_per_task: None,
            gpus_per_task: None,
            gres: None,
            time_limit: Some("01:00:00".to_string()),
            exclusive: staged.runspec.mode == Mode::Measure,
            modules: Vec::new(),
            output_log: Some(format!("{}/slurm-%j.out", staged.runspec.outputs.dir)),
        };

        let script = Self::render_sbatch_script(&req, &[staged.argv.join(" ")]);
        let script_path = staged.runspec_path.with_extension("sbatch.sh");
        fs::write(&script_path, script).map_err(|source| ExecError::Io {
            path: script_path,
            source,
        })?;

        Ok(staged)
    }

    fn submit(&self, staged: &StagedRun) -> Result<JobHandle, ExecError> {
        let script_path = staged.runspec_path.with_extension("sbatch.sh");
        let out = self.runner.run(&CommandSpec::new(
            "sbatch",
            vec!["--parsable".to_string(), script_path.display().to_string()],
        ))?;
        if !out.success() {
            return Err(ExecError::MissingTarget {
                kind: "sbatch_job",
                name: out.stderr,
            });
        }
        let job_id = Self::parse_sbatch_job_id(&out.stdout).map_err(|e| ExecError::MissingTarget {
            kind: "job_id",
            name: e.to_string(),
        })?;
        Ok(JobHandle {
            id: job_id,
            runspec_path: staged.runspec_path.clone(),
            outputs_dir: Path::new(&staged.runspec.outputs.dir).to_path_buf(),
        })
    }

    fn poll(&self, handle: &JobHandle) -> Result<JobState, ExecError> {
        let sq = self.runner.run(&CommandSpec::new(
            "squeue",
            vec![
                "-h".to_string(),
                "-j".to_string(),
                handle.id.clone(),
                "-o".to_string(),
                "%T".to_string(),
            ],
        ))?;
        let trimmed = sq.stdout.trim();
        if sq.success() && !trimmed.is_empty() {
            return Ok(parse_slurm_job_state(trimmed));
        }

        let sa = self.runner.run(&CommandSpec::new(
            "sacct",
            vec![
                "-n".to_string(),
                "-X".to_string(),
                "-j".to_string(),
                handle.id.clone(),
                "-o".to_string(),
                "State".to_string(),
            ],
        ))?;
        let sa_trimmed = sa.stdout.trim();
        if sa.success() && !sa_trimmed.is_empty() {
            return Ok(parse_slurm_job_state(sa_trimmed));
        }

        Ok(JobState::Pending)
    }

    fn fetch(&self, _handle: &JobHandle, _dest_dir: &Path) -> Result<(), ExecError> {
        Ok(())
    }

    fn cancel(&self, handle: &JobHandle) -> Result<(), ExecError> {
        let _ = self
            .runner
            .run(&CommandSpec::new("scancel", vec![handle.id.clone()]))?;
        Ok(())
    }
}
