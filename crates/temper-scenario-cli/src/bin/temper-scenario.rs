// SPDX-License-Identifier: MPL-2.0

#[path = "temper-scenario/basic_delivery.rs"]
mod basic_delivery;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use temper_scenario_core::{
    AcceptanceCriterion, DEFAULT_SCENARIOS_DIR, Diagnostic, EvidenceEntry, EvidenceKind, Severity,
    ValidatedClaim, ValidationReport, ValidationStatus, ValidationVerdict, check_scenario,
    check_scenarios, discover_scenarios,
};

const EX_USAGE: u8 = 64;

const USAGE: &str = "\
temper-scenario: list, check, run, and validate Temper executable scenario manifests

Usage: temper-scenario <COMMAND> [OPTIONS]

Commands:
  list         List scenario directories and stable manifest metadata
  check        Validate one scenario path or all scenarios under a scenarios directory
  run          Run a supported scenario deterministically in process
  validate-pr  Write a temporary post-merge PR validation Markdown report

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

This first runner supports only the checked-in scenarios/basic-delivery shape.
Unsupported scenario manifests fail clearly instead of being treated as passed.";

const VALIDATE_PR_USAGE: &str = "\
Write a temporary/manual post-merge validation Markdown report.

Usage: temper-scenario validate-pr --pr <N> --sha <SHA> [--scenario <PATH>] [--output-dir <DIR>]

Options:
  --pr <N>          Pull request number under validation
  --sha <SHA>       Merged/main commit SHA under validation
  --scenario <PATH> Scenario directory or manifest file to check, and run when supported
  --output-dir <DIR> Directory for the Markdown report (default: current directory)
  -h, --help        Print help

The bridge records local scenario evidence when available. It does not fetch live
Forgejo PR context or prove that the supplied SHA is the current main commit.";

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
        "validate-pr" => validate_pr_command(rest),
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
    if manifest.name != basic_delivery::SCENARIO_NAME {
        eprintln!(
            "temper-scenario run: unsupported scenario `{}` at {}; this first runner supports only scenarios/basic-delivery",
            manifest.name,
            display_path(&report.scenario_path)
        );
        return ExitCode::FAILURE;
    }
    let Some(manifest_path) = report.manifest_path.as_deref() else {
        eprintln!(
            "temper-scenario run: no scenario manifest found at {}",
            display_path(&report.scenario_path)
        );
        return ExitCode::FAILURE;
    };

    match basic_delivery::run_and_print(&report.scenario_path, manifest_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper-scenario run: {error}");
            ExitCode::FAILURE
        }
    }
}

fn validate_pr_command(args: &[String]) -> ExitCode {
    let args = match parse_validate_pr_args(args) {
        Ok(ValidatePrParse::Help) => {
            println!("{VALIDATE_PR_USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ValidatePrParse::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    let output_path = validation_report_path(&args.output_dir, args.pr_number, &args.sha);
    let report = match build_validation_report(&args, &output_path) {
        Ok(report) => report,
        Err(ValidatePrError::InvalidScenario(report)) => {
            eprintln!(
                "temper-scenario validate-pr: scenario check failed for {}",
                display_path(&report.scenario_path)
            );
            print_report_diagnostics(&report);
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = write_validation_report(&output_path, &report) {
        eprintln!("temper-scenario validate-pr: {error}");
        return ExitCode::FAILURE;
    }

    println!("{}", output_path.display());
    ExitCode::SUCCESS
}

#[derive(Debug)]
struct ValidatePrArgs {
    pr_number: u64,
    sha: String,
    scenario: Option<PathBuf>,
    output_dir: PathBuf,
}

#[derive(Debug)]
enum ValidatePrParse {
    Help,
    Args(ValidatePrArgs),
}

#[derive(Debug)]
enum ValidatePrError {
    InvalidScenario(temper_scenario_core::CheckReport),
}

fn parse_validate_pr_args(args: &[String]) -> Result<ValidatePrParse, ()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ValidatePrParse::Help);
    }

    let mut pr_number = None;
    let mut sha = None;
    let mut scenario = None;
    let mut output_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pr" => {
                let value = flag_value(args, index, "--pr")?;
                let parsed = value.parse::<u64>().ok().filter(|number| *number > 0);
                let Some(parsed) = parsed else {
                    eprintln!(
                        "temper-scenario validate-pr: --pr must be a positive integer: {value}\n\n{VALIDATE_PR_USAGE}"
                    );
                    return Err(());
                };
                if pr_number.replace(parsed).is_some() {
                    eprintln!(
                        "temper-scenario validate-pr: duplicate --pr option\n\n{VALIDATE_PR_USAGE}"
                    );
                    return Err(());
                }
                index += 2;
            }
            "--sha" => {
                let value = flag_value(args, index, "--sha")?;
                if value.trim().is_empty() {
                    eprintln!(
                        "temper-scenario validate-pr: --sha must not be empty\n\n{VALIDATE_PR_USAGE}"
                    );
                    return Err(());
                }
                if sha.replace(value.to_string()).is_some() {
                    eprintln!(
                        "temper-scenario validate-pr: duplicate --sha option\n\n{VALIDATE_PR_USAGE}"
                    );
                    return Err(());
                }
                index += 2;
            }
            "--scenario" => {
                let value = flag_value(args, index, "--scenario")?;
                if scenario.replace(PathBuf::from(value)).is_some() {
                    eprintln!(
                        "temper-scenario validate-pr: duplicate --scenario option\n\n{VALIDATE_PR_USAGE}"
                    );
                    return Err(());
                }
                index += 2;
            }
            "--output-dir" => {
                let value = flag_value(args, index, "--output-dir")?;
                if output_dir.replace(PathBuf::from(value)).is_some() {
                    eprintln!(
                        "temper-scenario validate-pr: duplicate --output-dir option\n\n{VALIDATE_PR_USAGE}"
                    );
                    return Err(());
                }
                index += 2;
            }
            other => {
                eprintln!(
                    "temper-scenario validate-pr: unexpected argument `{other}`\n\n{VALIDATE_PR_USAGE}"
                );
                return Err(());
            }
        }
    }

    let Some(pr_number) = pr_number else {
        eprintln!("temper-scenario validate-pr: missing --pr\n\n{VALIDATE_PR_USAGE}");
        return Err(());
    };
    let Some(sha) = sha else {
        eprintln!("temper-scenario validate-pr: missing --sha\n\n{VALIDATE_PR_USAGE}");
        return Err(());
    };

    Ok(ValidatePrParse::Args(ValidatePrArgs {
        pr_number,
        sha,
        scenario,
        output_dir: output_dir.unwrap_or_else(|| PathBuf::from(".")),
    }))
}

fn flag_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str, ()> {
    let Some(value) = args.get(index + 1) else {
        eprintln!("temper-scenario validate-pr: {flag} requires a value\n\n{VALIDATE_PR_USAGE}");
        return Err(());
    };
    if value.starts_with("--") {
        eprintln!("temper-scenario validate-pr: {flag} requires a value\n\n{VALIDATE_PR_USAGE}");
        return Err(());
    }
    Ok(value)
}

fn build_validation_report(
    args: &ValidatePrArgs,
    output_path: &Path,
) -> Result<ValidationReport, ValidatePrError> {
    let mut report = ValidationReport::new(
        args.pr_number,
        args.sha.clone(),
        ValidationVerdict::Inconclusive,
    );

    report.evidence.push(
        EvidenceEntry::new(
            EvidenceKind::Command,
            "validate-pr invoked with operator-supplied PR and SHA inputs.",
        )
        .with_detail(format!("pr: #{}", args.pr_number))
        .with_detail(format!("sha: `{}`", args.sha)),
    );
    report.evidence.push(EvidenceEntry::new(
        EvidenceKind::Artifact,
        format!("Markdown report artifact path: `{}`", output_path.display()),
    ));
    report.validated_claims.push(
        ValidatedClaim::new(
            format!(
                "PR #{} is the merged change at `{}`.",
                args.pr_number, args.sha
            ),
            ValidationStatus::Unproven,
        )
        .with_evidence("The temporary harness accepts PR/SHA as operator input only."),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "Live Forgejo PR state and the current main commit match the supplied identifiers.",
            ValidationStatus::Unproven,
        )
        .with_evidence("No live Forgejo lookup is performed by this temporary bridge."),
    );
    report.limitations.push(format!(
        "Temporary validate-pr does not fetch Forgejo PR #{} or confirm that `{}` is the current main SHA.",
        args.pr_number, args.sha
    ));

    match args.scenario.as_deref() {
        Some(path) => add_scenario_validation(&mut report, path)?,
        None => {
            report.validated_claims.push(
                ValidatedClaim::new(
                    "No scenario path was supplied for local post-merge validation.",
                    ValidationStatus::Unproven,
                )
                .with_evidence(
                    "Run with --scenario <PATH> to collect scenario check/run evidence.",
                ),
            );
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    "A supplied scenario path is checked and, when supported, run deterministically.",
                    ValidationStatus::Unproven,
                )
                .with_evidence("No --scenario argument was provided."),
            );
            report.evidence.push(EvidenceEntry::new(
                EvidenceKind::Observation,
                "No scenario path was supplied, so no scenario check or run evidence was collected.",
            ));
            report.limitations.push(
                "No scenario path was supplied; the report contains no scenario check/run evidence."
                    .to_string(),
            );
        }
    }

    Ok(report)
}

fn add_scenario_validation(
    report: &mut ValidationReport,
    path: &Path,
) -> Result<(), ValidatePrError> {
    let check_report = check_scenario(path);
    if !check_report.is_valid() {
        return Err(ValidatePrError::InvalidScenario(check_report));
    }

    let scenario_name = check_report
        .manifest
        .as_ref()
        .map(|manifest| manifest.name.as_str())
        .unwrap_or("unknown");
    report.validated_claims.push(
        ValidatedClaim::new(
            format!(
                "Scenario `{scenario_name}` manifest validates at `{}`.",
                display_path(&check_report.scenario_path)
            ),
            ValidationStatus::Observed,
        )
        .with_evidence("scenario check passed"),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "The supplied scenario path resolves to a valid scenario manifest.",
            ValidationStatus::Satisfied,
        )
        .with_evidence(format!(
            "checked `{}`",
            display_path(&check_report.scenario_path)
        )),
    );
    let mut check_evidence = EvidenceEntry::new(
        EvidenceKind::ScenarioCheck,
        format!(
            "Scenario check passed for `{}`.",
            display_path(&check_report.scenario_path)
        ),
    )
    .with_detail(format!("scenario: `{scenario_name}`"));
    if let Some(manifest_path) = check_report.manifest_path.as_deref() {
        check_evidence =
            check_evidence.with_detail(format!("manifest: `{}`", display_path(manifest_path)));
    }
    report.evidence.push(check_evidence);

    if scenario_name == basic_delivery::SCENARIO_NAME {
        let Some(manifest_path) = check_report.manifest_path.as_deref() else {
            report.limitations.push(format!(
                "Scenario `{scenario_name}` had no resolved manifest path, so no scenario run occurred."
            ));
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    "The supported deterministic scenario completes successfully.",
                    ValidationStatus::Unproven,
                )
                .with_evidence("No manifest path was available for the scenario runner."),
            );
            return Ok(());
        };
        match basic_delivery::run_evidence_lines(&check_report.scenario_path, manifest_path) {
            Ok(lines) => {
                report.validated_claims.push(
                    ValidatedClaim::new(
                        "Supported deterministic basic-delivery scenario completes successfully.",
                        ValidationStatus::Observed,
                    )
                    .with_evidence("scenario run passed"),
                );
                report.acceptance_criteria.push(
                    AcceptanceCriterion::new(
                        "A supported deterministic scenario run completes successfully.",
                        ValidationStatus::Satisfied,
                    )
                    .with_evidence("basic-delivery run completed in process"),
                );
                report.evidence.push(
                    EvidenceEntry::new(
                        EvidenceKind::ScenarioRun,
                        "Deterministic basic-delivery scenario run completed successfully.",
                    )
                    .with_details(lines),
                );
            }
            Err(error) => {
                report.verdict = ValidationVerdict::Failed;
                report.validated_claims.push(
                    ValidatedClaim::new(
                        "Supported deterministic basic-delivery scenario completes successfully.",
                        ValidationStatus::Failed,
                    )
                    .with_evidence(error.clone()),
                );
                report.acceptance_criteria.push(
                    AcceptanceCriterion::new(
                        "A supported deterministic scenario run completes successfully.",
                        ValidationStatus::Failed,
                    )
                    .with_evidence(error.clone()),
                );
                report.evidence.push(
                    EvidenceEntry::new(EvidenceKind::ScenarioRun, "Scenario run failed.")
                        .with_detail(error),
                );
            }
        }
    } else {
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(
                "A supported deterministic scenario run completes successfully.",
                ValidationStatus::NotApplicable,
            )
            .with_evidence(format!(
                "scenario `{scenario_name}` is not supported by this temporary runner"
            )),
        );
        report.limitations.push(format!(
            "No scenario run occurred for `{scenario_name}`; this temporary runner supports only scenarios/basic-delivery."
        ));
    }

    Ok(())
}

fn validation_report_path(output_dir: &Path, pr_number: u64, sha: &str) -> PathBuf {
    output_dir.join(format!(
        "validation-pr-{pr_number}-{}.md",
        safe_file_component(sha)
    ))
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
        "sha".to_string()
    } else {
        safe
    }
}

fn write_validation_report(path: &Path, report: &ValidationReport) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(format!("report path has no parent: {}", path.display()));
    };
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create output directory {}: {error}",
            parent.display()
        )
    })?;
    fs::write(path, report.render_markdown())
        .map_err(|error| format!("failed to write report {}: {error}", path.display()))
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
