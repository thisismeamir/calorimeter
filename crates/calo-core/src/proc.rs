//! Subprocess execution trait and `std::process::Command` runner (Global Rules, spec §8.3, §8.4).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Errors produced when spawning or waiting on a subprocess.
#[derive(Debug, Error)]
pub enum ProcError {
    /// Failed to spawn or wait on the subprocess.
    #[error("failed to execute `{program}`: {source}")]
    Spawn {
        /// Executable name or path.
        program: String,
        /// Underlying OS I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Specification of a subprocess invocation via argv vector (never a shell string unless `program == "sh"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Executable program name or path.
    pub program: String,
    /// Argument vector (`argv[1..]`).
    pub args: Vec<String>,
    /// Optional working directory.
    pub cwd: Option<PathBuf>,
    /// Environment variables to add/override.
    pub env: BTreeMap<String, String>,
    /// Environment variable names to remove from the child environment (`options.redact`, spec R-SAF-5).
    pub env_remove: Vec<String>,
    /// Optional wall-clock timeout.
    pub timeout: Option<Duration>,
}

impl CommandSpec {
    /// Creates a new `CommandSpec` for `program` and `args`.
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            timeout: None,
        }
    }

    /// Sets the working directory.
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.env.insert(key.into(), val.into());
        self
    }

    /// Sets the timeout duration.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Captured outcome of a subprocess invocation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandOutput {
    /// Exit status code (`None` if killed by signal or timed out).
    pub exit_code: Option<i32>,
    /// Signal name if terminated by signal.
    pub signal: Option<String>,
    /// `true` if the process was killed due to exceeding `CommandSpec::timeout`.
    pub timed_out: bool,
    /// Captured UTF-8 stdout.
    pub stdout: String,
    /// Captured UTF-8 stderr.
    pub stderr: String,
}

impl CommandOutput {
    /// Returns `true` if the command exited with code `0` and did not time out.
    pub const fn success(&self) -> bool {
        !self.timed_out && matches!(self.exit_code, Some(0))
    }
}

/// Subprocess execution abstraction allowing fake runners in unit/integration tests.
pub trait CommandRunner: Send + Sync {
    /// Executes `spec` and returns its captured output.
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcError>;
}

/// Real `std::process::Command` runner using an explicit argv vector.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        for k in &spec.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        if let Some(timeout) = spec.timeout {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = cmd.spawn().map_err(|source| ProcError::Spawn {
                program: spec.program.clone(),
                source,
            })?;

            let start = Instant::now();
            loop {
                match child.try_wait().map_err(|source| ProcError::Spawn {
                    program: spec.program.clone(),
                    source,
                })? {
                    Some(status) => {
                        let out = child
                            .wait_with_output()
                            .map_err(|source| ProcError::Spawn {
                                program: spec.program.clone(),
                                source,
                            })?;
                        return Ok(CommandOutput {
                            exit_code: status.code(),
                            signal: None,
                            timed_out: false,
                            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                        });
                    }
                    None => {
                        if start.elapsed() >= timeout {
                            let _ = child.kill();
                            let out = child.wait_with_output().ok();
                            return Ok(CommandOutput {
                                exit_code: None,
                                signal: Some("SIGKILL".to_string()),
                                timed_out: true,
                                stdout: out
                                    .as_ref()
                                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                                    .unwrap_or_default(),
                                stderr: out
                                    .as_ref()
                                    .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                                    .unwrap_or_default(),
                            });
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        } else {
            let out = cmd.output().map_err(|source| ProcError::Spawn {
                program: spec.program.clone(),
                source,
            })?;
            Ok(CommandOutput {
                exit_code: out.status.code(),
                signal: None,
                timed_out: false,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
    }
}
