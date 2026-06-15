// SPDX-License-Identifier: MPL-2.0

//! The work-item feed: translating scanned [`WorkItem`]s into daemon jobs and
//! enriching their payloads with the workspace context the worker-side coding
//! agent needs.
//!
//! [`job_from_work_item`] is a pure translation (no I/O); [`enrich_work_item_job`]
//! performs the Forge reads — repository coordinates, base branch, branch hint,
//! correlation key, artifact snapshot, and workspace manifest — and decides
//! whether a scanned item should be skipped (terminal artifact, or an
//! implementation PR already exists).

mod workspace;

use serde_json::json;
use temper_forge::{Forge, ForgeError, PullRequestState, RepositoryId};
use temper_runner::{
    ScanError, WorkItem, pr_branch_hint, pr_correlation_key, scan_role, scan_role_wake,
};
use temper_worker_protocol::{Artifact, JobContext};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, Effect, RoleId, ToolManifest, ValidatedWorkflow,
    find_pull_request_by_correlation,
};

use crate::workflow_meta::implementation_pr_labels;
use workspace::{build_workspace_manifest, target_number, terminal_checked_snapshot};

pub use self::backstop::{
    PollBackstopConfig, RoleFeedMode, RoleFeedTarget, run_poll_backstop_tick, spawn_poll_backstop,
};

mod backstop;

/// A daemon job derived from a scanned `WorkItem`: exactly the arguments
/// `Daemon::enqueue_job` consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemJob {
    pub job_id: String,
    pub role: String,
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: serde_json::Value,
}

/// Pure translation of a scanned `WorkItem` into a daemon job. No I/O.
pub fn job_from_work_item(repo: &str, item: &WorkItem) -> WorkItemJob {
    let (number, forge_kind) = match item.target {
        ArtifactSource::Issue { number } => (number.get(), "issue"),
        ArtifactSource::PullRequest { number } => (number.get(), "pull_request"),
    };

    let role = item.role.as_str().to_string();
    let queue = item.queue.as_str().to_string();

    let context = JobContext {
        role: role.clone(),
        repo: repo.to_string(),
        queue: queue.clone(),
        artifact_kind: item.kind.as_str().to_string(),
        artifact: None,
        workspace: None,
        action: None,
        checkout_capability: None,
        allowed_verdicts: Vec::new(),
    };

    WorkItemJob {
        job_id: format!("{repo}/{forge_kind}-{number}/{role}/{queue}"),
        role,
        repo: repo.to_string(),
        artifact: Artifact {
            item: json!(number),
            kind: forge_kind.to_string(),
        },
        job_payload: serde_json::to_value(&context).expect("JobContext serializes"),
    }
}

/// Enrich a mapped job's payload with the workspace context the worker-side
/// coding agent needs. Forge reads happen here so `job_from_work_item` stays
/// pure.
pub(crate) async fn enrich_work_item_job<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    item: &WorkItem,
    job: &mut WorkItemJob,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
) -> Result<EnrichOutcome, ScanError> {
    let repository = forge
        .get_repository(repo)
        .await?
        .ok_or_else(|| ScanError::Forge(ForgeError::NotFound(format!("repository {repo}"))))?;

    let number = target_number(item.target);
    let Some(artifact) = terminal_checked_snapshot(forge, repo, item.target).await? else {
        return Ok(EnrichOutcome::SkipTerminalArtifact);
    };

    // Assemble the job's workspace manifest: the primary (writable) repo, plus
    // any additional repos the coordinating issue declares in a `temper:workspace`
    // metadata block (ADR 0023). Absent that block, the manifest is a single
    // writable primary — the degenerate single-repo job.
    let coordination_key = pr_correlation_key(&item.kind, number);
    let branch_hint = pr_branch_hint(&item.kind, number);
    let workspace = build_workspace_manifest(
        forge,
        &repository,
        &job.repo,
        &coordination_key,
        &branch_hint,
        &artifact.body,
    )
    .await?;

    let mut context = JobContext {
        role: job.role.clone(),
        repo: job.repo.clone(),
        queue: item.queue.as_str().to_string(),
        artifact_kind: item.kind.as_str().to_string(),
        artifact: Some(artifact),
        workspace: Some(workspace),
        action: None,
        checkout_capability: None,
        allowed_verdicts: Vec::new(),
    };
    enrich_job_context_from_workflow(item, compiled, &mut context);

    if is_writable_issue_job(item, &context)
        && implementation_pull_request_exists_for_correlation(forge, repo, workflow, &context)
            .await?
    {
        return Ok(EnrichOutcome::SkipExistingPullRequest);
    }

    job.job_payload = serde_json::to_value(&context).expect("JobContext serializes");

    Ok(EnrichOutcome::Enriched)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum EnrichOutcome {
    Enriched,
    SkipTerminalArtifact,
    SkipExistingPullRequest,
}

fn is_writable_issue_job(item: &WorkItem, context: &JobContext) -> bool {
    matches!(item.target, ArtifactSource::Issue { .. })
        && context.checkout_capability.as_deref() == Some("writable")
}

async fn implementation_pull_request_exists_for_correlation<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    context: &JobContext,
) -> Result<bool, ScanError> {
    let Some(coordination_key) = context
        .workspace
        .as_ref()
        .map(|workspace| workspace.coordination_key.as_str())
    else {
        return Ok(false);
    };
    let labels = implementation_pr_labels(workflow);
    let pull_request = find_pull_request_by_correlation(forge, repo, coordination_key, &labels)
        .await
        .map_err(ScanError::Execution)?;
    // A merged implementation PR permanently covers its correlation key. If the
    // source issue is reopened after that merge, rework needs a fresh issue with
    // a fresh correlation key rather than automatic re-feed of the old one.
    Ok(pull_request.is_some_and(|pull_request| {
        matches!(
            pull_request.state,
            PullRequestState::Open | PullRequestState::Merged
        )
    }))
}

pub(crate) fn skip_log_reason(outcome: EnrichOutcome) -> &'static str {
    match outcome {
        EnrichOutcome::Enriched => "",
        EnrichOutcome::SkipTerminalArtifact => "terminal artifact",
        EnrichOutcome::SkipExistingPullRequest => "existing implementation pull request",
    }
}

pub(crate) fn skip_log_line(
    repo_label: &str,
    role: &RoleId,
    item: &WorkItem,
    reason: EnrichOutcome,
) -> String {
    format!(
        "temper-daemon: skipped role work for {} repo={} role={} queue={} artifact_kind={} target={:?}",
        skip_log_reason(reason),
        repo_label,
        role.as_str(),
        item.queue.as_str(),
        item.kind.as_str(),
        item.target
    )
}

pub(crate) fn enrichment_failure_log_line(
    repo_label: &str,
    role: &RoleId,
    item: &WorkItem,
    error: &ScanError,
) -> String {
    format!(
        "temper-daemon: skipped scanned work item after enrichment failed for repo={} role={} queue={} artifact_kind={} target={:?}: {error}",
        repo_label,
        role.as_str(),
        item.queue.as_str(),
        item.kind.as_str(),
        item.target
    )
}

fn enrich_job_context_from_workflow(
    item: &WorkItem,
    compiled: &CompiledWorkflow,
    context: &mut JobContext,
) {
    let Some(role) = compiled.role(&item.role) else {
        return;
    };

    let mut matches = role
        .tools
        .iter()
        .filter(|tool| action_is_workspace_backed(tool) && tool.artifact == item.kind);
    let Some(tool) = matches.next() else {
        return;
    };
    if matches.next().is_some() {
        return;
    }

    context.action = Some(tool.name.clone());
    context.allowed_verdicts = allowed_verdicts(tool);
    context.checkout_capability = Some(if create_pull_request_count(tool) > 0 {
        "writable".to_string()
    } else {
        match item.target {
            ArtifactSource::Issue { .. } => "read_only".to_string(),
            ArtifactSource::PullRequest { .. } => "pull_request_read_only".to_string(),
        }
    });
}

fn action_is_workspace_backed(tool: &ToolManifest) -> bool {
    !tool.outcomes.is_empty()
        || tool
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
}

fn create_pull_request_count(tool: &ToolManifest) -> usize {
    tool.effects
        .iter()
        .filter(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
        .count()
}

fn allowed_verdicts(tool: &ToolManifest) -> Vec<String> {
    tool.outcomes
        .keys()
        .map(|verdict| verdict.as_str().to_string())
        .collect()
}

/// Scans `repo` for `role`'s queue work and enqueues each enriched `WorkItem`
/// into `daemon`. Returns the number of successfully enriched and enqueued jobs.
/// The protocol `repo` label is the artifact repository's `owner/name` path.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_scanned_role_work<F: Forge + ?Sized>(
    daemon: &crate::Daemon,
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: chrono::DateTime<chrono::Utc>,
    role: &RoleId,
    mode: RoleFeedMode,
) -> Result<usize, ScanError> {
    let repo_label = repo_label(forge, repo).await?;
    let items: Vec<WorkItem> = match mode {
        RoleFeedMode::Normal => scan_role(forge, repo, workflow, compiled, now, role).await?,
        RoleFeedMode::Wake => scan_role_wake(forge, repo, workflow, compiled, now, role).await?,
    };
    let mut enqueued = 0;
    for item in &items {
        let mut job = job_from_work_item(&repo_label, item);
        match enrich_work_item_job(forge, repo, item, &mut job, workflow, compiled).await {
            Ok(EnrichOutcome::Enriched) => {
                daemon
                    .enqueue_job(
                        job.job_id,
                        job.role,
                        job.repo,
                        job.artifact,
                        job.job_payload,
                    )
                    .await;
                enqueued += 1;
            }
            Ok(
                skip @ (EnrichOutcome::SkipTerminalArtifact
                | EnrichOutcome::SkipExistingPullRequest),
            ) => {
                eprintln!("{}", skip_log_line(&repo_label, role, item, skip));
            }
            Err(error) => eprintln!(
                "{}",
                enrichment_failure_log_line(&repo_label, role, item, &error)
            ),
        }
    }
    Ok(enqueued)
}

/// Resolves the `owner/name` protocol repo label for a scanned `RepositoryId`.
async fn repo_label<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
) -> Result<String, ScanError> {
    let repository = forge
        .get_repository(repo)
        .await?
        .ok_or_else(|| ScanError::Forge(ForgeError::NotFound(format!("repository {repo}"))))?;
    Ok(format!("{}/{}", repository.owner, repository.name))
}

#[cfg(test)]
mod feed_tests;
