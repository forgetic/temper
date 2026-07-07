// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use temper_scenario_core::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, ValidatedClaim, ValidationReport,
    ValidationStatus, ValidationVerdict, check_scenario,
};

use super::run_context::{ScenarioRunFacts, ScenarioTier};

#[path = "validate_pr/live.rs"]
mod live;
#[path = "validate_pr/run_evidence.rs"]
mod run_evidence;
#[path = "validate_pr/runner.rs"]
mod runner;

const EX_USAGE: u8 = 64;

const USAGE: &str = "\
Write a temporary/manual post-merge validation Markdown report.

Usage: temper-scenario validate-pr --pr <N> --sha <SHA> [--scenario <PATH>] [--run-evidence <PATH>] [--tier <live|hermetic>] [--temper-bin <PATH>] [--output-dir <DIR>]

Options:
  --pr <N>             Pull request number under validation
  --sha <SHA>          Merged/main commit SHA under validation
  --scenario <PATH>     Scenario directory or manifest file to check, and run when supported unless --run-evidence is supplied
  --run-evidence <PATH>  Previous run-evidence JSON file or directory to cite instead of rerunning scenario evidence
  --tier <live|hermetic>  Scenario confidence tier to run or compare (default: live)
  --temper-bin <PATH>   Standalone `temper` binary for --tier live manifest scenarios
  --output-dir <DIR>    Directory for the Markdown report (default: current directory)
  -h, --help           Print help

The bridge records local scenario evidence when available, including scenario
source, the selected confidence tier, and manifest topology. Pass
`--run-evidence <PATH>` to render from a previous `temper-scenario run
--evidence-out <PATH>` artifact without rerunning scenario evidence. Direct
`--scenario` runs use the single public `manifest` runner on the validation-grade
live stack: real Forgejo + real forgejo-runner CI + real Temper + Jig fake LLM.
The command does not fetch live Forgejo PR context or prove that the supplied
SHA is the current main commit.";

pub(super) fn command(args: &[String]) -> ExitCode {
    let args = match parse_args(args) {
        Ok(ParseResult::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseResult::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    let output_path = validation_report_path(&args.output_dir, args.pr_number, &args.sha);
    let report = match build_report(&args, &output_path) {
        Ok(report) => report,
        Err(Error::InvalidScenario(report)) => {
            eprintln!(
                "temper-scenario validate-pr: scenario check failed for {}",
                super::display_path(&report.scenario_path)
            );
            super::print_report_diagnostics(&report);
            return ExitCode::FAILURE;
        }
        Err(Error::RunEvidence(message)) => {
            eprintln!("temper-scenario validate-pr: {message}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = write_report(&output_path, &report) {
        eprintln!("temper-scenario validate-pr: {error}");
        return ExitCode::FAILURE;
    }

    println!("{}", output_path.display());
    if report.verdict == ValidationVerdict::Failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[derive(Debug)]
pub(super) struct Args {
    pub(super) pr_number: u64,
    pub(super) sha: String,
    pub(super) scenario: Option<PathBuf>,
    pub(super) run_evidence: Option<PathBuf>,
    pub(super) tier: ScenarioTier,
    pub(super) tier_explicit: bool,
    pub(super) temper_bin: Option<PathBuf>,
    pub(super) output_dir: PathBuf,
}

#[derive(Debug)]
enum ParseResult {
    Help,
    Args(Args),
}

#[derive(Debug)]
pub(super) enum Error {
    InvalidScenario(Box<temper_scenario_core::CheckReport>),
    RunEvidence(String),
}

fn parse_args(args: &[String]) -> Result<ParseResult, ()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ParseResult::Help);
    }

    let mut pr_number = None;
    let mut sha = None;
    let mut scenario = None;
    let mut run_evidence = None;
    let mut tier = None;
    let mut temper_bin = None;
    let mut output_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pr" => {
                let value = flag_value(args, index, "--pr")?;
                let parsed = value.parse::<u64>().ok().filter(|number| *number > 0);
                let Some(parsed) = parsed else {
                    eprintln!(
                        "temper-scenario validate-pr: --pr must be a positive integer: {value}\n\n{USAGE}"
                    );
                    return Err(());
                };
                if pr_number.replace(parsed).is_some() {
                    eprintln!("temper-scenario validate-pr: duplicate --pr option\n\n{USAGE}");
                    return Err(());
                }
                index += 2;
            }
            "--sha" => {
                let value = flag_value(args, index, "--sha")?;
                if value.trim().is_empty() {
                    eprintln!("temper-scenario validate-pr: --sha must not be empty\n\n{USAGE}");
                    return Err(());
                }
                if sha.replace(value.to_string()).is_some() {
                    eprintln!("temper-scenario validate-pr: duplicate --sha option\n\n{USAGE}");
                    return Err(());
                }
                index += 2;
            }
            "--scenario" => {
                let value = flag_value(args, index, "--scenario")?;
                if scenario.replace(PathBuf::from(value)).is_some() {
                    eprintln!(
                        "temper-scenario validate-pr: duplicate --scenario option\n\n{USAGE}"
                    );
                    return Err(());
                }
                index += 2;
            }
            "--run-evidence" => {
                let value = flag_value(args, index, "--run-evidence")?;
                if run_evidence.replace(PathBuf::from(value)).is_some() {
                    eprintln!(
                        "temper-scenario validate-pr: duplicate --run-evidence option\n\n{USAGE}"
                    );
                    return Err(());
                }
                index += 2;
            }
            "--tier" => {
                let value = flag_value(args, index, "--tier")?;
                set_tier(&mut tier, value)?;
                index += 2;
            }
            "--temper-bin" => {
                let value = flag_value(args, index, "--temper-bin")?;
                set_temper_bin(&mut temper_bin, value)?;
                index += 2;
            }
            "--output-dir" => {
                let value = flag_value(args, index, "--output-dir")?;
                if output_dir.replace(PathBuf::from(value)).is_some() {
                    eprintln!(
                        "temper-scenario validate-pr: duplicate --output-dir option\n\n{USAGE}"
                    );
                    return Err(());
                }
                index += 2;
            }
            other => {
                eprintln!("temper-scenario validate-pr: unexpected argument `{other}`\n\n{USAGE}");
                return Err(());
            }
        }
    }

    let Some(pr_number) = pr_number else {
        eprintln!("temper-scenario validate-pr: missing --pr\n\n{USAGE}");
        return Err(());
    };
    let Some(sha) = sha else {
        eprintln!("temper-scenario validate-pr: missing --sha\n\n{USAGE}");
        return Err(());
    };

    let tier_explicit = tier.is_some();
    Ok(ParseResult::Args(Args {
        pr_number,
        sha,
        scenario,
        run_evidence,
        tier: tier.unwrap_or(ScenarioTier::Live),
        tier_explicit,
        temper_bin,
        output_dir: output_dir.unwrap_or_else(|| PathBuf::from(".")),
    }))
}

fn flag_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str, ()> {
    let Some(value) = args.get(index + 1) else {
        eprintln!("temper-scenario validate-pr: {flag} requires a value\n\n{USAGE}");
        return Err(());
    };
    if value.starts_with("--") {
        eprintln!("temper-scenario validate-pr: {flag} requires a value\n\n{USAGE}");
        return Err(());
    }
    Ok(value)
}

fn set_tier(tier: &mut Option<ScenarioTier>, value: &str) -> Result<(), ()> {
    let Some(parsed) = ScenarioTier::parse(value) else {
        eprintln!(
            "temper-scenario validate-pr: unknown --tier `{value}` (expected live or hermetic)\n\n{USAGE}"
        );
        return Err(());
    };
    if tier.replace(parsed).is_some() {
        eprintln!("temper-scenario validate-pr: duplicate --tier option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_temper_bin(temper_bin: &mut Option<PathBuf>, value: &str) -> Result<(), ()> {
    if temper_bin.replace(PathBuf::from(value)).is_some() {
        eprintln!("temper-scenario validate-pr: duplicate --temper-bin option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}

pub(super) fn build_report(args: &Args, output_path: &Path) -> Result<ValidationReport, Error> {
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
        .with_detail(format!("sha: `{}`", args.sha))
        .with_detail(format!("requested scenario tier: {}", args.tier.as_str()))
        .with_details(
            args.run_evidence
                .as_ref()
                .map(|path| format!("run evidence: `{}`", path.display())),
        ),
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

    if let Some(path) = args.run_evidence.as_deref() {
        run_evidence::add_run_evidence_validation(
            &mut report,
            path,
            args.scenario.as_deref(),
            args.tier,
            args.tier_explicit,
        )?;
    } else {
        match args.scenario.as_deref() {
            Some(path) => add_scenario_validation(
                &mut report,
                path,
                args.tier,
                args.temper_bin.as_deref(),
                &args.output_dir,
            )?,
            None => {
                report.validated_claims.push(
                    ValidatedClaim::new(
                        "No scenario path or run evidence was supplied for local post-merge validation.",
                        ValidationStatus::Unproven,
                    )
                    .with_evidence(
                        "Run with --scenario <PATH> or --run-evidence <PATH> to collect scenario evidence.",
                    ),
                );
                report.acceptance_criteria.push(
                    AcceptanceCriterion::new(
                        "A supplied scenario path is checked and, when supported, run at the requested tier, or a previous run-evidence artifact is ingested.",
                        ValidationStatus::Unproven,
                    )
                    .with_evidence("No --scenario or --run-evidence argument was provided."),
                );
                report.evidence.push(EvidenceEntry::new(
                    EvidenceKind::Observation,
                    "No scenario path or run evidence was supplied, so no scenario check/run evidence was collected.",
                ));
                report.limitations.push(
                    "No scenario path or run evidence was supplied; the report contains no scenario check/run evidence."
                        .to_string(),
                );
            }
        }
    }

    Ok(report)
}

fn add_scenario_validation(
    report: &mut ValidationReport,
    path: &Path,
    tier: ScenarioTier,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) -> Result<(), Error> {
    let check_report = check_scenario(path);
    if !check_report.is_valid() {
        return Err(Error::InvalidScenario(Box::new(check_report)));
    }

    let scenario_name = check_report
        .manifest
        .as_ref()
        .map(|manifest| manifest.name.as_str())
        .unwrap_or("unknown");
    let facts = ScenarioRunFacts::from_check_report(&check_report, tier);

    report.validated_claims.push(
        ValidatedClaim::new(
            format!(
                "Scenario `{scenario_name}` manifest validates at `{}`.",
                super::display_path(&check_report.scenario_path)
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
            super::display_path(&check_report.scenario_path)
        )),
    );
    let mut check_evidence = EvidenceEntry::new(
        EvidenceKind::ScenarioCheck,
        format!(
            "Scenario check passed for `{}`.",
            super::display_path(&check_report.scenario_path)
        ),
    )
    .with_detail(format!("scenario: `{scenario_name}`"));
    if let Some(manifest_path) = check_report.manifest_path.as_deref() {
        check_evidence = check_evidence.with_detail(format!(
            "manifest: `{}`",
            super::display_path(manifest_path)
        ));
    }
    check_evidence = check_evidence.with_details(facts.evidence_details());
    report.evidence.push(check_evidence);

    runner::add_scenario_run(
        report,
        &check_report,
        &facts,
        scenario_name,
        tier,
        temper_bin,
        artifact_dir,
    );

    Ok(())
}

pub(super) fn validation_report_path(output_dir: &Path, pr_number: u64, sha: &str) -> PathBuf {
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

pub(super) fn write_report(path: &Path, report: &ValidationReport) -> Result<(), String> {
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
