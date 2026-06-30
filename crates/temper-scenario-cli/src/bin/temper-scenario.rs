// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    CiJobConclusion, CiJobQuery, CiJobStatus, CreateIssue, Forge, Issue, IssueQuery, IssueState,
    ItemNumber, PullRequest, PullRequestQuery, PullRequestState, RepositoryId,
};
use temper_runner::{
    BoxError, InProcessStage, RunReport, Scenario, Stage, run_scenario_with_budget,
};
use temper_scenario_core::{
    DEFAULT_SCENARIOS_DIR, Diagnostic, Severity, check_scenario, check_scenarios,
    discover_scenarios,
};
use toml::Value;

const EX_USAGE: u8 = 64;
const BASIC_DELIVERY_SCENARIO: &str = "basic-delivery";
const BASIC_DELIVERY_BUDGET: u64 = 64;

const USAGE: &str = "\
temper-scenario: list, check, and run Temper executable scenario manifests

Usage: temper-scenario <COMMAND> [OPTIONS]

Commands:
  list   List scenario directories and stable manifest metadata
  check  Validate one scenario path or all scenarios under a scenarios directory
  run    Run a supported scenario deterministically in process

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
    if manifest.name != BASIC_DELIVERY_SCENARIO {
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

    match temper_testing::block_on(run_basic_delivery(&report.scenario_path, manifest_path)) {
        Ok(outcome) => {
            print_basic_delivery_outcome(&outcome);
            ExitCode::SUCCESS
        }
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

#[derive(Clone, Debug)]
struct IntakeSeed {
    title: String,
    body: String,
    labels: Vec<String>,
}

#[derive(Debug)]
struct BasicDeliveryFixture {
    workflow_path: PathBuf,
    intake: IntakeSeed,
}

#[derive(Debug)]
struct BasicRunOutcome {
    scenario_name: String,
    evidence: BasicRunEvidence,
    report: RunReport,
}

#[derive(Debug)]
struct BasicRunEvidence {
    issue_number: ItemNumber,
    issue_title: String,
    issue_state: IssueState,
    pr_number: ItemNumber,
    pr_state: PullRequestState,
    completed_ci_jobs: usize,
    closed_parent_issues: usize,
}

async fn run_basic_delivery(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<BasicRunOutcome, String> {
    let fixture = load_basic_delivery_fixture(scenario_path, manifest_path)?;
    let workflow = temper_testing::resolve_workflow(Some(&fixture.workflow_path))
        .map_err(|error| error.to_string())?;
    let config = temper_testing::runner_config_for_workflow(&workflow);
    let forge = MemoryForge::new();
    let stage = InProcessStage::with_identity(
        forge,
        workflow,
        config,
        temper_testing::agents::basic_fake_registry(),
        |forge, binding| forge.as_user(binding.user.clone()),
    )
    .await
    .map_err(|error| error.to_string())?
    .with_extra_worker_factory(temper_testing::world::memory_ci_worker);

    let scenario = basic_delivery_scenario(fixture.intake.clone());
    let report = run_scenario_with_budget(&stage, &scenario, BASIC_DELIVERY_BUDGET)
        .await
        .map_err(|error| error.to_string())?;
    let evidence = read_basic_delivery_evidence(stage.forge(), stage.repo(), &fixture.intake)
        .await
        .map_err(|error| error.to_string())?;

    Ok(BasicRunOutcome {
        scenario_name: BASIC_DELIVERY_SCENARIO.to_string(),
        evidence,
        report,
    })
}

fn basic_delivery_scenario(seed: IntakeSeed) -> Scenario {
    let seed = Arc::new(seed);
    let seed_for_seed = Arc::clone(&seed);
    let seed_for_assert = Arc::clone(&seed);
    Scenario::new(
        BASIC_DELIVERY_SCENARIO,
        Box::new(move |forge, repo| {
            let seed = Arc::clone(&seed_for_seed);
            Box::pin(async move {
                forge
                    .create_issue(
                        repo,
                        CreateIssue {
                            title: seed.title.clone(),
                            body: seed.body.clone(),
                            labels: seed.labels.clone(),
                            assignees: Vec::new(),
                        },
                    )
                    .await?;
                Ok(())
            })
        }),
        Box::new(move |forge, repo| {
            let seed = Arc::clone(&seed_for_assert);
            Box::pin(async move {
                read_basic_delivery_evidence(forge, repo, &seed).await?;
                Ok(())
            })
        }),
    )
}

async fn read_basic_delivery_evidence(
    forge: &dyn Forge,
    repo: &RepositoryId,
    seed: &IntakeSeed,
) -> Result<BasicRunEvidence, BoxError> {
    let issues = forge.list_issues(repo, IssueQuery::default()).await?;
    let issue = find_seeded_code_issue(&issues, seed)?;
    let pull_requests = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await?;
    let pull_request = only_implementation_pr(&pull_requests)?;
    match pull_request.state {
        PullRequestState::Merged => {
            if issue.state != IssueState::Closed {
                return Err(boxed_error(format!(
                    "seeded code issue #{} was not closed after merge",
                    issue.number
                )));
            }
            if has_label(&pull_request.labels, "landing") {
                return Err(boxed_error(format!(
                    "implementation PR #{} still has `landing` label",
                    pull_request.number
                )));
            }
        }
        PullRequestState::Open => {
            if issue.state != IssueState::Open || !has_label(&issue.labels, "in-progress") {
                return Err(boxed_error(format!(
                    "seeded code issue #{} did not remain in the deterministic open-PR state",
                    issue.number
                )));
            }
        }
        PullRequestState::Closed => {
            return Err(boxed_error(format!(
                "implementation PR #{} was closed without merging",
                pull_request.number
            )));
        }
    }
    for stale_label in ["ready", "untriaged"] {
        if has_label(&issue.labels, stale_label) {
            return Err(boxed_error(format!(
                "seeded code issue #{} still has `{stale_label}` label",
                issue.number
            )));
        }
    }
    if !pull_request
        .body
        .contains(&format!("#{}", issue.number.get()))
    {
        return Err(boxed_error(format!(
            "implementation PR #{} does not reference seeded issue #{}",
            pull_request.number, issue.number
        )));
    }

    let ci_jobs = forge
        .list_ci_jobs(
            repo,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                status: Some(CiJobStatus::Completed),
                ..CiJobQuery::default()
            },
        )
        .await?;
    if !ci_jobs
        .iter()
        .any(|job| job.conclusion == Some(CiJobConclusion::Success))
    {
        return Err(boxed_error(format!(
            "implementation PR #{} has no passing CI job",
            pull_request.number
        )));
    }

    let closed_parent_issues = issues
        .iter()
        .filter(|candidate| candidate.state == IssueState::Closed)
        .filter(|candidate| candidate.number == issue.number)
        .count();

    Ok(BasicRunEvidence {
        issue_number: issue.number,
        issue_title: issue.title.clone(),
        issue_state: issue.state,
        pr_number: pull_request.number,
        pr_state: pull_request.state,
        completed_ci_jobs: ci_jobs.len(),
        closed_parent_issues,
    })
}

fn find_seeded_code_issue<'a>(
    issues: &'a [Issue],
    seed: &IntakeSeed,
) -> Result<&'a Issue, BoxError> {
    issues
        .iter()
        .find(|issue| issue.title == seed.title && has_label(&issue.labels, "code"))
        .ok_or_else(|| {
            boxed_error(format!(
                "seeded issue `{}` was not triaged into a code issue",
                seed.title
            ))
        })
}

fn only_implementation_pr(pull_requests: &[PullRequest]) -> Result<&PullRequest, BoxError> {
    let implementation_prs = pull_requests
        .iter()
        .filter(|pull_request| has_label(&pull_request.labels, "implementation"))
        .collect::<Vec<_>>();
    match implementation_prs.as_slice() {
        [pull_request] => Ok(*pull_request),
        [] => Err(boxed_error("no implementation PR was created")),
        many => Err(boxed_error(format!(
            "expected one implementation PR, found {}",
            many.len()
        ))),
    }
}

fn load_basic_delivery_fixture(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<BasicDeliveryFixture, String> {
    let manifest = load_manifest_toml(manifest_path)?;
    let workflow_path = workflow_path(scenario_path, &manifest)?;
    let intake = intake_seed(scenario_path, &manifest)?;
    Ok(BasicDeliveryFixture {
        workflow_path,
        intake,
    })
}

fn load_manifest_toml(manifest_path: &Path) -> Result<Value, String> {
    let source = fs::read_to_string(manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    source
        .parse::<Value>()
        .map_err(|error| format!("parse {}: {error}", manifest_path.display()))
}

fn workflow_path(scenario_path: &Path, manifest: &Value) -> Result<PathBuf, String> {
    let path = manifest
        .get("workflow")
        .and_then(Value::as_table)
        .and_then(|workflow| workflow.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("config/workflow.json");
    Ok(scenario_path.join(path))
}

fn intake_seed(scenario_path: &Path, manifest: &Value) -> Result<IntakeSeed, String> {
    let issue = manifest
        .get("issues")
        .and_then(Value::as_array)
        .and_then(|issues| {
            issues.iter().filter_map(Value::as_table).find(|issue| {
                issue.get("kind").and_then(Value::as_str) == Some("intake")
                    || issue.get("id").and_then(Value::as_str) == Some("intake")
            })
        })
        .ok_or_else(|| "basic-delivery manifest has no intake issue fixture".to_string())?;
    let title = issue
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| "basic-delivery intake issue is missing `title`".to_string())?
        .to_string();
    let body_ref = issue
        .get("body")
        .and_then(Value::as_str)
        .ok_or_else(|| "basic-delivery intake issue is missing `body`".to_string())?;
    let body_path = scenario_path.join(body_ref);
    let body = fs::read_to_string(&body_path)
        .map_err(|error| format!("read intake body {}: {error}", body_path.display()))?;
    let labels = issue
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .map(|label| {
                    label.as_str().map(str::to_string).ok_or_else(|| {
                        "basic-delivery intake issue labels must be strings".to_string()
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(IntakeSeed {
        title,
        body,
        labels,
    })
}

fn print_basic_delivery_outcome(outcome: &BasicRunOutcome) {
    println!("scenario: {}", outcome.scenario_name);
    println!("verdict: passed");
    println!("evidence:");
    println!(
        "  seeded issue: #{} \"{}\" {} as code",
        outcome.evidence.issue_number,
        outcome.evidence.issue_title,
        issue_state_word(outcome.evidence.issue_state)
    );
    println!(
        "  implementation PR: #{} {} with passing CI ({} completed job(s))",
        outcome.evidence.pr_number,
        pr_state_evidence(outcome.evidence.pr_state),
        outcome.evidence.completed_ci_jobs
    );
    println!(
        "  closed parent issues: {}",
        outcome.evidence.closed_parent_issues
    );
    println!("  actions: {}", action_counts(&outcome.report));
    println!(
        "  report: ticks={} workers={}",
        outcome.report.ticks,
        outcome.report.workers.len()
    );
}

fn pr_state_evidence(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open as deterministic PR equivalent",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

fn issue_state_word(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open/in-progress",
        IssueState::Closed => "closed",
    }
}

fn action_counts(report: &RunReport) -> String {
    report
        .workers
        .iter()
        .map(|worker| format!("{}={}", worker.name, worker.actions))
        .collect::<Vec<_>>()
        .join(", ")
}

fn boxed_error(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(message.into()))
}

fn has_label(labels: &[String], expected: &str) -> bool {
    labels.iter().any(|label| label == expected)
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
