use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    BranchRef, CommitFile, CreateBranch, CreatePullRequest, Forge, ForgeContent, ItemNumber,
    PullRequest, PullRequestQuery, PullRequestState, RepositoryId, UpdateIssue, UpdatePullRequest,
};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, WorkflowMetadata, parse_metadata_block, render_metadata_block,
};
use toml::Value as TomlValue;

use super::{FinalStateEvidence, IssueEvidence, LiveManifestHarness, PullRequestEvidence};

pub(super) mod fake;

const REFRESH_FAKE_TIMEOUT: Duration = Duration::from_secs(60);
const ASSERT_POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveHandoffEvidence {
    pub create: LiveHandoffCaseEvidence,
    pub refresh: LiveHandoffCaseEvidence,
    pub stale_body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveHandoffCaseEvidence {
    pub issue_number: u64,
    pub pr_number: u64,
    pub pr_state: String,
    pub labels: Vec<String>,
    pub head_branch: String,
    pub head_sha: Option<String>,
    pub title: String,
    pub body: String,
    pub body_prefix: String,
    pub correlation_key: String,
    pub source_artifact: String,
}

#[derive(Clone, Debug)]
struct HandoffFixture {
    create_issue_title: String,
    create_title: String,
    create_body: String,
    refresh_title: String,
    refresh_body: String,
    stale_body: String,
}

pub(super) fn converge(
    harness: &LiveManifestHarness,
    forge: &ForgejoForge,
    repository: &RepositoryId,
    standalone: &mut super::process::ChildGuard,
    timeout: Duration,
    issues: &std::collections::BTreeMap<String, ItemNumber>,
) -> Result<(FinalStateEvidence, LiveHandoffEvidence), String> {
    let fixture = HandoffFixture::load(
        &harness.scenario.scenario_path,
        &harness.scenario.resolved_manifest,
    )?;
    let create_issue = issues
        .get("create")
        .copied()
        .ok_or_else(|| "handoff convergence requires issue binding `create`".to_string())?;
    let refresh_issue = issues
        .get("refresh")
        .copied()
        .ok_or_else(|| "handoff convergence requires issue binding `refresh`".to_string())?;
    let deadline = Instant::now() + timeout;
    let create = poll_handoff_case(
        deadline,
        standalone,
        forge,
        repository,
        &harness.scenario.repo.slug,
        create_issue,
        &fixture.create_title,
        &fixture.create_body,
        None,
    )?;
    let refresh = poll_handoff_case(
        deadline,
        standalone,
        forge,
        repository,
        &harness.scenario.repo.slug,
        refresh_issue,
        &fixture.refresh_title,
        &fixture.refresh_body,
        Some(&fixture.stale_body),
    )?;
    super::process::engine_block_on(assert_no_duplicate_for_branch(
        forge,
        repository,
        &branch_name(refresh_issue),
    ))?;
    let final_state = FinalStateEvidence {
        issue: IssueEvidence {
            number: create.issue_number,
            title: fixture.create_issue_title.clone(),
            state: "open".to_string(),
            labels: vec!["code".to_string(), "in-progress".to_string()],
        },
        pull_request: PullRequestEvidence {
            number: create.pr_number,
            title: create.title.clone(),
            state: create.pr_state.clone(),
            labels: create.labels.clone(),
            author: super::ENGINEER.to_string(),
            merged_by: None,
            head_branch: create.head_branch.clone(),
            head_sha: create.head_sha.clone(),
            merged_sha: None,
        },
        ci_jobs: Vec::new(),
        ci_observations: Vec::new(),
        ci_heads: Vec::new(),
    };
    Ok((
        final_state,
        LiveHandoffEvidence {
            create,
            refresh,
            stale_body: fixture.stale_body,
        },
    ))
}

pub(super) async fn mark_issue_ready(
    forge: &impl Forge,
    repo_slug: &str,
    issue: ItemNumber,
) -> Result<(), String> {
    let issue_id =
        temper_forge_model::IssueId::new(format!("forgejo:{repo_slug}:issue:{}", issue.get()));
    forge
        .update_issue(
            &issue_id,
            UpdateIssue {
                add_labels: vec!["ready".to_string()],
                ..UpdateIssue::default()
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| format!("mark refresh issue #{issue} ready failed: {error}"))
}

pub(super) async fn mark_stale_pr_as_implementation(
    forge: &impl Forge,
    pull_request: &PullRequest,
) -> Result<(), String> {
    forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                add_labels: vec!["implementation".to_string(), "in-progress".to_string()],
                ..UpdatePullRequest::default()
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| {
            format!(
                "mark stale refresh PR #{} as implementation failed: {error}",
                pull_request.number
            )
        })
}

pub(super) async fn seed_existing_pr(
    forge: &(impl Forge + ForgeContent),
    repository: &RepositoryId,
    default_branch: &str,
    issue: ItemNumber,
    title: &str,
    body: &str,
    metadata_kind: &str,
) -> Result<PullRequest, String> {
    let branch = branch_name(issue);
    forge
        .create_branch(
            repository,
            CreateBranch {
                new_branch: branch.clone(),
                from_branch: default_branch.to_string(),
            },
        )
        .await
        .map_err(|error| format!("create refresh branch {branch}: {error}"))?;
    forge
        .commit_file(
            repository,
            CommitFile {
                path: "HANDOFF_REFRESH_STALE.md".to_string(),
                contents: body.as_bytes().to_vec(),
                message: "seed stale implementation PR handoff".to_string(),
                branch: branch.clone(),
            },
        )
        .await
        .map_err(|error| format!("commit stale refresh fixture on {branch}: {error}"))?;

    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new(metadata_kind)),
        parents: vec![ArtifactRef::same_repo(issue)],
        correlation_key: Some(correlation_key(issue)),
        ..WorkflowMetadata::default()
    };
    forge
        .create_pull_request(
            repository,
            CreatePullRequest {
                title: title.to_string(),
                body: format!("{}\n\n{}", body.trim(), render_metadata_block(&metadata)),
                source: BranchRef {
                    repository_id: repository.clone(),
                    branch,
                },
                target: BranchRef {
                    repository_id: repository.clone(),
                    branch: default_branch.to_string(),
                },
                labels: Vec::new(),
                assignees: Vec::new(),
            },
        )
        .await
        .map_err(|error| format!("create stale refresh implementation PR failed: {error}"))
}

async fn assert_no_duplicate_for_branch(
    forge: &impl Forge,
    repository: &RepositoryId,
    branch: &str,
) -> Result<(), String> {
    let pulls = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| error.to_string())?;
    let matches = pulls
        .iter()
        .filter(|pull| pull.labels.iter().any(|label| label == "implementation"))
        .filter(|pull| pull.source.branch == branch)
        .count();
    if matches == 1 {
        Ok(())
    } else {
        Err(format!(
            "expected one implementation PR for branch `{branch}`, found {matches}"
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn poll_handoff_case(
    deadline: Instant,
    standalone: &mut super::process::ChildGuard,
    forge: &impl Forge,
    repository: &RepositoryId,
    repo_slug: &str,
    issue: ItemNumber,
    expected_title: &str,
    expected_body: &str,
    stale_body: Option<&str>,
) -> Result<LiveHandoffCaseEvidence, String> {
    loop {
        if let Some(status) = standalone.try_wait()? {
            return Err(format!("{} exited early with {status:?}", standalone.label));
        }
        match super::process::engine_block_on(verify_handoff_case(
            forge,
            repository,
            repo_slug,
            issue,
            expected_title,
            expected_body,
            stale_body,
        )) {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(ASSERT_POLL);
            }
            Err(error) => return Err(error),
        }
    }
}

async fn verify_handoff_case(
    forge: &impl Forge,
    repository: &RepositoryId,
    repo_slug: &str,
    issue: ItemNumber,
    expected_title: &str,
    expected_body: &str,
    stale_body: Option<&str>,
) -> Result<LiveHandoffCaseEvidence, String> {
    let correlation = correlation_key(issue);
    let pulls = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list pull requests failed: {error}"))?;
    let pull = pulls
        .iter()
        .find(|pull| {
            pull.labels.iter().any(|label| label == "implementation")
                && parse_metadata_block(&pull.body)
                    .ok()
                    .flatten()
                    .and_then(|metadata| metadata.correlation_key)
                    .as_deref()
                    == Some(correlation.as_str())
        })
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
    if let Some(stale_body) = stale_body.map(str::trim) {
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
    if !pull.labels.iter().any(|label| label == "landing") {
        return Err(format!(
            "PR #{} handoff labels not applied yet (labels {:?})",
            pull.number, pull.labels
        ));
    }
    if pull.labels.iter().any(|label| label == "in-progress") {
        return Err(format!(
            "PR #{} still has in-progress label after handoff (labels {:?})",
            pull.number, pull.labels
        ));
    }

    Ok(LiveHandoffCaseEvidence {
        issue_number: issue.get(),
        pr_number: pull.number.get(),
        pr_state: pr_state_evidence(pull.state).to_string(),
        labels: pull.labels.clone(),
        head_branch: pull.source.branch.clone(),
        head_sha: pull.head_sha.clone(),
        title: pull.title.clone(),
        body: pull.body.clone(),
        body_prefix: first_line(expected_prefix),
        correlation_key: correlation,
        source_artifact: format!("{repo_slug}#{}", issue.get()),
    })
}

impl HandoffFixture {
    fn load(scenario_path: &Path, manifest: &TomlValue) -> Result<Self, String> {
        let create_issue_title = issue_title(manifest, "create")?
            .or(issue_title(manifest, "source")?)
            .ok_or_else(|| {
                "implementation-pr-handoff manifest has no create/source issue".to_string()
            })?;
        let handoff = manifest
            .get("handoff")
            .and_then(TomlValue::as_table)
            .ok_or_else(|| {
                "implementation-pr-handoff manifest has no [handoff] section".to_string()
            })?;
        Ok(Self {
            create_issue_title,
            create_title: required_string(handoff, "create_title")?,
            create_body: read_path_field(scenario_path, handoff, "create_body_path")?,
            refresh_title: required_string(handoff, "refresh_title")?,
            refresh_body: read_path_field(scenario_path, handoff, "refresh_body_path")?,
            stale_body: required_string(handoff, "stale_body")?,
        })
    }
}

fn issue_title(manifest: &TomlValue, id: &str) -> Result<Option<String>, String> {
    let issue = manifest
        .get("issues")
        .and_then(TomlValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(TomlValue::as_table)
        .find(|issue| issue.get("id").and_then(TomlValue::as_str) == Some(id));
    issue
        .map(|issue| {
            issue
                .get("title")
                .and_then(TomlValue::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("issue `{id}` is missing `title`"))
        })
        .transpose()
}

fn required_string(table: &toml::Table, field: &str) -> Result<String, String> {
    table
        .get(field)
        .and_then(TomlValue::as_str)
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

fn branch_name(issue: ItemNumber) -> String {
    format!("agent/{}", correlation_key(issue))
}

fn correlation_key(issue: ItemNumber) -> String {
    format!("pr-for-code-{}", issue.get())
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or(value).trim().to_string()
}

fn pr_state_evidence(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}
