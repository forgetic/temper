// SPDX-License-Identifier: MPL-2.0

#[path = "temper-scenario/focused_validation.rs"]
mod focused_validation;
#[path = "temper-scenario/manifest_executor.rs"]
mod manifest_executor;
#[path = "temper-scenario/manifest_runner.rs"]
mod manifest_runner;
#[path = "temper-scenario/promote.rs"]
mod promote;
#[path = "temper-scenario/resolve_feature.rs"]
mod resolve_feature;
#[path = "temper-scenario/run_context.rs"]
mod run_context;
#[path = "temper-scenario/run_evidence.rs"]
mod run_evidence;
#[path = "temper-scenario/runner_registry.rs"]
mod runner_registry;
#[path = "temper-scenario/scaffold.rs"]
mod scaffold;
#[path = "temper-scenario/validate.rs"]
mod validate;
#[path = "temper-scenario/validate_pr.rs"]
mod validate_pr;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use temper_scenario_core::{
    DEFAULT_SCENARIOS_DIR, Diagnostic, Severity, check_scenario, check_scenarios,
    discover_scenarios,
};

use run_context::{ScenarioRunFacts, ScenarioTier};

const EX_USAGE: u8 = 64;
const RUN_EVIDENCE_FILE: &str = "run-evidence.json";

const USAGE: &str = "\
temper-scenario: list, check, scaffold, resolve, run, and validate Temper scenario artifacts

Usage: temper-scenario <COMMAND> [OPTIONS]

Commands:
  list              List scenario directories and stable manifest metadata
  check             Validate one scenario path or all scenarios under a scenarios directory
  scaffold          Create a minimal inherited feature scenario with local Jig data
  resolve-feature   Resolve one active feature-mapped scenario and emit deterministic JSON
  run               Run a supported scenario at an explicit confidence tier
  validate          Run a scenario bundle and render validation artifacts from structured evidence
  validate-feature  Resolve and run one mapped scenario at an exact feature-landing head
  validate-pr       Write a temporary post-merge PR validation Markdown report
  promote           Draft an optional scenario-promotion candidate from validation artifacts

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
Run a supported Temper scenario at an explicit confidence tier.

Usage: temper-scenario run [--tier <live|hermetic>] [--temper-bin <PATH>] [--evidence-out <PATH>] <SCENARIO_PATH>

Arguments:
  SCENARIO_PATH  Scenario directory or manifest file to run

Options:
  --tier <live|hermetic>  Confidence tier to request (default: live)
  --temper-bin <PATH>    Standalone `temper` binary for --tier live
  --evidence-out <PATH>  Write structured JSON run evidence to PATH
  -h, --help             Print help

The only registered scenario runner is `manifest`, which boots the validation-grade
stack: real Forgejo + real forgejo-runner CI + real Temper + Jig fake LLM. It is
live-only and rejects hermetic, MemoryForge, or in-process substitutes instead
of falling back.

For live manifests, pass --temper-bin <PATH>, set TEMPER_SCENARIO_TEMPER_BIN,
or prebuild a sibling target-dir `temper`; `cargo dev-scenario-run <path>` builds it.\nManifests must select `[runner] uses = \"manifest\"`; the legacy manifest `name` fallback has been removed.";

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
        "scaffold" => scaffold::command(rest),
        "resolve-feature" => resolve_feature::command(rest),
        "run" => run_command(rest),
        "validate" => validate::command(rest),
        "validate-feature" => focused_validation::command(rest),
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
    let args = match parse_run_args(args) {
        Ok(RunParseResult::Help) => {
            println!("{RUN_USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(RunParseResult::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    let report = check_scenario(&args.path);
    if !report.is_valid() {
        print_report_diagnostics(&report);
        return ExitCode::FAILURE;
    }

    let Some(manifest) = report.manifest.as_ref() else {
        eprintln!(
            "temper-scenario run: no runnable scenario manifest found at {}",
            display_path(&args.path)
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

    let facts = ScenarioRunFacts::from_check_report(&report, args.tier);

    let selected_runner = match runner_registry::select_runner(manifest, args.tier) {
        Ok(runner) => runner,
        Err(error) => {
            eprintln!(
                "temper-scenario run: {}",
                error.message(&report.scenario_path)
            );
            return ExitCode::FAILURE;
        }
    };

    let evidence_context =
        run_evidence::RunEvidenceContext::from_check_report(&report, &facts, &selected_runner);
    let started = Instant::now();
    let result = selected_runner.run_and_print(
        &report.scenario_path,
        manifest_path,
        &facts,
        args.tier,
        args.temper_bin.as_deref(),
        &evidence_context,
    );

    match result {
        Ok(mut artifact) => {
            let assertion_evidence =
                match run_evidence::evaluate_manifest_assertions(manifest_path, &artifact) {
                    Ok(assertions) => assertions,
                    Err(error) => {
                        eprintln!("temper-scenario run: evaluate manifest assertions: {error}");
                        return ExitCode::FAILURE;
                    }
                };
            if let Some(assertions) = assertion_evidence {
                artifact.record_assertions(assertions);
            }

            let artifact_dir = run_artifact_dir(args.evidence_out.as_deref(), &manifest.name);
            if let Err(error) =
                run_evidence::append_script_assertions(manifest_path, &mut artifact, &artifact_dir)
            {
                eprintln!("temper-scenario run: evaluate script assertions: {error}");
                return ExitCode::FAILURE;
            }

            let assertions_failed = artifact
                .assertions
                .as_ref()
                .is_some_and(|assertions| assertions.has_failures());
            if let Some(assertions) = artifact.assertions.as_ref() {
                run_evidence::print_assertions(assertions);
            }
            let evidence_destination = args
                .evidence_out
                .clone()
                .or_else(|| assertions_failed.then(|| artifact_dir.join(RUN_EVIDENCE_FILE)));
            if let Some(path) = evidence_destination.as_deref() {
                artifact
                    .artifacts
                    .artifact_paths
                    .push(resolved_run_evidence_output(path).display().to_string());
                match artifact.write_to_path(path) {
                    Ok(path) => println!("run evidence: {}", path.display()),
                    Err(error) => {
                        eprintln!("temper-scenario run: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            if assertions_failed {
                eprintln!("temper-scenario run: manifest assertions failed");
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let mut artifact = evidence_context.failure_artifact(error.clone(), elapsed_ms);
            match run_evidence::evaluate_manifest_assertions(manifest_path, &artifact) {
                Ok(Some(assertions)) => artifact.record_assertions(assertions),
                Ok(None) => {}
                Err(assertion_error) => artifact.limitations.push(format!(
                    "Declarative assertions could not be evaluated after execution failure: {assertion_error}"
                )),
            }
            artifact.limitations.push(
                "After-convergence script assertions were not executed because convergence did not complete."
                    .to_string(),
            );
            let path = args
                .evidence_out
                .clone()
                .unwrap_or_else(|| run_artifact_dir(None, &manifest.name).join(RUN_EVIDENCE_FILE));
            artifact
                .artifacts
                .artifact_paths
                .push(resolved_run_evidence_output(&path).display().to_string());
            match artifact.write_to_path(&path) {
                Ok(path) => eprintln!(
                    "temper-scenario run: failure evidence retained at {}",
                    path.display()
                ),
                Err(write_error) => eprintln!(
                    "temper-scenario run: could not retain failure evidence: {write_error}"
                ),
            }
            eprintln!("temper-scenario run: {error}");
            ExitCode::FAILURE
        }
    }
}

fn resolved_run_evidence_output(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join(RUN_EVIDENCE_FILE)
    } else {
        path.to_path_buf()
    }
}

fn run_artifact_dir(evidence_out: Option<&Path>, scenario_name: &str) -> PathBuf {
    if let Some(path) = evidence_out {
        if path.is_dir() {
            return path.to_path_buf();
        }
        return path
            .parent()
            .map(Path::to_path_buf)
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("."));
    }

    let root = env::current_dir()
        .ok()
        .and_then(|current| scenario_workspace_root(&current))
        .map(|root| root.join("target"))
        .unwrap_or_else(std::env::temp_dir);
    root.join("temper-scenario-artifacts").join(format!(
        "{}-{}",
        safe_file_component(scenario_name),
        std::process::id()
    ))
}

fn scenario_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        if current.join("Cargo.toml").is_file() && current.join("scenarios").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn safe_file_component(value: &str) -> String {
    let safe = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "scenario".to_string()
    } else {
        safe
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

#[derive(Debug, Clone, Eq, PartialEq)]
struct RunArgs {
    path: PathBuf,
    tier: ScenarioTier,
    temper_bin: Option<PathBuf>,
    evidence_out: Option<PathBuf>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum RunParseResult {
    Help,
    Args(RunArgs),
}

fn parse_run_args(args: &[String]) -> Result<RunParseResult, ()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(RunParseResult::Help);
    }

    let mut path = None;
    let mut tier = None;
    let mut temper_bin = None;
    let mut evidence_out = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--tier" {
            let value = run_flag_value(args, index, "--tier")?;
            set_run_tier(&mut tier, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--tier=") {
            if value.is_empty() {
                eprintln!("temper-scenario run: --tier requires a value\n\n{RUN_USAGE}");
                return Err(());
            }
            set_run_tier(&mut tier, value)?;
            index += 1;
            continue;
        }
        if arg == "--temper-bin" {
            let value = run_flag_value(args, index, "--temper-bin")?;
            set_temper_bin(&mut temper_bin, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--temper-bin=") {
            if value.is_empty() {
                eprintln!("temper-scenario run: --temper-bin requires a value\n\n{RUN_USAGE}");
                return Err(());
            }
            set_temper_bin(&mut temper_bin, value)?;
            index += 1;
            continue;
        }
        if arg == "--evidence-out" {
            let value = run_flag_value(args, index, "--evidence-out")?;
            set_evidence_out(&mut evidence_out, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--evidence-out=") {
            if value.is_empty() {
                eprintln!("temper-scenario run: --evidence-out requires a value\n\n{RUN_USAGE}");
                return Err(());
            }
            set_evidence_out(&mut evidence_out, value)?;
            index += 1;
            continue;
        }
        if arg.starts_with("--") {
            eprintln!("temper-scenario run: unexpected option `{arg}`\n\n{RUN_USAGE}");
            return Err(());
        }
        if path.replace(PathBuf::from(arg)).is_some() {
            eprintln!("temper-scenario run: unexpected argument `{arg}`\n\n{RUN_USAGE}");
            return Err(());
        }
        index += 1;
    }

    let Some(path) = path else {
        eprintln!("temper-scenario run: missing SCENARIO_PATH\n\n{RUN_USAGE}");
        return Err(());
    };

    Ok(RunParseResult::Args(RunArgs {
        path,
        tier: tier.unwrap_or(ScenarioTier::Live),
        temper_bin,
        evidence_out,
    }))
}

fn run_flag_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str, ()> {
    let Some(value) = args.get(index + 1) else {
        eprintln!("temper-scenario run: {flag} requires a value\n\n{RUN_USAGE}");
        return Err(());
    };
    if value.starts_with("--") {
        eprintln!("temper-scenario run: {flag} requires a value\n\n{RUN_USAGE}");
        return Err(());
    }
    Ok(value)
}

fn set_run_tier(tier: &mut Option<ScenarioTier>, value: &str) -> Result<(), ()> {
    let Some(parsed) = ScenarioTier::parse(value) else {
        eprintln!(
            "temper-scenario run: unknown --tier `{value}` (expected live or hermetic)\n\n{RUN_USAGE}"
        );
        return Err(());
    };
    if tier.replace(parsed).is_some() {
        eprintln!("temper-scenario run: duplicate --tier option\n\n{RUN_USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_temper_bin(temper_bin: &mut Option<PathBuf>, value: &str) -> Result<(), ()> {
    if temper_bin.replace(PathBuf::from(value)).is_some() {
        eprintln!("temper-scenario run: duplicate --temper-bin option\n\n{RUN_USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_evidence_out(evidence_out: &mut Option<PathBuf>, value: &str) -> Result<(), ()> {
    if evidence_out.replace(PathBuf::from(value)).is_some() {
        eprintln!("temper-scenario run: duplicate --evidence-out option\n\n{RUN_USAGE}");
        return Err(());
    }
    Ok(())
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
