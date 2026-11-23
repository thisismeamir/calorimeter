//! SSH staging/execution argv builder and POSIX `shell_quote` helper (spec §6.1, §7 R-INV-6).

use std::path::Path;

/// Configuration for `SshAdapter` including `ControlMaster` connection multiplexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfig {
    /// Remote host (`hostname` or `user@hostname`).
    pub target: String,
    /// Enable SSH `ControlMaster=auto` multiplexing.
    pub control_master: bool,
    /// Control socket path template (e.g. `"~/.ssh/calo-%r@%h:%p"`).
    pub control_path: String,
    /// `ControlPersist` duration (e.g. `"10m"`).
    pub control_persist: String,
    /// Remote staging tool (`"rsync"` or `"scp"`).
    pub stage_tool: String,
}

impl SshConfig {
    /// Creates an `SshConfig` for `target` with `ControlMaster` enabled by default.
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            control_master: true,
            control_path: "~/.ssh/calo-%r@%h:%p".to_string(),
            control_persist: "10m".to_string(),
            stage_tool: "rsync".to_string(),
        }
    }
}

/// Quotes an argument safely for POSIX `/bin/sh` using single-quote escaping.
pub fn shell_quote(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    let safe = arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='));
    if safe {
        return arg.to_string();
    }
    let escaped = arg.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Pure argv builder for SSH remote staging and execution (spec §6.1, §7 R-INV-6).
pub struct SshAdapter {
    config: SshConfig,
}

impl SshAdapter {
    /// Creates a new `SshAdapter` with `config`.
    pub fn new(config: SshConfig) -> Self {
        Self { config }
    }

    /// Returns the SSH `-o` option flags for `ControlMaster` when enabled.
    pub fn ssh_control_args(&self) -> Vec<String> {
        if !self.config.control_master {
            return Vec::new();
        }
        vec![
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            format!("ControlPath={}", self.config.control_path),
            "-o".to_string(),
            format!("ControlPersist={}", self.config.control_persist),
        ]
    }

    /// Builds the argv vector to stage `local_path` to `remote_path` via `rsync` or `scp`.
    pub fn stage_argv(&self, local_path: &Path, remote_path: &str) -> Vec<String> {
        let remote_dest = format!("{}:{}", self.config.target, remote_path);
        if self.config.stage_tool == "scp" {
            let mut argv = vec!["scp".to_string()];
            argv.extend(self.ssh_control_args());
            argv.push(local_path.display().to_string());
            argv.push(remote_dest);
            argv
        } else {
            let mut argv = vec!["rsync".to_string(), "-az".to_string()];
            if self.config.control_master {
                let ssh_cmd = format!("ssh {}", self.ssh_control_args().join(" "));
                argv.push("-e".to_string());
                argv.push(ssh_cmd);
            }
            argv.push(local_path.display().to_string());
            argv.push(remote_dest);
            argv
        }
    }

    /// Builds the `ssh` argv vector to execute `remote_argv` on the remote host with every argument shell-quoted.
    pub fn exec_argv(&self, remote_argv: &[String]) -> Vec<String> {
        let mut argv = vec!["ssh".to_string()];
        argv.extend(self.ssh_control_args());
        argv.push(self.config.target.clone());
        let quoted_cmd: Vec<String> = remote_argv.iter().map(|a| shell_quote(a)).collect();
        argv.push(quoted_cmd.join(" "));
        argv
    }
}
