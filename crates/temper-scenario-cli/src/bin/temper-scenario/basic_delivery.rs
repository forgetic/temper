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
use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

use super::run_evidence;

#[path = "basic_delivery/live.rs"]
mod live;

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
    issue_labels: Vec<String>,
    pr_number: ItemNumber,
    pr_title: String,
    pr_state: PullRequestState,
    pr_labels: Vec<String>,
    pr_head_branch: String,
    pr_head_sha: Option<String>,
    completed_ci_jobs: usize,
    ci_jobs: Vec<run_evidence::CiJobEvidence>,
    closed_parent_issues: usize,
}

pub(super) fn run_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &super::run_context::ScenarioRunFacts,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    let outcome = temper_testing::block_on(run_basic_delivery(scenario_path, manifest_path))?;
    print_outcome(&outcome, facts);
    Ok(outcome_artifact(&outcome, context))
}

pub(super) fn run_evidence_lines(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<Vec<String>, String> {
    let outcome = temper_testing::block_on(run_basic_delivery(scenario_path, manifest_path))?;
    Ok(outcome_evidence_lines(&outcome))
}

pub(super) fn run_live_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &super::run_context::ScenarioRunFacts,
    temper_bin: Option<&Path>,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    live::run_and_print(scenario_path, manifest_path, facts, temper_bin, context)
}

pub(super) fn run_live_evidence_lines_for_report(
    scenario_path: &Path,
    manifest_path: &Path,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) -> Result<Vec<String>, String> {
    live::evidence_lines(scenario_path, manifest_path, temper_bin, Some(artifact_dir))
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
    if pull_request.state != PullRequestState::Merged {
        return Err(boxed_error(format!(
            "implementation PR #{} was not merged (state: {})",
            pull_request.number,
            pr_state_evidence(pull_request.state)
        )));
    }
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
    for stale_label in ["ready", "untriaged", "in-progress"] {
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
    if closed_parent_issues != 1 {
        return Err(boxed_error(format!(
            "expected exactly 1 closed parent issue, found {closed_parent_issues}"
        )));
    }

    Ok(RunEvidence {
        issue_number: issue.number,
        issue_title: issue.title.clone(),
        issue_state: issue.state,
        issue_labels: issue.labels.clone(),
        pr_number: pull_request.number,
        pr_title: pull_request.title.clone(),
        pr_state: pull_request.state,
        pr_labels: pull_request.labels.clone(),
        pr_head_branch: pull_request.source.branch.clone(),
        pr_head_sha: pull_request.head_sha.clone(),
        completed_ci_jobs: ci_jobs.len(),
        ci_jobs: ci_jobs
            .iter()
            .map(|job| run_evidence::CiJobEvidence {
                name: job.name.clone(),
                status: format!("{:?}", job.status).to_ascii_lowercase(),
                pull_request_number: Some(pull_request.number.get()),
                conclusion: job
                    .conclusion
                    .map(ci_job_conclusion_value)
                    .map(str::to_string),
                url: job.url.clone(),
            })
            .collect(),
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
    load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())
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

fn print_outcome(outcome: &RunOutcome, facts: &super::run_context::ScenarioRunFacts) {
    println!("scenario: {}", outcome.scenario_name);
    facts.print_stdout();
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

fn outcome_artifact(
    outcome: &RunOutcome,
    context: &run_evidence::RunEvidenceContext,
) -> run_evidence::RunEvidenceArtifact {
    let mut artifact = context.artifact(run_evidence::FinalStateEvidence {
        issues: vec![run_evidence::IssueStateEvidence {
            number: outcome.evidence.issue_number.get(),
            id: Some("intake".to_string()),
            title: Some(outcome.evidence.issue_title.clone()),
            state: Some(issue_state_value(outcome.evidence.issue_state).to_string()),
            labels: outcome.evidence.issue_labels.clone(),
        }],
        pull_requests: vec![run_evidence::PullRequestStateEvidence {
            number: outcome.evidence.pr_number.get(),
            id: Some("implementation".to_string()),
            title: Some(outcome.evidence.pr_title.clone()),
            state: Some(pr_state_value(outcome.evidence.pr_state).to_string()),
            labels: outcome.evidence.pr_labels.clone(),
            head_branch: Some(outcome.evidence.pr_head_branch.clone()),
            head_sha: outcome.evidence.pr_head_sha.clone(),
            merged_sha: if outcome.evidence.pr_state == PullRequestState::Merged {
                outcome.evidence.pr_head_sha.clone()
            } else {
                None
            },
        }],
        ci: run_evidence::CiStateEvidence {
            completed_jobs: Some(outcome.evidence.completed_ci_jobs),
            jobs: outcome.evidence.ci_jobs.clone(),
        },
    });
    artifact.convergence = Some(run_evidence::ConvergenceEvidence {
        ticks: Some(outcome.report.ticks),
        workers: outcome
            .report
            .workers
            .iter()
            .map(|worker| run_evidence::WorkerTickEvidence {
                name: worker.name.clone(),
                ticks: worker.ticks,
                actions: worker.actions,
            })
            .collect(),
        ..run_evidence::ConvergenceEvidence::default()
    });
    artifact.evidence_lines = outcome_evidence_lines(outcome);
    artifact
}

fn pr_state_evidence(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open (not merged)",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

fn pr_state_value(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

fn issue_state_value(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    }
}

fn ci_job_conclusion_value(conclusion: CiJobConclusion) -> &'static str {
    match conclusion {
        CiJobConclusion::Success => "success",
        CiJobConclusion::Failure => "failure",
        CiJobConclusion::Cancelled => "cancelled",
        CiJobConclusion::Skipped => "skipped",
        CiJobConclusion::TimedOut => "timed_out",
        CiJobConclusion::Neutral => "neutral",
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

#[cfg(test)]
mod tests {
    use super::{IntakeSeed, read_evidence};
    use temper_forge_memory::MemoryForge;
    use temper_forge_model::{
        BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreateIssue, CreatePullRequest,
        Forge, MergeMethod, MergePullRequest, PullRequest, RepositoryId,
    };

    #[test]
    fn evidence_rejects_open_pr_even_with_passing_ci() {
        let error = temper_testing::block_on(async {
            let (forge, repo, seed) = fixture_state(false).await;
            read_evidence(&forge, &repo, &seed)
                .await
                .expect_err("open PR must not satisfy the scenario contract")
                .to_string()
        });

        assert!(
            error.contains("implementation PR #1 was not merged"),
            "{error}"
        );
    }

    #[test]
    fn evidence_rejects_merged_pr_when_parent_issue_is_still_open() {
        let error = temper_testing::block_on(async {
            let (forge, repo, seed) = fixture_state(true).await;
            read_evidence(&forge, &repo, &seed)
                .await
                .expect_err("open parent issue must not satisfy the scenario contract")
                .to_string()
        });

        assert!(
            error.contains("seeded code issue #1 was not closed after merge"),
            "{error}"
        );
    }

    async fn fixture_state(merge_pr: bool) -> (MemoryForge, RepositoryId, IntakeSeed) {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(temper_testing::repo_input())
            .await
            .expect("repository created")
            .id;
        let seed = IntakeSeed {
            title: "Seeded contract work".to_string(),
            body: "Implement the contract.".to_string(),
            labels: Vec::new(),
        };
        let issue = forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: seed.title.clone(),
                    body: seed.body.clone(),
                    labels: vec!["code".to_string(), "in-progress".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue created");
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: format!("Implement #{}", issue.number),
                    body: format!("Implementation for parent #{}.", issue.number),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "fake/pr-for-code-1".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string(), "landing".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request created");
        forge.seed_ci_jobs(&repo, vec![ci_job(&repo, &pull_request)]);
        if merge_pr {
            forge
                .merge_pull_request(
                    &pull_request.id,
                    MergePullRequest {
                        method: MergeMethod::MergeCommit,
                        commit_title: None,
                        commit_body: None,
                        delete_source_branch: false,
                    },
                )
                .await
                .expect("pull request merged");
        }
        (forge, repo, seed)
    }

    fn ci_job(repo: &RepositoryId, pull_request: &PullRequest) -> CiJob {
        let now = temper_testing::ts("2026-05-29T00:00:00Z");
        CiJob {
            id: CiJobId::new("ci-basic-delivery-contract"),
            repo_id: repo.clone(),
            pull_request_id: Some(pull_request.id.clone()),
            commit_sha: pull_request.head_sha.clone().unwrap_or_default(),
            name: "ci".to_string(),
            status: CiJobStatus::Completed,
            conclusion: Some(CiJobConclusion::Success),
            url: None,
            created_at: now,
            started_at: Some(now),
            completed_at: Some(now),
            updated_at: now,
        }
    }
}
