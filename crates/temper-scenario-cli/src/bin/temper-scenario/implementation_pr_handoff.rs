// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use temper_engine::{ForgeApplier, InFlightJob, JobContext, ResultApplier};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, ItemNumber, PullRequest,
    PullRequestQuery, RepositoryId,
};
use temper_protocol_worker::{
    Artifact, Branch, JobResult, RepoOutcome, ResultStatus, WORKER_PROTOCOL_VERSION,
};
use temper_scenario_core::load_resolved_manifest_toml;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, WorkflowMetadata, parse_metadata_block, render_metadata_block,
};
use toml::Value;

use super::run_evidence;

#[path = "implementation_pr_handoff/evidence.rs"]
mod evidence;

pub(super) const SCENARIO_NAME: &str = "implementation-pr-handoff";

#[derive(Debug)]
struct Fixture {
    workflow_path: PathBuf,
    repo: RepoFixture,
    source_issue: SourceIssueFixture,
    handoff: HandoffFixture,
}

#[derive(Debug)]
struct RepoFixture {
    owner: String,
    name: String,
    default_branch: String,
}

impl RepoFixture {
    fn path(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Debug)]
struct SourceIssueFixture {
    title: String,
    body: String,
    labels: Vec<String>,
}

#[derive(Debug)]
struct HandoffFixture {
    create_title: String,
    create_body: String,
    refresh_title: String,
    refresh_body: String,
    stale_title: String,
    stale_body: String,
    summary: String,
}

#[derive(Debug)]
struct RunOutcome {
    create: CaseEvidence,
    refresh: CaseEvidence,
}

#[derive(Debug)]
struct CaseEvidence {
    issue_number: ItemNumber,
    pr_number: ItemNumber,
    pr_state: String,
    labels: Vec<String>,
    head_branch: String,
    head_sha: Option<String>,
    title: String,
    body_prefix: String,
    correlation_key: String,
}

pub(super) fn run_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &super::run_context::ScenarioRunFacts,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    let outcome = temper_testing::block_on(run_handoff(scenario_path, manifest_path))?;
    print_outcome(&outcome, facts);
    Ok(evidence::outcome_artifact(&outcome, context))
}

pub(super) fn run_evidence_lines(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<Vec<String>, String> {
    let outcome = temper_testing::block_on(run_handoff(scenario_path, manifest_path))?;
    Ok(outcome_evidence_lines(&outcome))
}

async fn run_handoff(scenario_path: &Path, manifest_path: &Path) -> Result<RunOutcome, String> {
    let fixture = load_fixture(scenario_path, manifest_path)?;
    let workflow = Arc::new(
        temper_testing::resolve_workflow(Some(&fixture.workflow_path))
            .map_err(|error| error.to_string())?,
    );
    let forge = Arc::new(MemoryForge::new());
    let repo = forge
        .create_repository(CreateRepository {
            owner: fixture.repo.owner.clone(),
            name: fixture.repo.name.clone(),
            default_branch: fixture.repo.default_branch.clone(),
            description: None,
        })
        .await
        .map_err(|error| error.to_string())?
        .id;
    let applier = ForgeApplier::new(Arc::clone(&forge), workflow);
    let repo_path = fixture.repo.path();

    let create_issue = create_ready_issue(
        forge.as_ref(),
        &repo,
        &fixture.source_issue,
        "create authored handoff",
    )
    .await?;
    apply_engineer_success(
        &applier,
        &repo_path,
        create_issue,
        &fixture.handoff.create_title,
        &fixture.handoff.create_body,
        &fixture.handoff.summary,
    )
    .await;
    let create = verify_handoff_pr(
        forge.as_ref(),
        &repo,
        create_issue,
        &fixture.handoff.create_title,
        &fixture.handoff.create_body,
        None,
    )
    .await?;

    let refresh_issue = create_ready_issue(
        forge.as_ref(),
        &repo,
        &fixture.source_issue,
        "refresh existing handoff",
    )
    .await?;
    let refresh_branch = branch_name(refresh_issue);
    let refresh_correlation = correlation_key(refresh_issue);
    let seeded = seed_existing_pr(
        forge.as_ref(),
        &repo,
        &fixture.repo.default_branch,
        refresh_issue,
        &refresh_branch,
        &refresh_correlation,
        &fixture.handoff.stale_title,
        &fixture.handoff.stale_body,
    )
    .await?;
    apply_engineer_success(
        &applier,
        &repo_path,
        refresh_issue,
        &fixture.handoff.refresh_title,
        &fixture.handoff.refresh_body,
        &fixture.handoff.summary,
    )
    .await;
    let refresh = verify_handoff_pr(
        forge.as_ref(),
        &repo,
        refresh_issue,
        &fixture.handoff.refresh_title,
        &fixture.handoff.refresh_body,
        Some(&fixture.handoff.stale_body),
    )
    .await?;
    if refresh.pr_number != seeded.number {
        return Err(format!(
            "refresh opened PR #{} instead of updating existing PR #{}",
            refresh.pr_number, seeded.number
        ));
    }
    assert_no_duplicate_for_branch(forge.as_ref(), &repo, &refresh_branch).await?;

    Ok(RunOutcome { create, refresh })
}

async fn create_ready_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    fixture: &SourceIssueFixture,
    suffix: &str,
) -> Result<ItemNumber, String> {
    let mut labels = fixture.labels.clone();
    push_unique(&mut labels, "code");
    push_unique(&mut labels, "ready");
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: format!("{} ({suffix})", fixture.title),
                body: fixture.body.clone(),
                labels,
                assignees: Vec::new(),
            },
        )
        .await
        .map(|issue| issue.number)
        .map_err(|error| error.to_string())
}

async fn apply_engineer_success(
    applier: &ForgeApplier<MemoryForge>,
    repo_path: &str,
    issue: ItemNumber,
    title: &str,
    body: &str,
    summary: &str,
) {
    let job = open_pr_job(repo_path, issue);
    let result = JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "scripted-coding-workspace".to_string(),
        job_id: job.job_id.clone(),
        status: ResultStatus::Success,
        repos: vec![RepoOutcome {
            repo: repo_path.to_string(),
            branch: Branch {
                name: branch_name(issue),
                head_sha: format!("scenario-head-{}", issue.get()),
            },
        }],
        verdict: None,
        title: Some(title.to_string()),
        body: Some(body.to_string()),
        children: Vec::new(),
        failure: None,
        summary: Some(summary.to_string()),
        details: Some(json!({"scenario": SCENARIO_NAME})),
    };
    applier.apply(job, result).await;
}

fn open_pr_job(repo_path: &str, issue: ItemNumber) -> InFlightJob {
    let context = JobContext {
        role: "engineer".to_string(),
        repo: repo_path.to_string(),
        queue: "code_ready".to_string(),
        artifact_kind: "code".to_string(),
        artifact: None,
        workspace: None,
        action: Some("open_pr".to_string()),
        checkout_capability: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string(), "needs_human".to_string()],
        guidance: None,
        pull_request_freshness: None,
    };
    InFlightJob {
        job_id: format!("{repo_path}/issue-{}/engineer/code_ready", issue.get()),
        role: "engineer".to_string(),
        repo: repo_path.to_string(),
        artifact: Artifact {
            item: json!(issue.get()),
            kind: "issue".to_string(),
        },
        job_payload: serde_json::to_value(context).expect("JobContext serializes"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn seed_existing_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
    base_branch: &str,
    issue: ItemNumber,
    branch: &str,
    correlation_key: &str,
    title: &str,
    body: &str,
) -> Result<PullRequest, String> {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(issue)],
        correlation_key: Some(correlation_key.to_string()),
        ..WorkflowMetadata::default()
    };
    forge
        .create_pull_request(
            repo,
            CreatePullRequest {
                title: title.to_string(),
                body: format!("{}\n\n{}", body.trim(), render_metadata_block(&metadata)),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: branch.to_string(),
                },
                target: BranchRef {
                    repository_id: repo.clone(),
                    branch: base_branch.to_string(),
                },
                labels: vec!["implementation".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .map_err(|error| error.to_string())
}

async fn verify_handoff_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
    issue: ItemNumber,
    expected_title: &str,
    expected_body: &str,
    stale_body: Option<&str>,
) -> Result<CaseEvidence, String> {
    let correlation = correlation_key(issue);
    let pulls = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await
        .map_err(|error| error.to_string())?;
    let pull = find_pull_request_by_correlation(&pulls, &correlation)
        .ok_or_else(|| format!("no implementation PR carried correlation `{correlation}`"))?;
    if pull.title != expected_title {
        return Err(format!(
            "PR #{} title mismatch: expected `{expected_title}`, got `{}`",
            pull.number, pull.title
        ));
    }
    let expected_prefix = expected_body.trim();
    if !pull.body.starts_with(expected_prefix) {
        return Err(format!(
            "PR #{} body did not start with authored report `{expected_prefix}`",
            pull.number
        ));
    }
    if pull.body.contains("Summary:") {
        return Err(format!(
            "PR #{} body used summary fallback instead of authored report",
            pull.number
        ));
    }
    if let Some(stale_body) = stale_body {
        let stale_body = stale_body.trim();
        if !stale_body.is_empty() && pull.body.contains(stale_body) {
            return Err(format!(
                "PR #{} body still contained stale handoff text `{stale_body}`",
                pull.number
            ));
        }
    }
    let metadata = parse_metadata_block(&pull.body)
        .map_err(|error| format!("parse PR #{} metadata: {error}", pull.number))?
        .ok_or_else(|| format!("PR #{} had no workflow metadata block", pull.number))?;
    if metadata.kind != Some(ArtifactKindId::new("implementation_pr")) {
        return Err(format!(
            "PR #{} metadata kind mismatch: {:?}",
            pull.number, metadata.kind
        ));
    }
    if metadata.parents != vec![ArtifactRef::same_repo(issue)] {
        return Err(format!(
            "PR #{} metadata parents mismatch: {:?}",
            pull.number, metadata.parents
        ));
    }
    if metadata.correlation_key.as_deref() != Some(correlation.as_str()) {
        return Err(format!(
            "PR #{} metadata correlation mismatch: {:?}",
            pull.number, metadata.correlation_key
        ));
    }

    Ok(CaseEvidence {
        issue_number: issue,
        pr_number: pull.number,
        pr_state: evidence::pr_state_value(pull.state).to_string(),
        labels: pull.labels.clone(),
        head_branch: pull.source.branch.clone(),
        head_sha: pull.head_sha.clone(),
        title: pull.title.clone(),
        body_prefix: first_line(expected_prefix),
        correlation_key: correlation,
    })
}

fn find_pull_request_by_correlation<'a>(
    pulls: &'a [PullRequest],
    correlation: &str,
) -> Option<&'a PullRequest> {
    pulls.iter().find(|pull| {
        has_label(&pull.labels, "implementation")
            && parse_metadata_block(&pull.body)
                .ok()
                .flatten()
                .and_then(|metadata| metadata.correlation_key)
                .as_deref()
                == Some(correlation)
    })
}

async fn assert_no_duplicate_for_branch(
    forge: &MemoryForge,
    repo: &RepositoryId,
    branch: &str,
) -> Result<(), String> {
    let pulls = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await
        .map_err(|error| error.to_string())?;
    let matches = pulls
        .iter()
        .filter(|pull| has_label(&pull.labels, "implementation"))
        .filter(|pull| pull.source.branch == branch)
        .count();
    if matches != 1 {
        return Err(format!(
            "expected one implementation PR for branch `{branch}`, found {matches}"
        ));
    }
    Ok(())
}

fn load_fixture(scenario_path: &Path, manifest_path: &Path) -> Result<Fixture, String> {
    let manifest = load_manifest_toml(manifest_path)?;
    let workflow_path = workflow_path(scenario_path, &manifest)?;
    let repo = repo_fixture(&manifest)?;
    let source_issue = source_issue_fixture(scenario_path, &manifest)?;
    let handoff = handoff_fixture(scenario_path, &manifest)?;
    Ok(Fixture {
        workflow_path,
        repo,
        source_issue,
        handoff,
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

fn repo_fixture(manifest: &Value) -> Result<RepoFixture, String> {
    let repo = manifest
        .get("repos")
        .and_then(Value::as_array)
        .and_then(|repos| repos.iter().filter_map(Value::as_table).next())
        .ok_or_else(|| "implementation-pr-handoff manifest has no repo fixture".to_string())?;
    let slug = repo
        .get("slug")
        .or_else(|| repo.get("repo"))
        .or_else(|| repo.get("repository"))
        .and_then(Value::as_str)
        .ok_or_else(|| "implementation-pr-handoff repo is missing `slug`".to_string())?;
    let Some((owner, name)) = slug.split_once('/') else {
        return Err(format!(
            "implementation-pr-handoff repo slug must be owner/name, got `{slug}`"
        ));
    };
    let default_branch = repo
        .get("default_branch")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_string();
    Ok(RepoFixture {
        owner: owner.to_string(),
        name: name.to_string(),
        default_branch,
    })
}

fn source_issue_fixture(
    scenario_path: &Path,
    manifest: &Value,
) -> Result<SourceIssueFixture, String> {
    let issue = manifest
        .get("issues")
        .and_then(Value::as_array)
        .and_then(|issues| {
            issues.iter().filter_map(Value::as_table).find(|issue| {
                issue.get("id").and_then(Value::as_str) == Some("source")
                    || issue.get("kind").and_then(Value::as_str) == Some("code")
            })
        })
        .ok_or_else(|| "implementation-pr-handoff manifest has no source issue".to_string())?;
    let title = issue
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| "implementation-pr-handoff source issue is missing `title`".to_string())?
        .to_string();
    let body_ref = issue
        .get("body")
        .and_then(Value::as_str)
        .ok_or_else(|| "implementation-pr-handoff source issue is missing `body`".to_string())?;
    let body_path = scenario_path.join(body_ref);
    let body = fs::read_to_string(&body_path)
        .map_err(|error| format!("read source issue body {}: {error}", body_path.display()))?;
    let labels = labels_field(issue, "source issue labels")?;
    Ok(SourceIssueFixture {
        title,
        body,
        labels,
    })
}

fn handoff_fixture(scenario_path: &Path, manifest: &Value) -> Result<HandoffFixture, String> {
    let handoff = manifest
        .get("handoff")
        .and_then(Value::as_table)
        .ok_or_else(|| "implementation-pr-handoff manifest has no [handoff] section".to_string())?;
    Ok(HandoffFixture {
        create_title: required_string(handoff, "create_title")?,
        create_body: read_path_field(scenario_path, handoff, "create_body_path")?,
        refresh_title: required_string(handoff, "refresh_title")?,
        refresh_body: read_path_field(scenario_path, handoff, "refresh_body_path")?,
        stale_title: required_string(handoff, "stale_title")?,
        stale_body: required_string(handoff, "stale_body")?,
        summary: required_string(handoff, "summary")?,
    })
}

fn required_string(table: &toml::Table, field: &str) -> Result<String, String> {
    table
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("implementation-pr-handoff [handoff] is missing `{field}`"))
}

fn read_path_field(
    scenario_path: &Path,
    table: &toml::Table,
    field: &str,
) -> Result<String, String> {
    let path = required_string(table, field)?;
    let resolved = scenario_path.join(&path);
    fs::read_to_string(&resolved)
        .map_err(|error| format!("read {field} {}: {error}", resolved.display()))
}

fn labels_field(table: &toml::Table, context: &str) -> Result<Vec<String>, String> {
    table
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .map(|label| {
                    label
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("{context} must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map(|labels| labels.unwrap_or_default())
}

fn print_outcome(outcome: &RunOutcome, facts: &super::run_context::ScenarioRunFacts) {
    println!("scenario: {SCENARIO_NAME}");
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
            "create authored PR title/body: PR #{} for issue #{} has title \"{}\" and body prefix \"{}\"",
            outcome.create.pr_number,
            outcome.create.issue_number,
            outcome.create.title,
            outcome.create.body_prefix
        ),
        format!(
            "refresh authored PR title/body: existing PR #{} for issue #{} has title \"{}\" and stale body text was cleared",
            outcome.refresh.pr_number, outcome.refresh.issue_number, outcome.refresh.title
        ),
        format!(
            "workflow metadata/source relation: create parent #{} correlation {}; refresh parent #{} correlation {}",
            outcome.create.issue_number,
            outcome.create.correlation_key,
            outcome.refresh.issue_number,
            outcome.refresh.correlation_key
        ),
        "metadata kind verified: implementation_pr".to_string(),
    ]
}

fn branch_name(issue: ItemNumber) -> String {
    format!("agent/{}", correlation_key(issue))
}

fn correlation_key(issue: ItemNumber) -> String {
    format!("pr-for-code-{}", issue.get())
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or(value).trim().to_string()
}

fn push_unique(labels: &mut Vec<String>, label: &str) {
    if !labels.iter().any(|candidate| candidate == label) {
        labels.push(label.to_string());
    }
}

fn has_label(labels: &[String], expected: &str) -> bool {
    labels.iter().any(|label| label == expected)
}
