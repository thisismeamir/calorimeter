//! Out-of-process Python service and plot runner (spec §12.4 R-PY-1..3, §16 R-SAF-5).

pub mod runner;

pub use runner::{detect_python, run_python_plot, run_python_service, PythonRunnerConfig};
