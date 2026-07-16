// SPDX-License-Identifier: MPL-2.0

//! Maps scanned work items into enriched daemon jobs while keeping pure job
//! identity translation separate from Forge-backed workspace context loading.

mod action_assignment;
mod broad;
mod recovery;
mod targeted;
mod workspace;

use self::action_assignment::{enrich_job_context_from_workflow, enrich_pull_request_writable_job};
use self::targeted::TargetedEnrichment;
use std::collections::BTreeSet;

use serde_json::json;
use temper_forge::{Forge, ForgeError, PullRequestState, RepositoryId};
use temper_protocol_worker::{Artifact, JobContext};
use temper_runner::{
    ScanError, TargetedArtifactSnapshot, WorkItem, pr_branch_hint, pr_correlation_key, scan_role,
    scan_role_wake,
};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, RoleId, ValidatedWorkflow, find_pull_request_by_correlation,
    parse_metadata_block,
};

use crate::workflow_meta::implementation_pr_labels;
use workspace::{build_workspace_manifest, target_number, terminal_checked_snapshot};

pub use self::backstop::{
    PollBackstopConfig, RoleFeedMode, RoleFeedTarget, run_poll_backstop_tick,
    spawn_coordinated_poll_backstop, spawn_poll_backstop,
};
pub(crate) use self::broad::enqueue_scanned_roles_wake;
use self::recovery::recover_advanced_pull_request_assignments;
pub use self::targeted::{TargetedRoleFeedResult, enqueue_targeted_role_work};

mod backstop;

/// Arguments consumed by `Daemon::enqueue_job` for a scanned work item.
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
        trace_context: None,
        artifact_context: None,
        role: role.clone(),
        repo: repo.to_string(),
        queue: queue.clone(),
        artifact_kind: item.kind.as_str().to_string(),
        artifact: None,
        workspace: None,
        action: None,
        checkout_capability: None,
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: None,
        pull_request_freshness: None,
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

pub async fn recovered_job_from_assignment<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
    assignment: &temper_workflow::DurableAssignment,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
) -> Result<WorkItemJob, String> {
    recovered_job_from_assignment_inner(forge, repo, target, assignment, workflow, compiled, None)
        .await
}

/// Reconstructs a durable assignment through the same artifact-context service
/// used for fresh poll and webhook dispatches.
pub async fn recovered_job_from_assignment_with_artifact_context<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
    assignment: &temper_workflow::DurableAssignment,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    artifact_context: &crate::ArtifactContextBundleService,
) -> Result<WorkItemJob, String> {
    recovered_job_from_assignment_inner(
        forge,
        repo,
        target,
        assignment,
        workflow,
        compiled,
        Some(artifact_context),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn recovered_job_from_assignment_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
    assignment: &temper_workflow::DurableAssignment,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    artifact_context: Option<&crate::ArtifactContextBundleService>,
) -> Result<WorkItemJob, String> {
    let role = assignment
        .role
        .clone()
        .ok_or_else(|| "durable assignment is missing role".to_string())?;
    let queue = assignment
        .queue
        .clone()
        .filter(|queue| !queue.trim().is_empty())
        .ok_or_else(|| "durable assignment is missing queue".to_string())?;
    let body = match target {
        ArtifactSource::Issue { number } => forge
            .get_issue_by_number(repo, number)
            .await
            .map_err(|error| error.to_string())?
            .map(|issue| issue.body),
        ArtifactSource::PullRequest { number } => forge
            .get_pull_request_by_number(repo, number)
            .await
            .map_err(|error| error.to_string())?
            .map(|pull_request| pull_request.body),
    }
    .ok_or_else(|| "durable assignment target no longer exists".to_string())?;
    let kind = parse_metadata_block(&body)
        .map_err(|error| error.to_string())?
        .and_then(|metadata| metadata.kind)
        .ok_or_else(|| "durable assignment target is missing workflow kind".to_string())?;
    let item = WorkItem {
        queue: temper_workflow::QueueId::new(queue),
        role,
        target,
        kind,
    };
    let repo_label = repo_label(forge, repo)
        .await
        .map_err(|error| error.to_string())?;
    let mut job = job_from_work_item(&repo_label, &item);
    match enrich_work_item_job_inner(
        forge,
        repo,
        &item,
        &mut job,
        workflow,
        compiled,
        true,
        artifact_context,
        None,
        None,
    )
    .await
    .map_err(|error| error.to_string())?
    {
        EnrichOutcome::Enriched => {}
        outcome => {
            return Err(format!(
                "recovered target is no longer dispatchable: {outcome:?}"
            ));
        }
    }
    if assignment.job_id.as_deref() != Some(job.job_id.as_str()) {
        return Err("durable assignment job id does not match current target".to_string());
    }
    let context: JobContext = serde_json::from_value(job.job_payload.clone())
        .map_err(|error| format!("recovered job context is invalid: {error}"))?;
    if assignment.action.as_deref() != context.action.as_deref() {
        return Err("durable assignment action no longer matches workflow".to_string());
    }
    if assignment.coordination_key.as_deref()
        != context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.as_str())
    {
        return Err("durable assignment coordination key no longer matches target".to_string());
    }
    Ok(job)
}

/// Adds Forge-backed workspace context to a mapped job.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn enrich_work_item_job<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    item: &WorkItem,
    job: &mut WorkItemJob,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
) -> Result<EnrichOutcome, ScanError> {
    enrich_work_item_job_inner(
        forge, repo, item, job, workflow, compiled, false, None, None, None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn enrich_work_item_job_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    item: &WorkItem,
    job: &mut WorkItemJob,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    recovering_assignment: bool,
    artifact_context: Option<&crate::ArtifactContextBundleService>,
    repository: Option<&temper_forge::Repository>,
    targeted: Option<TargetedEnrichment<'_>>,
) -> Result<EnrichOutcome, ScanError> {
    let repository = match targeted.as_ref() {
        Some(targeted) => targeted.repository.clone(),
        None => match repository {
            Some(repository) => repository.clone(),
            None => forge.get_repository(repo).await?.ok_or_else(|| {
                ScanError::Forge(ForgeError::NotFound(format!("repository {repo}")))
            })?,
        },
    };

    let number = target_number(item.target);
    let artifact = match targeted.as_ref() {
        Some(targeted) => {
            if targeted.snapshot.source() != item.target
                || targeted.classified.source != item.target
            {
                return Err(ScanError::InvalidWorkflow(
                    "targeted enrichment snapshot does not match work item".to_string(),
                ));
            }
            let Some(snapshot) = targeted::job_snapshot(targeted.snapshot) else {
                return Ok(EnrichOutcome::SkipTerminalArtifact);
            };
            snapshot
        }
        None => {
            let Some(snapshot) = terminal_checked_snapshot(forge, repo, item.target).await? else {
                return Ok(EnrichOutcome::SkipTerminalArtifact);
            };
            snapshot
        }
    };
    // Deterministic failures are durably parked with this engine-owned
    // attention label. Exclude it before any custom queue/action can dispatch
    // the unchanged artifact again.
    if artifact.labels.iter().any(|label| label == "needs-human") {
        return Ok(EnrichOutcome::SkipAttentionArtifact);
    }

    // Assemble the job's workspace manifest: the primary (writable) repo, plus
    // any additional repos the coordinating issue declares in a `temper:workspace`
    // metadata block (ADR 0023). Absent that block, the manifest is a single
    // writable primary — the degenerate single-repo job. PR-targeted jobs inherit
    // the implementation PR's stamped correlation key when present so PR-head
    // repair work shares the source issue's engineer workstream.
    let coordination_key = inherited_pull_request_coordination_key(item, &artifact.body)
        .unwrap_or_else(|| pr_correlation_key(&item.kind, number));
    let branch_hint = pr_branch_hint(&item.kind, number);
    let target_base_branch = issue_metadata_target_branch(item, &artifact.body);
    let workspace = build_workspace_manifest(
        forge,
        &repository,
        &job.repo,
        &coordination_key,
        &branch_hint,
        &artifact.body,
        target_base_branch.as_deref(),
    )
    .await?;

    let source_metadata = crate::verdict_contract::source_metadata_from_snapshot(Some(&artifact));
    let mut context = JobContext {
        trace_context: None,
        artifact_context: None,
        role: job.role.clone(),
        repo: job.repo.clone(),
        queue: item.queue.as_str().to_string(),
        artifact_kind: item.kind.as_str().to_string(),
        artifact: Some(artifact),
        workspace: Some(workspace),
        action: None,
        checkout_capability: None,
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata,
        guidance: None,
        pull_request_freshness: None,
    };
    enrich_job_context_from_workflow(item, workflow, compiled, &mut context)?;

    let action = context.action.as_deref().ok_or_else(|| {
        ScanError::InvalidWorkflow("enriched job is missing a selected action".to_string())
    })?;
    context.artifact_context = Some(
        targeted::resolve_artifact_context(
            forge,
            &repository,
            &job.repo,
            repo,
            workflow,
            item,
            action,
            artifact_context,
            targeted.as_ref(),
        )
        .await?,
    );

    // A pull-request writable job is an in-place PR-head fix: the worker checks
    // out the PR's real head branch and pushes the agent's fix back to that same
    // branch so native CI/review gates can re-evaluate. The selected queue action
    // declares when this mode is appropriate; the enrichment only materializes
    // the PR-head checkout and action-specific guidance.
    if context.checkout_capability.as_deref() == Some("pull_request_writable") {
        let preloaded = targeted
            .as_ref()
            .and_then(|targeted| match targeted.snapshot {
                TargetedArtifactSnapshot::PullRequest(pull_request) => Some(pull_request.as_ref()),
                TargetedArtifactSnapshot::Issue(_) => None,
            });
        enrich_pull_request_writable_job(forge, repo, item, compiled, &mut context, preloaded)
            .await?;
    }

    if !recovering_assignment
        && is_writable_issue_job(item, &context)
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
    SkipAttentionArtifact,
    SkipExistingPullRequest,
}

fn is_writable_issue_job(item: &WorkItem, context: &JobContext) -> bool {
    matches!(item.target, ArtifactSource::Issue { .. })
        && context.checkout_capability.as_deref() == Some("writable")
}

fn inherited_pull_request_coordination_key(item: &WorkItem, artifact_body: &str) -> Option<String> {
    if !matches!(item.target, ArtifactSource::PullRequest { .. }) {
        return None;
    }

    parse_metadata_block(artifact_body)
        .ok()
        .flatten()
        .and_then(|metadata| metadata.correlation_key)
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

fn issue_metadata_target_branch(item: &WorkItem, artifact_body: &str) -> Option<String> {
    if !matches!(item.target, ArtifactSource::Issue { .. }) {
        return None;
    }

    parse_metadata_block(artifact_body)
        .ok()
        .flatten()
        .and_then(|metadata| metadata.target_branch)
        .and_then(|branch| {
            let trimmed = branch.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
}

/// Recovers a worker-pushed PR head that became visible before its result was
/// published. Durable assignment fencing and atomic transition publication are
/// owned by the workflow layer so startup and live recovery use the same path.
pub async fn recover_advanced_pull_request_assignment_from_durable<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
    assignment: &temper_workflow::DurableAssignment,
    kind: temper_workflow::ArtifactKindId,
    workflow: &ValidatedWorkflow,
) -> Result<bool, ScanError> {
    temper_workflow::recover_advanced_pull_request_assignment_from_durable(
        forge, repo, target, assignment, kind, workflow,
    )
    .await
    .map_err(assignment_convergence_scan_error)
}

async fn recover_advanced_pull_request_assignment<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    item: &WorkItem,
    workflow: &ValidatedWorkflow,
) -> Result<bool, ScanError> {
    let ArtifactSource::PullRequest { number } = item.target else {
        return Ok(false);
    };
    let Some(pull_request) = forge.get_pull_request_by_number(repo, number).await? else {
        return Ok(false);
    };
    let assignment = parse_metadata_block(&pull_request.body)
        .map_err(|error| ScanError::InvalidWorkflow(error.to_string()))?
        .and_then(|metadata| metadata.assignment);
    let Some(assignment) = assignment else {
        return Ok(false);
    };
    recover_advanced_pull_request_assignment_from_durable(
        forge,
        repo,
        item.target,
        &assignment,
        item.kind.clone(),
        workflow,
    )
    .await
}

fn assignment_convergence_scan_error(
    error: temper_workflow::AssignmentConvergenceError,
) -> ScanError {
    match error {
        temper_workflow::AssignmentConvergenceError::Forge(error) => ScanError::Forge(error),
        temper_workflow::AssignmentConvergenceError::Lease(error) => {
            ScanError::InvalidWorkflow(error.to_string())
        }
        temper_workflow::AssignmentConvergenceError::InvalidContract(reason) => {
            ScanError::InvalidWorkflow(reason)
        }
    }
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
        EnrichOutcome::SkipTerminalArtifact => "terminal",
        EnrichOutcome::SkipAttentionArtifact => "attention",
        EnrichOutcome::SkipExistingPullRequest => "existing-pr",
    }
}

pub(crate) fn skip_log_line(
    repo_label: &str,
    role: &RoleId,
    item: &WorkItem,
    reason: EnrichOutcome,
) -> String {
    format!(
        "engine: skip {} {} role={} queue={} kind={}",
        skip_log_reason(reason),
        target_ref(repo_label, item.target),
        role.as_str(),
        item.queue.as_str(),
        item.kind.as_str(),
    )
}

pub(crate) fn enrichment_failure_log_line(
    repo_label: &str,
    role: &RoleId,
    item: &WorkItem,
    error: &ScanError,
) -> String {
    format!(
        "engine: skip enrich-failed {} role={} queue={} kind={}: {error}",
        target_ref(repo_label, item.target),
        role.as_str(),
        item.queue.as_str(),
        item.kind.as_str(),
    )
}

fn target_ref(repo_label: &str, target: ArtifactSource) -> String {
    match target {
        ArtifactSource::Issue { number } => format!("{}#{}", repo_label, number.get()),
        ArtifactSource::PullRequest { number } => format!("{} PR#{}", repo_label, number.get()),
    }
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
    recover_advanced_pull_request_assignments(daemon, forge, repo, workflow, role).await?;
    let repo_label = repo_label(forge, repo).await?;
    let items: Vec<WorkItem> = match mode {
        RoleFeedMode::Normal => scan_role(forge, repo, workflow, compiled, now, role).await?,
        RoleFeedMode::Wake => scan_role_wake(forge, repo, workflow, compiled, now, role).await?,
    };
    let mut enqueued = 0;
    let mut current_job_ids = BTreeSet::new();
    for item in &items {
        let mut job = job_from_work_item(&repo_label, item);
        match enrich_work_item_job_inner(
            forge,
            repo,
            item,
            &mut job,
            workflow,
            compiled,
            false,
            daemon.artifact_context.as_deref(),
            None,
            None,
        )
        .await
        {
            Ok(EnrichOutcome::Enriched) => {
                current_job_ids.insert(job.job_id.clone());
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
                outcome @ (EnrichOutcome::SkipTerminalArtifact
                | EnrichOutcome::SkipAttentionArtifact
                | EnrichOutcome::SkipExistingPullRequest),
            ) => {
                tracing::debug!("{}", skip_log_line(&repo_label, role, item, outcome));
            }
            Err(error) => tracing::debug!(
                "{}",
                enrichment_failure_log_line(&repo_label, role, item, &error)
            ),
        }
    }
    daemon
        .reconcile_pending_role_jobs(&repo_label, role.as_str(), current_job_ids)
        .await;
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
