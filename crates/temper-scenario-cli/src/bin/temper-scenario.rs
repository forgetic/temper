// SPDX-License-Identifier: MPL-2.0

#[path = "temper-scenario/basic_delivery.rs"]
mod basic_delivery;
#[path = "temper-scenario/implementation_pr_handoff.rs"]
mod implementation_pr_handoff;
#[path = "temper-scenario/promote.rs"]
mod promote;
#[path = "temper-scenario/validate_pr.rs"]
mod validate_pr;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use temper_scenario_core::{
    DEFAULT_SCENARIOS_DIR, Diagnostic, Severity, check_scenario, check_scenarios,
    discover_scenarios,
};

const EX_USAGE: u8 = 64;

const USAGE: &str = "\
temper-scenario: list, check, run, validate, and draft Temper scenario artifacts

Usage: temper-scenario <COMMAND> [OPTIONS]

Commands:
  list         List scenario directories and stable manifest metadata
  check        Validate one scenario path or all scenarios under a scenarios directory
  run          Run a supported scenario deterministically in process
  validate-pr  Write a temporary post-merge PR validation Markdown report
  promote      Draft an optional scenario-promotion candidate from validation artifacts

Options:
  -h, --help  Print help

Run `temper-scenario <command> --help` for command usage.";

const LIST_USAGE: &str = "\
List scenario directories and stable manifest metadata.

Usage: temper-scenario list [SCENARIOS_DIR]

Arguments:
  SCENARIOS_DIR  Directory containing scenario subdirectories (default: scenarios)

Output columns are tab-separated: name, status, stability, intent, path.";

const CHECK_USAGE: &str = "\
Validate Temper scenario manifests.

Usage: temper-scenario check [PATH]

Arguments:
  PATH  Scenario directory, manifest file, or scenarios root (default: scenarios)

A directory with its own manifest is checked as one scenario. Other directories
are scanned for immediate child scenario directories. Diagnostics are printed in
a concise `path: error: field: message` form suitable for CI logs.";

const RUN_USAGE: &str = "\
Run a supported Temper scenario deterministically in process.

Usage: temper-scenario run <SCENARIO_PATH>

Arguments:
  SCENARIO_PATH  Scenario directory or manifest file to run

Supported checked-in runners are scenarios/basic-delivery and
scenarios/implementation-pr-handoff. Unsupported scenario manifests fail clearly
instead of being treated as passed.";

fn main() -> ExitCode {
    run(env::args().skip(1))
}

fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let args = args.into_iter().collect::<Vec<_>>();
    let Some((command, rest)) = args.split_first() else {
        println!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    };

    match command.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "list" => list_command(rest),
        "check" => check_command(rest),
        "run" => run_command(rest),
        "validate-pr" => validate_pr::command(rest),
        "promote" => promote::command(rest),
        other => {
            eprintln!("temper-scenario: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn list_command(args: &[String]) -> ExitCode {
    let root = match parse_optional_path(args, LIST_USAGE, "temper-scenario list") {
        Ok(CommandPath::Help) => {
            println!("{LIST_USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(CommandPath::Path { path, explicit }) => {
            if explicit && !path.exists() && path != PathBuf::from(DEFAULT_SCENARIOS_DIR) {
                eprintln!(
                    "temper-scenario list: scenario root does not exist: {}",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
            path
        }
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    let entries = match discover_scenarios(&root) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("temper-scenario list: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("name\tstatus\tstability\tintent\tpath");
    let mut had_error = false;
    for entry in entries {
        let report = check_scenario(&entry.scenario_path);
        if let Some(manifest) = report.manifest {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                sanitize(&manifest.name),
                manifest.status,
                manifest.stability,
                sanitize(&manifest.intent.display_value()),
                display_path(&entry.scenario_path)
            );
        } else {
            had_error = true;
            print_report_diagnostics(&report);
        }
    }

    if had_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn check_command(args: &[String]) -> ExitCode {
    let (path, explicit) = match parse_optional_path(args, CHECK_USAGE, "temper-scenario check") {
        Ok(CommandPath::Help) => {
            println!("{CHECK_USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(CommandPath::Path { path, explicit }) => (path, explicit),
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    let reports = if path.exists()
        && (path.is_file() || temper_scenario_core::resolve_manifest_path(&path).is_ok())
    {
        vec![check_scenario(&path)]
    } else if !path.exists() {
        if explicit && path != PathBuf::from(DEFAULT_SCENARIOS_DIR) {
            vec![check_scenario(&path)]
        } else {
            match check_scenarios(&path) {
                Ok(reports) => reports,
                Err(error) => {
                    eprintln!("temper-scenario check: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
    } else {
        match check_scenarios(&path) {
            Ok(reports) if explicit && reports.is_empty() => vec![check_scenario(&path)],
            Ok(reports) => reports,
            Err(error) => {
                eprintln!("temper-scenario check: {error}");
                return ExitCode::FAILURE;
            }
        }
    };

    let mut checked = 0usize;
    let mut had_error = false;
    for report in &reports {
        checked += 1;
        if report.is_valid() {
            continue;
        }
        had_error = true;
        print_report_diagnostics(report);
    }

    if had_error {
        ExitCode::FAILURE
    } else {
        println!("OK - checked {checked} scenario(s).");
        ExitCode::SUCCESS
    }
}

fn run_command(args: &[String]) -> ExitCode {
    let path = match parse_required_path(args, RUN_USAGE, "temper-scenario run") {
        Ok(CommandPath::Help) => {
            println!("{RUN_USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(CommandPath::Path { path, .. }) => path,
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    let report = check_scenario(&path);
    if !report.is_valid() {
        print_report_diagnostics(&report);
        return ExitCode::FAILURE;
    }

    let Some(manifest) = report.manifest.as_ref() else {
        eprintln!(
            "temper-scenario run: no runnable scenario manifest found at {}",
            display_path(&path)
        );
        return ExitCode::FAILURE;
    };
    let Some(manifest_path) = report.manifest_path.as_deref() else {
        eprintln!(
            "temper-scenario run: no scenario manifest found at {}",
            display_path(&report.scenario_path)
        );
        return ExitCode::FAILURE;
    };

    let result = match manifest.name.as_str() {
        basic_delivery::SCENARIO_NAME => {
            basic_delivery::run_and_print(&report.scenario_path, manifest_path)
        }
        implementation_pr_handoff::SCENARIO_NAME => {
            implementation_pr_handoff::run_and_print(&report.scenario_path, manifest_path)
        }
        other => {
            eprintln!(
                "temper-scenario run: unsupported scenario `{other}` at {}; supported checked-in runners: scenarios/basic-delivery, scenarios/implementation-pr-handoff",
                display_path(&report.scenario_path)
            );
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper-scenario run: {error}");
            ExitCode::FAILURE
        }
    }
}

enum CommandPath {
    Help,
    Path { path: PathBuf, explicit: bool },
}

fn parse_optional_path(
    args: &[String],
    usage: &str,
    command_name: &str,
) -> Result<CommandPath, ()> {
    match args {
        [] => Ok(CommandPath::Path {
            path: PathBuf::from(DEFAULT_SCENARIOS_DIR),
            explicit: false,
        }),
        [arg] if matches!(arg.as_str(), "-h" | "--help" | "help") => Ok(CommandPath::Help),
        [path] => Ok(CommandPath::Path {
            path: PathBuf::from(path),
            explicit: true,
        }),
        [arg, ..] => {
            eprintln!("{command_name}: unexpected argument `{arg}`\n\n{usage}");
            Err(())
        }
    }
}

fn parse_required_path(
    args: &[String],
    usage: &str,
    command_name: &str,
) -> Result<CommandPath, ()> {
    match args {
        [] => {
            eprintln!("{command_name}: missing SCENARIO_PATH\n\n{usage}");
            Err(())
        }
        [arg] if matches!(arg.as_str(), "-h" | "--help" | "help") => Ok(CommandPath::Help),
        [path] => Ok(CommandPath::Path {
            path: PathBuf::from(path),
            explicit: true,
        }),
        [arg, ..] => {
            eprintln!("{command_name}: unexpected argument `{arg}`\n\n{usage}");
            Err(())
        }
    }
}

fn print_report_diagnostics(report: &temper_scenario_core::CheckReport) {
    let path = report
        .manifest_path
        .as_deref()
        .unwrap_or(report.scenario_path.as_path());
    for diagnostic in &report.diagnostics {
        print_diagnostic(path, diagnostic);
    }
}

fn print_diagnostic(path: &Path, diagnostic: &Diagnostic) {
    let prefix = display_path(path);
    match diagnostic.severity {
        Severity::Error | Severity::Warning => eprintln!("{prefix}: {diagnostic}"),
    }
}

fn sanitize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\t', " ")
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::run;

    #[test]
    fn top_level_help_succeeds() {
        assert_eq!(run(["--help".to_string()]), ExitCode::SUCCESS);
    }

    #[test]
    fn unknown_command_is_usage_error() {
        assert_eq!(run(["wat".to_string()]), ExitCode::from(64));
    }

    #[test]
    fn run_without_path_is_usage_error() {
        assert_eq!(run(["run".to_string()]), ExitCode::from(64));
    }
}
