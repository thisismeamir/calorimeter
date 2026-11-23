//! Local subprocess execution adapter (`LocalAdapter`, spec §7 R-INV-6, §15, §20).

use crate::exec::adapter::{Adapter, JobHandle, JobState, StagedRun};
use crate::exec::error::ExecError;
use crate::proc::{CommandRunner, CommandSpec};
use calo_schema::to_yaml_string;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Executes staged runs synchronously on the local machine via `CommandRunner` (spec §7 R-INV-6, §20).
pub struct LocalAdapter {
    runner: Arc<dyn CommandRunner>,
    states: Mutex<BTreeMap<String, JobState>>,
}

impl LocalAdapter {
    /// Creates a new `LocalAdapter` backed by `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            states: Mutex::new(BTreeMap::new()),
        }
    }
}

impl Adapter for LocalAdapter {
    fn stage(&self, staged: StagedRun) -> Result<StagedRun, ExecError> {
        if let Some(parent) = staged.runspec_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ExecError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let out_dir = Path::new(&staged.runspec.outputs.dir);
        fs::create_dir_all(out_dir).map_err(|source| ExecError::Io {
            path: out_dir.to_path_buf(),
            source,
        })?;
        let yaml = to_yaml_string(&staged.runspec)?;
        fs::write(&staged.runspec_path, yaml).map_err(|source| ExecError::Io {
            path: staged.runspec_path.clone(),
            source,
        })?;
        Ok(staged)
    }

    fn submit(&self, staged: &StagedRun) -> Result<JobHandle, ExecError> {
        let Some((program, args)) = staged.argv.split_first() else {
            return Err(ExecError::MissingTarget {
                kind: "argv",
                name: staged.run_id.clone(),
            });
        };

        let mut spec = CommandSpec::new(program.clone(), args.to_vec()).with_cwd(&staged.cwd);
        for (k, v) in &staged.env {
            spec.env.insert(k.clone(), v.clone());
        }
        if let Some(t) = staged.timeout {
            spec = spec.with_timeout(t);
        }

        let out = self.runner.run(&spec)?;
        let state = if out.timed_out {
            JobState::TimedOut
        } else if out.success() {
            JobState::Completed
        } else {
            JobState::Failed {
                code: out.exit_code,
            }
        };

        if let Ok(mut guard) = self.states.lock() {
            guard.insert(staged.run_id.clone(), state);
        }

        Ok(JobHandle {
            id: staged.run_id.clone(),
            runspec_path: staged.runspec_path.clone(),
            outputs_dir: Path::new(&staged.runspec.outputs.dir).to_path_buf(),
        })
    }

    fn poll(&self, handle: &JobHandle) -> Result<JobState, ExecError> {
        let guard = self.states.lock().map_err(|_| ExecError::MissingTarget {
            kind: "job_state",
            name: handle.id.clone(),
        })?;
        Ok(guard.get(&handle.id).cloned().unwrap_or(JobState::Pending))
    }

    fn fetch(&self, _handle: &JobHandle, _dest_dir: &Path) -> Result<(), ExecError> {
        // LocalAdapter writes directly into the target `outputs_dir`.
        Ok(())
    }

    fn cancel(&self, handle: &JobHandle) -> Result<(), ExecError> {
        if let Ok(mut guard) = self.states.lock() {
            guard.insert(handle.id.clone(), JobState::Cancelled);
        }
        Ok(())
    }
}
