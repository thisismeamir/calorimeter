//! `probe` binary CLI entrypoint (Spec §9.0, §9.9).

#![forbid(unsafe_code)]

use std::{env, fs, process::ExitCode};

use calo_probe::probe::Probe;
use calo_schema::runspec::RunSpec;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut runspec_path: Option<String> = None;
    let mut out_path: Option<String> = None;
    let mut discover = false;
    let mut version = false;
    let mut rank_aware = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--runspec" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    runspec_path = Some(p.clone());
                } else {
                    eprintln!("probe: --runspec requires a path");
                    return ExitCode::from(70);
                }
            }
            "--discover" => discover = true,
            "--version" => version = true,
            "--rank-aware" => rank_aware = true,
            "--out" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    out_path = Some(p.clone());
                } else {
                    eprintln!("probe: --out requires a path");
                    return ExitCode::from(70);
                }
            }
            other => {
                eprintln!("probe: unknown argument '{other}'");
                return ExitCode::from(70);
            }
        }
        i += 1;
    }

    if version {
        println!("calo-probe {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    if discover {
        let disc = match Probe::discover(&calo_probe::discover::DiscoverOpts::default()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("probe: discovery failed: {e}");
                return ExitCode::from(70);
            }
        };
        let yaml = match calo_schema::to_yaml_string(&disc) {
            Ok(y) => y,
            Err(e) => {
                eprintln!("probe: failed to serialize discovery YAML: {e}");
                return ExitCode::from(70);
            }
        };
        if let Some(out) = out_path {
            if let Err(e) = fs::write(&out, yaml) {
                eprintln!("probe: failed to write '{out}': {e}");
                return ExitCode::from(70);
            }
        } else {
            print!("{yaml}");
        }
        return ExitCode::SUCCESS;
    }
    let Some(path) = runspec_path else {
        eprintln!("probe: missing --runspec <file>");
        return ExitCode::from(70);
    };

    let content = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("probe: failed to read runspec '{path}': {e}");
            return ExitCode::from(70);
        }
    };

    let spec: RunSpec = match serde_yaml_ng::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("probe: invalid runspec YAML '{path}': {e}");
            return ExitCode::from(70);
        }
    };
    let opts = calo_probe::probe::ProbeOptions {
        rank_aware,
        ..Default::default()
    };
    match Probe::run_with_options(&spec, &opts) {
        Ok(outcome) => ExitCode::from((outcome.exit_code & 0xff) as u8),
        Err(e) => {
            eprintln!("probe: fatal error: {e}");
            ExitCode::from(70)
        }
    }
}
