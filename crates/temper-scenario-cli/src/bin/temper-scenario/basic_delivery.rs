// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    CiJobConclusion, CiJobQuery, CiJobStatus, CreateIssue, Forge, Issue, IssueQuery, IssueState,
    ItemNumber, PullRequest, PullRequestQuery, PullRequestState, RepositoryId,
};
use temper_runner::{
    BoxError, InProcessStage, RunReport, Scenario, Stage, run_scenario_with_budget,
};
use toml::Value;

pub(super) const SCENARIO_NAME: &str = "basic-delivery";
const BUDGET: u64 = 64;

#[derive(Clone, Debug)]
struct IntakeSeed {
    title: String,
    body: String,
    labels: Vec<String>,
}

#[derive(Debug)]
struct Fixture {
    workflow_path: PathBuf,
    intake: IntakeSeed,
}

#[derive(Debug)]
struct RunOutcome {
    scenario_name: String,
    evidence: RunEvidence,
    report: RunReport,
}

#[derive(Debug)]
struct RunEvidence {
    issue_number: ItemNumber,
    issue_title: String,
    issue_state: IssueState,
    pr_number: ItemNumber,
    pr_state: PullRequestState,
    completed_ci_jobs: usize,
    closed_parent_issues: usize,
}

pub(super) fn run_and_print(scenario_path: &Path, manifest_path: &Path) -> Result<(), String> {
    let outcome = temper_testing::block_on(run_basic_delivery(scenario_path, manifest_path))?;
    print_outcome(&outcome);
    Ok(())
}

pub(super) fn run_evidence_lines(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<Vec<String>, String> {
    let outcome = temper_testing::block_on(run_basic_delivery(scenario_path, manifest_path))?;
    Ok(outcome_evidence_lines(&outcome))
}

async fn run_basic_delivery(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<RunOutcome, String> {
    let fixture = load_fixture(scenario_path, manifest_path)?;
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

    let scenario = scenario(fixture.intake.clone());
    let report = run_scenario_with_budget(&stage, &scenario, BUDGET)
        .await
        .map_err(|error| error.to_string())?;
    let evidence = read_evidence(stage.forge(), stage.repo(), &fixture.intake)
        .await
        .map_err(|error| error.to_string())?;

    Ok(RunOutcome {
        scenario_name: SCENARIO_NAME.to_string(),
        evidence,
        report,
    })
}

fn scenario(seed: IntakeSeed) -> Scenario {
    let seed = Arc::new(seed);
    let seed_for_seed = Arc::clone(&seed);
    let seed_for_assert = Arc::clone(&seed);
    Scenario::new(
        SCENARIO_NAME,
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
                read_evidence(forge, repo, &seed).await?;
                Ok(())
            })
        }),
    )
}

async fn read_evidence(
    forge: &dyn Forge,
    repo: &RepositoryId,
    seed: &IntakeSeed,
) -> Result<RunEvidence, BoxError> {
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

    Ok(RunEvidence {
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

fn load_fixture(scenario_path: &Path, manifest_path: &Path) -> Result<Fixture, String> {
    let manifest = load_manifest_toml(manifest_path)?;
    let workflow_path = workflow_path(scenario_path, &manifest)?;
    let intake = intake_seed(scenario_path, &manifest)?;
    Ok(Fixture {
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

fn print_outcome(outcome: &RunOutcome) {
    println!("scenario: {}", outcome.scenario_name);
    println!("verdict: passed");
    println!("evidence:");
    for line in outcome_evidence_lines(outcome) {
        println!("  {line}");
    }
}

fn outcome_evidence_lines(outcome: &RunOutcome) -> Vec<String> {
    vec![
        format!(
            "seeded issue: #{} \"{}\" {} as code",
            outcome.evidence.issue_number,
            outcome.evidence.issue_title,
            issue_state_word(outcome.evidence.issue_state)
        ),
        format!(
            "implementation PR: #{} {} with passing CI ({} completed job(s))",
            outcome.evidence.pr_number,
            pr_state_evidence(outcome.evidence.pr_state),
            outcome.evidence.completed_ci_jobs
        ),
        format!(
            "closed parent issues: {}",
            outcome.evidence.closed_parent_issues
        ),
        format!("actions: {}", action_counts(&outcome.report)),
        format!(
            "report: ticks={} workers={}",
            outcome.report.ticks,
            outcome.report.workers.len()
        ),
    ]
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
