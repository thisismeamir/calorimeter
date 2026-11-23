use calo_core::proc::{CommandOutput, CommandRunner, CommandSpec, ProcError};
use calo_core::python::{run_python_service, PythonRunnerConfig};
use calo_core::services::{Column, Frame, Options, ServiceError};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("calo_py_{tag}_{nanos}"));
    fs::create_dir_all(&p).unwrap();
    p
}

struct FakePyRunner {
    last_spec: Mutex<Option<CommandSpec>>,
    mode: &'static str,
}

impl CommandRunner for FakePyRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcError> {
        if spec.args.first().map(String::as_str) == Some("--version") {
            if self.mode == "missing" {
                return Ok(CommandOutput {
                    exit_code: Some(127),
                    stderr: "python3: command not found".to_string(),
                    ..CommandOutput::default()
                });
            }
            return Ok(CommandOutput {
                exit_code: Some(0),
                stdout: "Python 3.11.0\n".to_string(),
                ..CommandOutput::default()
            });
        }

        *self.last_spec.lock().unwrap() = Some(spec.clone());

        match self.mode {
            "timeout" => Ok(CommandOutput {
                timed_out: true,
                ..CommandOutput::default()
            }),
            "bad_col" => {
                let out_pos = spec.args.iter().position(|a| a == "--out").unwrap() + 1;
                fs::write(&spec.args[out_pos], "run_id:str,wrong_col:float\nr0,1.5\n").unwrap();
                Ok(CommandOutput {
                    exit_code: Some(0),
                    ..CommandOutput::default()
                })
            }
            _ => {
                let out_pos = spec.args.iter().position(|a| a == "--out").unwrap() + 1;
                fs::write(&spec.args[out_pos], "run_id:str,alpha:float\nr0,2.5\n").unwrap();
                Ok(CommandOutput {
                    exit_code: Some(0),
                    ..CommandOutput::default()
                })
            }
        }
    }
}

#[test]
fn python_service_round_trip_redaction_mismatch_timeout_and_missing() {
    let dir = temp_dir("py");
    let mut frame = Frame::new();
    frame
        .insert_column("run_id", Column::Str(vec!["r0".to_string()]))
        .unwrap();
    frame
        .insert_column("wall_s", Column::Float(vec![10.0]))
        .unwrap();

    let cfg = PythonRunnerConfig {
        redact: vec!["AWS_SECRET_ACCESS_KEY".to_string()],
        timeout_s: 30,
        ..PythonRunnerConfig::default()
    };

    // Success & redaction
    let runner_ok = FakePyRunner {
        last_spec: Mutex::new(None),
        mode: "ok",
    };
    let out = run_python_service(
        "my.fit",
        "pkg.mod:fit",
        &["wall_s".to_string()],
        &["alpha".to_string()],
        &frame,
        &Options::default(),
        &cfg,
        &dir,
        &runner_ok,
    )
    .unwrap();
    assert_eq!(out.column("alpha").unwrap().to_f64_vec("alpha").unwrap(), vec![2.5]);
    assert!(runner_ok
        .last_spec
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .env_remove
        .contains(&"AWS_SECRET_ACCESS_KEY".to_string()));

    // Schema mismatch (missing `alpha`)
    let runner_bad = FakePyRunner {
        last_spec: Mutex::new(None),
        mode: "bad_col",
    };
    let err_col = run_python_service(
        "my.fit",
        "pkg.mod:fit",
        &["wall_s".to_string()],
        &["alpha".to_string()],
        &frame,
        &Options::default(),
        &cfg,
        &dir,
        &runner_bad,
    )
    .unwrap_err();
    assert!(matches!(err_col, ServiceError::InvalidInput { .. }));

    // Timeout
    let runner_to = FakePyRunner {
        last_spec: Mutex::new(None),
        mode: "timeout",
    };
    let err_to = run_python_service(
        "my.fit",
        "pkg.mod:fit",
        &["wall_s".to_string()],
        &["alpha".to_string()],
        &frame,
        &Options::default(),
        &cfg,
        &dir,
        &runner_to,
    )
    .unwrap_err();
    assert!(matches!(err_to, ServiceError::PythonUnavailable { .. }));

    // Missing interpreter
    let runner_missing = FakePyRunner {
        last_spec: Mutex::new(None),
        mode: "missing",
    };
    let err_miss = run_python_service(
        "my.fit",
        "pkg.mod:fit",
        &["wall_s".to_string()],
        &["alpha".to_string()],
        &frame,
        &Options::default(),
        &cfg,
        &dir,
        &runner_missing,
    )
    .unwrap_err();
    assert!(matches!(err_miss, ServiceError::PythonUnavailable { .. }));

    let _ = fs::remove_dir_all(&dir);
}
