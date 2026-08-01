// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use temper_scenario_core::{DEFAULT_SCENARIOS_DIR, check_scenarios};

const USAGE: &str = "Usage: temper-scenario-check [SCENARIOS_DIR]";

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SCENARIOS_DIR));
    if args.next().is_some() {
        eprintln!("{USAGE}");
        return ExitCode::from(64);
    }

    let reports = match check_scenarios(&path) {
        Ok(reports) => reports,
        Err(error) => {
            eprintln!("temper-scenario-check: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut had_error = false;
    for report in &reports {
        if report.is_valid() {
            continue;
        }
        had_error = true;
        let manifest = report
            .manifest_path
            .as_deref()
            .unwrap_or(&report.scenario_path);
        for diagnostic in &report.diagnostics {
            eprintln!("{}: {diagnostic}", manifest.display());
        }
    }

    if had_error {
        ExitCode::FAILURE
    } else {
        println!("OK - checked {} scenario(s).", reports.len());
        ExitCode::SUCCESS
    }
}
