// SPDX-License-Identifier: MPL-2.0

//! Standalone async daemon transport for the Worker/Daemon wire protocol.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use temper_forge::{
    BranchRef, CreateComment, CreatePullRequest, Forge, ForgeError, Issue, IssueState, ItemNumber,
    PullRequest, PullRequestState, Repository, RepositoryId, RepositoryPath, UpdateIssue,
};
use temper_runner::{
    pr_branch_hint, pr_correlation_key, scan_role, scan_role_wake, workspace_content_key,
    ScanError, WorkItem,
};
use temper_worker_protocol::{
    Artifact, Assign, ErrorCode, FailureClass, JobResult, Poll, ResultStatus, WorkerProtocolMessage,
};
#[cfg(test)]
use temper_worker_registry::daemon_core::QueuedJob;
use temper_worker_registry::DaemonCore;
// Public so out-of-crate `ResultApplier` implementations can name the job type
// the trait passes them.
pub use temper_worker_registry::InFlightJob;
use temper_workflow::{
    find_pull_request_by_correlation, ArtifactKindId, ArtifactSource, Classifier, CompiledWorkflow,
    Effect, ExecutionContext, ExecutionError, Executor, LeaseError, LeaseManager, LeasePolicy,
    RoleId, ToolManifest, ValidatedWorkflow, VerdictId,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::{sleep_until, Instant as TokioInstant},
};

pub mod config;
pub mod mechanical;
mod webhook;

pub use config::{parse, DaemonRunConfig, ParseOutcome, USAGE};
pub use mechanical::{
    run_mechanical_backstop, run_mechanical_backstop_tick, MechanicalBackstopConfig,
};
pub use temper_runner::{RepositorySet, RepositoryTarget};
pub use temper_worker_protocol::{JobArtifactSnapshot, JobContext, JobRepository};
pub use webhook::*;

pub const DEFAULT_MAX_POLL_WAIT_MS: u64 = 30_000;
const APPLY_GRACE: Duration = Duration::from_secs(10);

/// Which read-only scan the daemon feed runs for a role.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoleFeedMode {
    /// Steady-state active-queue scan (`scan_role`). This is the default mode.
    #[default]
    Normal,
    /// Wake-triggered scan (`scan_role_wake`).
    Wake,
}

/// A configured repository/role feed target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleFeedTarget {
    pub repo: RepositoryId,
    pub role: RoleId,
    pub mode: RoleFeedMode,
}

/// Configured target set and fixed delay between poll-backstop passes.
#[derive(Clone, Debug)]
pub struct PollBackstopConfig {
    /// Targets scanned in order on each cadence tick.
    pub targets: Vec<RoleFeedTarget>,
    /// Delay after one complete pass before the next pass starts.
    pub cadence: Duration,
}

/// Pluggable seam invoked when the daemon accepts a worker `result`.
///
/// The default implementation is a no-op. Use [`LeaseApplier`] to compose a
/// lease-gated Forge decorator around a concrete role-authored applier.
/// Implementations are invoked off the serial core task, so they may perform
/// async I/O without blocking the single-owner `DaemonCore` loop.
#[async_trait::async_trait]
pub trait ResultApplier: Send + Sync {
    async fn apply(&self, job: InFlightJob, result: JobResult);
}

/// Default applier that preserves existing daemon transport behavior.
#[derive(Debug, Default)]
pub struct NoopApplier;

#[async_trait::async_trait]
impl ResultApplier for NoopApplier {
    async fn apply(&self, _job: InFlightJob, _result: JobResult) {}
}

/// Routes each applied result to the applier registered for the job's role,
/// falling back to the default applier for unknown roles.
pub struct RoleRoutingApplier {
    routes: BTreeMap<String, Arc<dyn ResultApplier>>,
    default: Arc<dyn ResultApplier>,
}

impl RoleRoutingApplier {
    pub fn new(default: Arc<dyn ResultApplier>) -> Self {
        Self {
            routes: BTreeMap::new(),
            default,
        }
    }

    pub fn with_route(mut self, role: impl Into<String>, applier: Arc<dyn ResultApplier>) -> Self {
        self.routes.insert(role.into(), applier);
        self
    }
}

#[async_trait::async_trait]
impl ResultApplier for RoleRoutingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        match self.routes.get(&job.role) {
            Some(applier) => applier.apply(job, result).await,
            None => self.default.apply(job, result).await,
        }
    }
}

/// Forge-backed applier for daemon-accepted worker results.
///
/// Successful issue-targeted worker results carrying a branch are turned into the
/// same implementation-PR creation input the runner workspace paths use, then
/// passed to [`Executor::ensure_pull_request`] with the deterministic workspace
/// correlation key. Permanent/protocol worker failures mark the source issue for
/// human attention and add an audit comment. It deliberately does not acquire or
/// release leases; compose it under [`LeaseApplier`] when real daemon
/// application is enabled.
pub struct ForgeApplier<F: Forge> {
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: CompiledWorkflow,
    attention_labels: Vec<String>,
}

impl<F: Forge> ForgeApplier<F> {
    pub fn new(forge: Arc<F>, workflow: Arc<ValidatedWorkflow>) -> Self {
        let compiled = workflow.compile();
        Self {
            forge,
            workflow,
            compiled,
            attention_labels: vec!["needs-human".to_string()],
        }
    }

    pub fn with_attention_labels(mut self, labels: Vec<String>) -> Self {
        let labels = labels
            .into_iter()
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>();
        self.attention_labels = if labels.is_empty() {
            vec!["needs-human".to_string()]
        } else {
            labels
        };
        self
    }

    async fn apply_success(&self, job: InFlightJob, result: JobResult) {
        if result.verdict.is_some() {
            self.apply_verdict(job, result).await;
            return;
        }

        let Some(branch) = result.branch else {
            eprintln!(
                "temper-daemon: forge applier ignored success result without branch for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return;
        };
        if branch.name.trim().is_empty() {
            eprintln!(
                "temper-daemon: forge applier ignored success result with blank branch for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return;
        }
        let branch_name = branch.name;

        let Some((repository, issue)) = self.resolve_issue(&job).await else {
            return;
        };
        let number = issue.number;

        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not parse JobContext for job_id={} repo={} issue={}: {error}",
                    job.job_id, job.repo, number
                );
                return;
            }
        };
        let source_kind = ArtifactKindId::new(context.artifact_kind);

        let base_branch = if repository.default_branch.trim().is_empty() {
            "main".to_string()
        } else {
            repository.default_branch.clone()
        };
        let labels = implementation_pr_labels(self.workflow.as_ref());
        let summary = result.summary.unwrap_or_default();
        let input = implementation_pr_pull_request_input(
            repository.id.clone(),
            number,
            &issue.title,
            branch_name,
            base_branch,
            &summary,
            labels,
        );
        let correlation_key = pr_correlation_key(&source_kind, number);

        if let Err(error) = Executor::new(self.workflow.as_ref(), self.forge.as_ref())
            .ensure_pull_request(&repository.id, &correlation_key, input)
            .await
        {
            eprintln!(
                "temper-daemon: forge applier ensure_pull_request failed for job_id={} repo={} issue={} correlation_key={}: {error}",
                job.job_id, job.repo, number, correlation_key
            );
        }
    }

    async fn apply_verdict(&self, job: InFlightJob, result: JobResult) {
        let Some(verdict) = result.verdict.clone() else {
            return;
        };
        let Some((repository, issue)) = self.resolve_issue(&job).await else {
            return;
        };
        let number = issue.number;

        let job_context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not parse JobContext for job_id={} repo={} issue={}: {error}",
                    job.job_id, job.repo, number
                );
                return;
            }
        };
        let Some(action) = job_context.action.as_deref() else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} issue={} role={} verdict={}: missing action in JobContext",
                job.job_id, job.repo, number, job.role, verdict
            );
            return;
        };

        let role_id = RoleId::new(job.role.as_str());
        let verdict_id = VerdictId::new(verdict.as_str());
        let Some(role) = self.compiled.role(&role_id) else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} issue={} role={} action={} verdict={}: role not found in compiled workflow",
                job.job_id, job.repo, number, job.role, action, verdict
            );
            return;
        };
        let Some(tool) = role.tools.iter().find(|tool| tool.name == action) else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} issue={} role={} action={} verdict={}: action not found in compiled workflow",
                job.job_id, job.repo, number, job.role, action, verdict
            );
            return;
        };
        let Some(routed) = tool.outcomes.get(&verdict_id).cloned() else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} issue={} role={} action={} verdict={}: verdict is not declared for action",
                job.job_id, job.repo, number, job.role, action, verdict
            );
            return;
        };

        let source_kind = ArtifactKindId::new(job_context.artifact_kind.as_str());
        // A routed outcome such as intake -> code changes identifying labels. On
        // replay the current kind no longer matches the queued source kind, so
        // treat it as stale before the executor would classify the request as a
        // validation error.
        if matches!(
            Classifier::new(self.workflow.as_ref()).classify_issue(&issue),
            Ok(classified) if classified.kind != source_kind
        ) {
            return;
        }

        let mut context = ExecutionContext::new();
        if let Some(body) = result.body.clone() {
            let content_key = workspace_content_key(
                &ArtifactKindId::new(job_context.artifact_kind.as_str()),
                &routed,
                number,
            );
            context.set_set_body_at(routed.clone(), 0, body);
            context.set_set_body_correlation_key_at(routed.clone(), 0, content_key);
        }

        match Executor::with_context(self.workflow.as_ref(), self.forge.as_ref(), context)
            .execute(
                &repository.id,
                ArtifactSource::Issue { number },
                &routed,
                &role_id,
            )
            .await
        {
            Ok(_) => {}
            Err(error) if is_stale(&error) => {}
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not apply routed verdict transition for job_id={} repo={} issue={} role={} action={} verdict={} routed={}: {error}",
                    job.job_id, job.repo, number, job.role, action, verdict, routed
                );
            }
        }
    }

    async fn apply_failure(&self, job: InFlightJob, result: JobResult) {
        let failure = result.failure.as_ref();
        let class = match failure.map(|failure| failure.class) {
            Some(FailureClass::Permanent) => "permanent",
            Some(FailureClass::Protocol) => "protocol",
            None => "unknown",
            Some(FailureClass::Transient | FailureClass::Canceled) => return,
        };

        let Some((_repository, issue)) = self.resolve_issue(&job).await else {
            return;
        };

        if self
            .attention_labels
            .iter()
            .all(|label| issue.labels.iter().any(|existing| existing == label))
        {
            return;
        }

        let comment_marker = failure_audit_comment_marker(&result.job_id);
        match self.forge.list_issue_comments(&issue.id).await {
            Ok(comments) => {
                if comments
                    .iter()
                    .any(|comment| comment.body.contains(&comment_marker))
                {
                    return;
                }
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not list failed job audit comments for job_id={} repo={} issue={} failure_class={}: {error}",
                    job.job_id, job.repo, issue.number, class
                );
                return;
            }
        }

        if let Err(error) = self
            .forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    add_labels: self.attention_labels.clone(),
                    ..Default::default()
                },
            )
            .await
        {
            eprintln!(
                "temper-daemon: forge applier could not label failed job source issue for job_id={} repo={} issue={} failure_class={}: {error}",
                job.job_id, job.repo, issue.number, class
            );
            return;
        }

        let body = failure_audit_body(class, &result);
        if let Err(error) = self
            .forge
            .add_issue_comment(&issue.id, CreateComment { body })
            .await
        {
            eprintln!(
                "temper-daemon: forge applier could not add failed job audit comment for job_id={} repo={} issue={} failure_class={}: {error}",
                job.job_id, job.repo, issue.number, class
            );
        }
    }

    async fn resolve_issue(&self, job: &InFlightJob) -> Option<(Repository, Issue)> {
        if job.artifact.kind != "issue" {
            eprintln!(
                "temper-daemon: forge applier ignored non-issue job for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return None;
        }

        let Some(number) = job.artifact.item.as_u64().map(ItemNumber::new) else {
            eprintln!(
                "temper-daemon: forge applier ignored job with non-numeric issue item for job_id={} repo={} artifact.item={}",
                job.job_id, job.repo, job.artifact.item
            );
            return None;
        };

        let Some((owner, name)) = job.repo.split_once('/') else {
            eprintln!(
                "temper-daemon: forge applier ignored job with malformed repo path for job_id={} repo={}",
                job.job_id, job.repo
            );
            return None;
        };
        let repository = match self
            .forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await
        {
            Ok(Some(repository)) => repository,
            Ok(None) => {
                eprintln!(
                    "temper-daemon: forge applier repository not found for job_id={} repo={} issue={}",
                    job.job_id, job.repo, number
                );
                return None;
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier repository lookup failed for job_id={} repo={} issue={}: {error}",
                    job.job_id, job.repo, number
                );
                return None;
            }
        };

        let issue = match self.forge.get_issue_by_number(&repository.id, number).await {
            Ok(Some(issue)) => issue,
            Ok(None) => {
                eprintln!(
                    "temper-daemon: forge applier source issue not found for job_id={} repo={} issue={}",
                    job.job_id, job.repo, number
                );
                return None;
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier issue lookup failed for job_id={} repo={} issue={}: {error}",
                    job.job_id, job.repo, number
                );
                return None;
            }
        };

        Some((repository, issue))
    }
}

fn implementation_pr_pull_request_input(
    repo: RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    head_branch: String,
    base_branch: String,
    summary: &str,
    labels: Vec<String>,
) -> CreatePullRequest {
    let metadata = temper_workflow::WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![temper_workflow::ArtifactRef::same_repo(code_number)],
        ..temper_workflow::WorkflowMetadata::default()
    };
    let summary = summary.trim();
    let body = format!(
        "Workspace-produced implementation for issue #{code_number}.\n\nSummary: {}\n\n{}",
        if summary.is_empty() {
            "(none)"
        } else {
            summary
        },
        temper_workflow::render_metadata_block(&metadata)
    );
    CreatePullRequest {
        title: format!("Implement #{code_number}: {issue_title}"),
        body,
        source: BranchRef {
            repository_id: repo.clone(),
            branch: head_branch,
        },
        target: BranchRef {
            repository_id: repo,
            branch: base_branch,
        },
        labels,
        assignees: Vec::new(),
    }
}

#[async_trait::async_trait]
impl<F: Forge + 'static> ResultApplier for ForgeApplier<F> {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        match result.status {
            ResultStatus::Success => self.apply_success(job, result).await,
            ResultStatus::Failure => self.apply_failure(job, result).await,
        }
    }
}

const FAILURE_AUDIT_COMMENT_KEY_PREFIX: &str = "daemon_failure_audit:";

fn failure_audit_comment_marker(job_id: &str) -> String {
    format!("<!-- temper:comment-key={FAILURE_AUDIT_COMMENT_KEY_PREFIX}{job_id} -->")
}

fn failure_audit_body(class: &str, result: &JobResult) -> String {
    let header = format!(
        "Daemon could not complete this work (failure class: {class}).\n\njob_id: `{}`\nworker: `{}`",
        result.job_id, result.worker_id
    );

    let body = match result
        .failure
        .as_ref()
        .map(|failure| failure.message.trim())
    {
        Some(message) if !message.is_empty() => format!("{header}\n\n{message}"),
        _ => header,
    };
    format!(
        "{}\n\n{}",
        body,
        failure_audit_comment_marker(&result.job_id)
    )
}

fn is_stale(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::TargetStale { .. }
            | ExecutionError::Classification(_)
    )
}

/// Lease-gated [`ResultApplier`] decorator for daemon-owned result application.
///
/// The decorator resolves the completed worker job's Forge artifact, acquires
/// the workflow lease for that `(artifact, role)` as the daemon owner, invokes
/// the inner applier only while that lease is held, and then releases the lease
/// best-effort. Duplicate or double-dispatched results that lose the lease race
/// no-op without disturbing the peer's live lease.
pub struct LeaseApplier<F: Forge> {
    forge: Arc<F>,
    policy: LeasePolicy,
    owner: String,
    inner: Arc<dyn ResultApplier>,
}

impl<F: Forge> LeaseApplier<F> {
    pub fn new(
        forge: Arc<F>,
        policy: LeasePolicy,
        owner: impl Into<String>,
        inner: Arc<dyn ResultApplier>,
    ) -> Self {
        Self {
            forge,
            policy,
            owner: owner.into(),
            inner,
        }
    }
}

#[async_trait::async_trait]
impl<F: Forge + 'static> ResultApplier for LeaseApplier<F> {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let Some((repo_id, target)) = resolve_target(self.forge.as_ref(), &job).await else {
            eprintln!(
                "temper-daemon: lease applier could not resolve target for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return;
        };

        let manager = LeaseManager::new(self.forge.as_ref(), self.policy);
        match manager
            .acquire(
                &repo_id,
                target,
                RoleId::new(job.role.clone()),
                self.owner.clone(),
                Utc::now(),
            )
            .await
        {
            Ok(_) => {}
            Err(LeaseError::Conflict(_) | LeaseError::Contended { .. }) => return,
            Err(error) => {
                eprintln!(
                    "temper-daemon: lease applier could not acquire lease for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                    job.job_id, job.repo, job.artifact.kind, job.artifact.item
                );
                return;
            }
        }

        self.inner.apply(job.clone(), result).await;

        if let Err(error) = manager.release(&repo_id, target, &self.owner).await {
            eprintln!(
                "temper-daemon: lease applier could not release lease for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
        }
    }
}

async fn resolve_target<F: Forge + ?Sized>(
    forge: &F,
    job: &InFlightJob,
) -> Option<(RepositoryId, ArtifactSource)> {
    let (owner, name) = job.repo.split_once('/')?;

    let repository = match forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await
    {
        Ok(Some(repository)) => repository,
        Ok(None) => return None,
        Err(error) => {
            eprintln!(
                "temper-daemon: lease applier repository lookup failed for job_id={} repo={}: {error}",
                job.job_id, job.repo
            );
            return None;
        }
    };

    let number = job.artifact.item.as_u64().map(ItemNumber::new)?;
    let target = match job.artifact.kind.as_str() {
        "issue" => ArtifactSource::Issue { number },
        "pull_request" => ArtifactSource::PullRequest { number },
        _ => return None,
    };

    Some((repository.id, target))
}

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
        repository: None,
        base_branch: None,
        branch_hint: None,
        correlation_key: None,
        artifact: None,
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
/// coding agent needs: repository coordinates, base branch, branch hint,
/// correlation key, and an artifact snapshot. Forge reads happen here so
/// `job_from_work_item` stays pure.
async fn enrich_work_item_job<F: Forge + ?Sized>(
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

    let base_branch = default_base_branch(&repository);
    let number = target_number(item.target);
    let Some(artifact) = terminal_checked_snapshot(forge, repo, item.target).await? else {
        return Ok(EnrichOutcome::SkipTerminalArtifact);
    };

    let mut context = JobContext {
        role: job.role.clone(),
        repo: job.repo.clone(),
        queue: item.queue.as_str().to_string(),
        artifact_kind: item.kind.as_str().to_string(),
        repository: Some(JobRepository {
            owner: repository.owner,
            name: repository.name,
            default_branch: repository.default_branch,
        }),
        base_branch: Some(base_branch),
        branch_hint: Some(pr_branch_hint(&item.kind, number)),
        correlation_key: Some(pr_correlation_key(&item.kind, number)),
        artifact: Some(artifact),
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
enum EnrichOutcome {
    Enriched,
    SkipTerminalArtifact,
    SkipExistingPullRequest,
}

fn default_base_branch(repository: &Repository) -> String {
    if repository.default_branch.trim().is_empty() {
        "main".to_string()
    } else {
        repository.default_branch.clone()
    }
}

fn target_number(target: ArtifactSource) -> ItemNumber {
    match target {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number,
    }
}

async fn terminal_checked_snapshot<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    target: ArtifactSource,
) -> Result<Option<JobArtifactSnapshot>, ScanError> {
    match target {
        ArtifactSource::Issue { number } => {
            let issue = forge
                .get_issue_by_number(repo, number)
                .await?
                .ok_or_else(|| ScanError::Forge(ForgeError::NotFound(format!("issue {number}"))))?;
            if issue.state != IssueState::Open {
                return Ok(None);
            }
            Ok(Some(snapshot_from_issue(issue)))
        }
        ArtifactSource::PullRequest { number } => {
            let pull_request = forge
                .get_pull_request_by_number(repo, number)
                .await?
                .ok_or_else(|| {
                    ScanError::Forge(ForgeError::NotFound(format!("pull request {number}")))
                })?;
            if pull_request.state != PullRequestState::Open {
                return Ok(None);
            }
            Ok(Some(snapshot_from_pull_request(pull_request)))
        }
    }
}

fn snapshot_from_issue(issue: Issue) -> JobArtifactSnapshot {
    JobArtifactSnapshot {
        number: issue.number.get(),
        title: issue.title,
        body: issue.body,
        labels: issue.labels,
        state: format!("{:?}", issue.state),
    }
}

fn snapshot_from_pull_request(pull_request: PullRequest) -> JobArtifactSnapshot {
    JobArtifactSnapshot {
        number: pull_request.number.get(),
        title: pull_request.title,
        body: pull_request.body,
        labels: pull_request.labels,
        state: format!("{:?}", pull_request.state),
    }
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
    let Some(correlation_key) = context.correlation_key.as_deref() else {
        return Ok(false);
    };
    let labels = implementation_pr_labels(workflow);
    let pull_request = find_pull_request_by_correlation(forge, repo, correlation_key, &labels)
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

fn implementation_pr_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    workflow
        .artifact_kind(&ArtifactKindId::new("implementation_pr"))
        .map(|kind| {
            kind.identifying_labels
                .iter()
                .map(|label| label.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn skip_log_reason(outcome: EnrichOutcome) -> &'static str {
    match outcome {
        EnrichOutcome::Enriched => "",
        EnrichOutcome::SkipTerminalArtifact => "terminal artifact",
        EnrichOutcome::SkipExistingPullRequest => "existing implementation pull request",
    }
}

fn skip_log_line(
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

fn enrichment_failure_log_line(
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
    !tool.outcomes.is_empty() || create_pull_request_count(tool) > 0
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

#[derive(Clone)]
struct DaemonState {
    cmd_tx: mpsc::UnboundedSender<DaemonCommand>,
    max_poll_wait_ms: u64,
}

#[derive(Clone)]
pub struct Daemon {
    cmd_tx: mpsc::UnboundedSender<DaemonCommand>,
    max_poll_wait_ms: u64,
}

enum DaemonCommand {
    Message {
        msg: WorkerProtocolMessage,
        reply: oneshot::Sender<Option<WorkerProtocolMessage>>,
    },
    Result {
        result: JobResult,
        reply: oneshot::Sender<Option<WorkerProtocolMessage>>,
    },
    Poll {
        poll: Poll,
        deadline: TokioInstant,
        reply: oneshot::Sender<WorkerProtocolMessage>,
    },
    ExpirePoll {
        id: u64,
    },
    EnqueueJob {
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    },
    ApplyFinished {
        job_id: String,
    },
    SetApplyGrace {
        apply_grace: Duration,
    },
    #[cfg(test)]
    QueuedJobs {
        reply: oneshot::Sender<Vec<QueuedJob>>,
    },
}

struct PollWaiter {
    poll: Poll,
    reply: oneshot::Sender<WorkerProtocolMessage>,
}

async fn run_core(
    mut rx: mpsc::UnboundedReceiver<DaemonCommand>,
    cmd_tx: mpsc::UnboundedSender<DaemonCommand>,
    applier: Arc<dyn ResultApplier>,
    apply_grace: Duration,
) {
    let mut core = DaemonCore::new();
    let mut waiters = BTreeMap::new();
    let mut applying = BTreeSet::new();
    let mut recently_applied = BTreeMap::new();
    let mut apply_grace = apply_grace;
    let mut next_id = 0_u64;

    while let Some(command) = rx.recv().await {
        match command {
            DaemonCommand::Message { msg, reply } => {
                let _ = reply.send(core.handle(msg));
            }
            DaemonCommand::Result { result, reply } => {
                // Capture full job context before the core completes and forgets the job.
                let in_flight = core.in_flight_job(&result.job_id);
                let response = core.handle(WorkerProtocolMessage::Result(result.clone()));

                // Route only when the core accepted/completed the in-flight job.
                // Unknown, never-assigned, version-mismatched, and double-sent
                // results must not apply, retry, or drop beyond the core response.
                if let (Some(job), Some(WorkerProtocolMessage::Release(_))) =
                    (in_flight, response.as_ref())
                {
                    let disposition = result_disposition(&result);
                    let line = result_received_log_line(
                        &result,
                        result_disposition_log_value(disposition),
                    );
                    eprintln!("{line}");

                    match disposition {
                        ResultDisposition::Apply => {
                            let job_id = job.job_id.clone();
                            applying.insert(job_id.clone());
                            let applier = applier.clone();
                            let apply_finished_tx = cmd_tx.clone();
                            tokio::spawn(async move {
                                applier.apply(job, result).await;
                                let _ =
                                    apply_finished_tx.send(DaemonCommand::ApplyFinished { job_id });
                            });
                        }
                        ResultDisposition::DropForRescan => {
                            // Let the next webhook wake or poll-backstop tick re-feed this
                            // through the guarded scan path instead of hot re-enqueuing.
                        }
                        ResultDisposition::Drop => {}
                    }
                }

                let _ = reply.send(response);
            }
            DaemonCommand::Poll {
                poll,
                deadline,
                reply,
            } => {
                let response = core
                    .handle(WorkerProtocolMessage::Poll(poll.clone()))
                    .expect("poll messages produce a response");

                if is_poll_timeout(&response) {
                    let id = next_id;
                    next_id = next_id.wrapping_add(1);
                    waiters.insert(id, PollWaiter { poll, reply });

                    let timer_tx = cmd_tx.clone();
                    tokio::spawn(async move {
                        sleep_until(deadline).await;
                        let _ = timer_tx.send(DaemonCommand::ExpirePoll { id });
                    });
                } else {
                    if let WorkerProtocolMessage::Assign(assign) = &response {
                        let line = assignment_log_line(assign, &poll.worker_id);
                        eprintln!("{line}");
                    }
                    let _ = reply.send(response);
                }
            }
            DaemonCommand::ExpirePoll { id } => {
                if let Some(waiter) = waiters.remove(&id) {
                    let response = core
                        .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
                        .expect("poll messages produce a response");
                    let _ = waiter.reply.send(response);
                }
            }
            DaemonCommand::EnqueueJob {
                job_id,
                role,
                repo,
                artifact,
                job_payload,
            } => {
                let now = Instant::now();
                recently_applied.retain(|_, deadline| *deadline > now);
                if applying.contains(&job_id) {
                    eprintln!(
                        "temper-daemon: skipped enqueue for job in apply window job_id={job_id}"
                    );
                    continue;
                }
                if recently_applied
                    .get(&job_id)
                    .is_some_and(|deadline| *deadline > now)
                {
                    eprintln!(
                        "temper-daemon: skipped enqueue for recently applied job job_id={job_id}"
                    );
                    continue;
                }
                core.enqueue_job(job_id, role, repo, artifact, job_payload);
                fulfil_waiters(&mut core, &mut waiters);
            }
            DaemonCommand::ApplyFinished { job_id } => {
                applying.remove(&job_id);
                recently_applied.insert(job_id, Instant::now() + apply_grace);
            }
            DaemonCommand::SetApplyGrace {
                apply_grace: new_apply_grace,
            } => {
                apply_grace = new_apply_grace;
            }
            #[cfg(test)]
            DaemonCommand::QueuedJobs { reply } => {
                let _ = reply.send(core.queued_jobs());
            }
        }
    }
}

fn fulfil_waiters(core: &mut DaemonCore, waiters: &mut BTreeMap<u64, PollWaiter>) {
    let ids = waiters.keys().copied().collect::<Vec<_>>();

    for id in ids {
        let Some(waiter) = waiters.get(&id) else {
            continue;
        };

        if waiter.reply.is_closed() {
            waiters.remove(&id);
            continue;
        }

        let response = core
            .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
            .expect("poll messages produce a response");

        if is_poll_timeout(&response) {
            continue;
        }

        let waiter = waiters
            .remove(&id)
            .expect("waiter exists after successful poll response");
        if let WorkerProtocolMessage::Assign(assign) = &response {
            let line = assignment_log_line(assign, &waiter.poll.worker_id);
            eprintln!("{line}");
        }
        let _ = waiter.reply.send(response);
    }
}

fn is_poll_timeout(message: &WorkerProtocolMessage) -> bool {
    matches!(
        message,
        WorkerProtocolMessage::Error(error) if error.code == ErrorCode::PollTimeout
    )
}

fn assignment_log_line(assign: &Assign, worker_id: &str) -> String {
    format!(
        "temper-daemon: assigned job_id={} role={} repo={} worker={}",
        assign.job_id, assign.role, assign.repo, worker_id
    )
}

fn result_received_log_line(result: &JobResult, disposition: &str) -> String {
    format!(
        "temper-daemon: result received job_id={} worker={} status={} disposition={}",
        result.job_id,
        result.worker_id,
        result_status_log_value(result),
        disposition
    )
}

fn result_status_log_value(result: &JobResult) -> String {
    match result.status {
        ResultStatus::Success => "success".to_string(),
        ResultStatus::Failure => {
            let class = result
                .failure
                .as_ref()
                .map(|failure| failure_class_log_value(failure.class))
                .unwrap_or("unknown");
            format!("failure({class})")
        }
    }
}

fn failure_class_log_value(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Transient => "transient",
        FailureClass::Permanent => "permanent",
        FailureClass::Canceled => "canceled",
        FailureClass::Protocol => "protocol",
    }
}

fn result_disposition_log_value(disposition: ResultDisposition) -> &'static str {
    match disposition {
        ResultDisposition::Apply => "apply",
        ResultDisposition::DropForRescan => "rescan",
        ResultDisposition::Drop => "drop",
    }
}

fn poll_backstop_log_line(enqueued: usize) -> String {
    format!("temper-daemon: poll backstop enqueued={enqueued}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultDisposition {
    Apply,
    DropForRescan,
    Drop,
}

fn result_disposition(result: &JobResult) -> ResultDisposition {
    match result.status {
        ResultStatus::Success => ResultDisposition::Apply,
        ResultStatus::Failure => match result.failure.as_ref().map(|failure| failure.class) {
            Some(FailureClass::Transient) => ResultDisposition::DropForRescan,
            Some(FailureClass::Canceled) => ResultDisposition::Drop,
            Some(FailureClass::Permanent | FailureClass::Protocol) | None => {
                ResultDisposition::Apply
            }
        },
    }
}

impl Daemon {
    pub fn new() -> Self {
        Self::with_applier(Arc::new(NoopApplier))
    }

    pub fn with_applier(applier: Arc<dyn ResultApplier>) -> Self {
        Self::with_applier_and_apply_grace(applier, APPLY_GRACE)
    }

    pub fn with_apply_grace(self, apply_grace: Duration) -> Self {
        let _ = self
            .cmd_tx
            .send(DaemonCommand::SetApplyGrace { apply_grace });
        self
    }

    fn with_applier_and_apply_grace(
        applier: Arc<dyn ResultApplier>,
        apply_grace: Duration,
    ) -> Self {
        let (cmd_tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run_core(rx, cmd_tx.clone(), applier, apply_grace));

        Self {
            cmd_tx,
            max_poll_wait_ms: DEFAULT_MAX_POLL_WAIT_MS,
        }
    }

    pub async fn enqueue_job(
        &self,
        job_id: impl Into<String>,
        role: impl Into<String>,
        repo: impl Into<String>,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) {
        let _ = self.cmd_tx.send(DaemonCommand::EnqueueJob {
            job_id: job_id.into(),
            role: role.into(),
            repo: repo.into(),
            artifact,
            job_payload,
        });
    }

    /// Map a scanned `WorkItem` to a job and enqueue it.
    pub async fn enqueue_work_item(&self, repo: &str, item: &WorkItem) {
        let job = job_from_work_item(repo, item);
        self.enqueue_job(
            job.job_id,
            job.role,
            job.repo,
            job.artifact,
            job.job_payload,
        )
        .await;
    }

    /// Scans `repo` for `role`'s active queue work and enqueues each resulting
    /// `WorkItem` into the daemon for dispatch. Returns the number of
    /// successfully enriched and enqueued jobs; the daemon/registry dedupes
    /// already-pending or in-flight jobs by `job_id`, so repeated feeds for an
    /// unchanged ready artifact do not double-dispatch.
    ///
    /// The protocol `repo` label is the artifact repository's `owner/name` path,
    /// matching worker registered capability `repo` values.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_scanned_role_work<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repo: &RepositoryId,
        workflow: &ValidatedWorkflow,
        compiled: &CompiledWorkflow,
        now: DateTime<Utc>,
        role: &RoleId,
        mode: RoleFeedMode,
    ) -> Result<usize, ScanError> {
        let repo_label = repo_label(forge, repo).await?;
        let items: Vec<WorkItem> = match mode {
            RoleFeedMode::Normal => scan_role(forge, repo, workflow, compiled, now, role).await?,
            RoleFeedMode::Wake => {
                scan_role_wake(forge, repo, workflow, compiled, now, role).await?
            }
        };
        let mut enqueued = 0;
        for item in &items {
            let mut job = job_from_work_item(&repo_label, item);
            match enrich_work_item_job(forge, repo, item, &mut job, workflow, compiled).await {
                Ok(EnrichOutcome::Enriched) => {
                    self.enqueue_job(
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

    #[cfg(test)]
    async fn queued_jobs(&self) -> Vec<WorkItemJob> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(DaemonCommand::QueuedJobs { reply })
            .expect("daemon core task is running");

        rx.await
            .expect("daemon core task replies with queued jobs")
            .into_iter()
            .map(|job| WorkItemJob {
                job_id: job.job_id,
                role: job.role,
                repo: job.repo,
                artifact: job.artifact,
                job_payload: job.job_payload,
            })
            .collect()
    }

    pub fn router(&self) -> Router {
        let state = DaemonState {
            cmd_tx: self.cmd_tx.clone(),
            max_poll_wait_ms: self.max_poll_wait_ms,
        };

        Router::new()
            .route("/v1/message", post(handle_message))
            .with_state(state)
    }
}

/// Runs one poll-backstop pass over the configured targets.
pub async fn run_poll_backstop_tick<F: Forge + ?Sized>(
    daemon: &Daemon,
    forge: &F,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    config: &PollBackstopConfig,
) -> usize {
    let mut total = 0;
    for target in &config.targets {
        match daemon
            .enqueue_scanned_role_work(
                forge,
                &target.repo,
                workflow,
                compiled,
                now,
                &target.role,
                target.mode,
            )
            .await
        {
            Ok(count) => total += count,
            Err(error) => eprintln!(
                "temper-daemon: poll backstop scan failed for repo={} role={}: {error}",
                target.repo,
                target.role.as_str()
            ),
        }
    }
    if total > 0 {
        let line = poll_backstop_log_line(total);
        eprintln!("{line}");
    }
    total
}

/// Runs a fixed-delay poll backstop forever.
pub async fn run_poll_backstop<F: Forge + ?Sized>(
    daemon: &Daemon,
    forge: &F,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    config: &PollBackstopConfig,
) {
    loop {
        run_poll_backstop_tick(daemon, forge, workflow, compiled, Utc::now(), config).await;
        tokio::time::sleep(config.cadence).await;
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
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

async fn handle_message(State(state): State<DaemonState>, body: Bytes) -> Response {
    let Ok(msg) = serde_json::from_slice::<WorkerProtocolMessage>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    match msg {
        WorkerProtocolMessage::Poll(poll) => {
            let requested = poll.max_wait_ms.unwrap_or(state.max_poll_wait_ms);
            let wait_ms = requested.min(state.max_poll_wait_ms);
            let deadline = TokioInstant::now() + Duration::from_millis(wait_ms);
            let (reply, rx) = oneshot::channel();

            if state
                .cmd_tx
                .send(DaemonCommand::Poll {
                    poll,
                    deadline,
                    reply,
                })
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            match rx.await {
                Ok(reply) => (StatusCode::OK, Json(reply)).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        WorkerProtocolMessage::Result(result) => {
            let (reply, rx) = oneshot::channel();

            if state
                .cmd_tx
                .send(DaemonCommand::Result { result, reply })
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            match rx.await {
                Ok(Some(reply)) => (StatusCode::OK, Json(reply)).into_response(),
                Ok(None) => StatusCode::NO_CONTENT.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        other => {
            let (reply, rx) = oneshot::channel();

            if state
                .cmd_tx
                .send(DaemonCommand::Message { msg: other, reply })
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            match rx.await {
                Ok(Some(reply)) => (StatusCode::OK, Json(reply)).into_response(),
                Ok(None) => StatusCode::NO_CONTENT.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}

pub async fn serve_router(router: Router, bind: SocketAddr) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!("temper-daemon: serving on {}", listener.local_addr()?);

    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("temper-daemon: failed to listen for shutdown signal: {error}");
            }
        })
        .await
}

pub async fn serve(daemon: &Daemon, bind: SocketAddr) -> std::io::Result<()> {
    serve_router(daemon.router(), bind).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use temper_forge::{
        BranchRef, CreatePullRequest, CreateRepository, Forge, ItemNumber, UpdateIssue,
        UpdatePullRequest,
    };
    use temper_forge_memory::MemoryForge;
    use temper_worker_protocol::{Artifact, Failure, ResultStatus, WORKER_PROTOCOL_VERSION};
    use temper_workflow::{
        render_metadata_block, ArtifactKindId, QueueId, RawWorkflowSpec, RoleId, WorkflowMetadata,
    };

    const BASIC_DELIVERY_FIXTURE: &str =
        include_str!("../../temper-workflow/fixtures/basic-delivery.json");

    fn work_item(target: ArtifactSource) -> WorkItem {
        WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target,
            kind: ArtifactKindId::new("code"),
        }
    }

    fn result_for_disposition(
        status: ResultStatus,
        failure_class: Option<FailureClass>,
    ) -> JobResult {
        JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            job_id: "job-1".to_string(),
            status,
            branch: None,
            verdict: None,
            body: None,
            failure: failure_class.map(|class| Failure {
                class,
                message: "worker failed".to_string(),
            }),
            summary: None,
            details: None,
        }
    }

    fn assign_for_log_line() -> Assign {
        Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id: "ai/temper/issue-147/engineer/code_ready".to_string(),
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(147),
                kind: "issue".to_string(),
            },
            job_payload: json!({"safe": "context"}),
        }
    }

    #[test]
    fn assignment_log_line_includes_worker_from_poll() {
        assert_eq!(
            assignment_log_line(&assign_for_log_line(), "worker-a"),
            "temper-daemon: assigned job_id=ai/temper/issue-147/engineer/code_ready role=engineer repo=ai/temper worker=worker-a"
        );
    }

    #[test]
    fn result_received_log_line_formats_success_status() {
        let result = result_for_disposition(ResultStatus::Success, None);

        assert_eq!(
            result_received_log_line(&result, "apply"),
            "temper-daemon: result received job_id=job-1 worker=worker-a status=success disposition=apply"
        );
    }

    #[test]
    fn result_received_log_line_formats_each_failure_class() {
        let cases = [
            (FailureClass::Transient, "transient", "rescan"),
            (FailureClass::Permanent, "permanent", "apply"),
            (FailureClass::Canceled, "canceled", "drop"),
            (FailureClass::Protocol, "protocol", "apply"),
        ];

        for (class, expected_class, disposition) in cases {
            let result = result_for_disposition(ResultStatus::Failure, Some(class));

            assert_eq!(
                result_received_log_line(&result, disposition),
                format!(
                    "temper-daemon: result received job_id=job-1 worker=worker-a status=failure({expected_class}) disposition={disposition}"
                )
            );
        }
    }

    #[test]
    fn skip_log_reason_names_existing_pull_request_without_state_qualifier() {
        assert_eq!(
            skip_log_reason(EnrichOutcome::SkipExistingPullRequest),
            "existing implementation pull request"
        );
    }

    #[test]
    fn skip_log_line_includes_existing_pull_request_reason() {
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(153),
        });

        assert_eq!(
            skip_log_line(
                "ai/temper",
                &RoleId::new("engineer"),
                &item,
                EnrichOutcome::SkipExistingPullRequest
            ),
            "temper-daemon: skipped role work for existing implementation pull request repo=ai/temper role=engineer queue=code_ready artifact_kind=code target=Issue { number: ItemNumber(153) }"
        );
    }

    #[tokio::test]
    async fn enrich_work_item_job_skips_merged_correlated_implementation_pr() {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let issue = forge
            .create_issue(
                &repo,
                temper_forge::CreateIssue {
                    title: "ready".to_string(),
                    body: "needs implementation".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        let correlation_key = format!("pr-for-code-{}", issue.number.get());
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement ready issue".to_string(),
                    body: format!(
                        "Implementation PR.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            correlation_key: Some(correlation_key.clone()),
                            ..WorkflowMetadata::default()
                        })
                    ),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: format!("agent/{correlation_key}"),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        forge
            .merge_pull_request(
                &pull_request.id,
                temper_forge::MergePullRequest {
                    method: temper_forge::MergeMethod::Squash,
                    commit_title: None,
                    commit_body: None,
                },
            )
            .await
            .expect("pull request is merged");
        let item = work_item(ArtifactSource::Issue {
            number: issue.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment skip succeeds"),
            EnrichOutcome::SkipExistingPullRequest
        );
    }

    #[test]
    fn poll_backstop_log_line_includes_enqueued_count() {
        assert_eq!(
            poll_backstop_log_line(5),
            "temper-daemon: poll backstop enqueued=5"
        );
    }

    #[test]
    fn result_disposition_routes_success_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(ResultStatus::Success, None)),
            ResultDisposition::Apply
        );
    }

    #[test]
    fn result_disposition_routes_transient_failure_to_drop_for_rescan() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Transient),
            )),
            ResultDisposition::DropForRescan
        );
    }

    #[test]
    fn result_disposition_routes_permanent_failure_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Permanent),
            )),
            ResultDisposition::Apply
        );
    }

    #[test]
    fn result_disposition_routes_protocol_failure_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Protocol),
            )),
            ResultDisposition::Apply
        );
    }

    #[test]
    fn result_disposition_routes_canceled_failure_to_drop() {
        assert_eq!(
            result_disposition(&result_for_disposition(
                ResultStatus::Failure,
                Some(FailureClass::Canceled),
            )),
            ResultDisposition::Drop
        );
    }

    #[test]
    fn result_disposition_routes_failure_without_details_to_apply() {
        assert_eq!(
            result_disposition(&result_for_disposition(ResultStatus::Failure, None)),
            ResultDisposition::Apply
        );
    }

    #[test]
    fn maps_issue_work_item_to_daemon_job() {
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(103),
        });

        let job = job_from_work_item("ai/temper", &item);

        assert_eq!(job.job_id, "ai/temper/issue-103/engineer/code_ready");
        assert_eq!(job.role, "engineer");
        assert_eq!(job.repo, "ai/temper");
        assert_eq!(
            job.artifact,
            Artifact {
                item: json!(103),
                kind: "issue".to_string(),
            }
        );
        assert_eq!(
            job.job_payload,
            json!({
                "role": "engineer",
                "repo": "ai/temper",
                "queue": "code_ready",
                "artifact_kind": "code"
            })
        );
        assert_eq!(
            serde_json::from_value::<JobContext>(job.job_payload).expect("valid JobContext"),
            JobContext {
                role: "engineer".to_string(),
                repo: "ai/temper".to_string(),
                queue: "code_ready".to_string(),
                artifact_kind: "code".to_string(),
                repository: None,
                base_branch: None,
                branch_hint: None,
                correlation_key: None,
                artifact: None,
                action: None,
                checkout_capability: None,
                allowed_verdicts: Vec::new(),
            }
        );
    }

    #[test]
    fn maps_pull_request_work_item_to_daemon_job() {
        let item = work_item(ArtifactSource::PullRequest {
            number: ItemNumber::new(42),
        });

        let job = job_from_work_item("ai/temper", &item);

        assert_eq!(job.artifact.kind, "pull_request");
        assert!(job.job_id.contains("/pull_request-42/"));
        assert_eq!(job.artifact.item, json!(42));
    }

    #[test]
    fn work_item_job_mapping_is_deterministic() {
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(103),
        });

        assert_eq!(
            job_from_work_item("ai/temper", &item),
            job_from_work_item("ai/temper", &item)
        );
    }

    #[tokio::test]
    async fn enqueue_work_item_stores_mapped_job() {
        let daemon = Daemon::new();
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(103),
        });
        let expected = job_from_work_item("ai/temper", &item);

        daemon.enqueue_work_item("ai/temper", &item).await;

        assert_eq!(daemon.queued_jobs().await, vec![expected]);
    }

    #[tokio::test]
    async fn enrich_work_item_job_skips_closed_issue() {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let issue = forge
            .create_issue(
                &repo,
                temper_forge::CreateIssue {
                    title: "closed".to_string(),
                    body: "done".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("issue is closed");
        let item = work_item(ArtifactSource::Issue {
            number: issue.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment skip succeeds"),
            EnrichOutcome::SkipTerminalArtifact
        );
    }

    #[tokio::test]
    async fn enrich_work_item_job_enriches_open_pull_request_artifact_snapshot() {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Fix failing CI".to_string(),
                    body: "Address the failing PR.".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-42".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        let item = work_item(ArtifactSource::PullRequest {
            number: pull_request.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds for pull request targets"),
            EnrichOutcome::Enriched
        );

        let context: JobContext =
            serde_json::from_value(job.job_payload).expect("enriched JobContext parses");
        assert_eq!(context.base_branch.as_deref(), Some("main"));
        assert_eq!(
            context.repository,
            Some(JobRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
            })
        );
        assert_eq!(
            context.branch_hint.as_deref(),
            Some(format!("agent/pr-for-code-{}", pull_request.number.get()).as_str())
        );
        assert_eq!(
            context.correlation_key.as_deref(),
            Some(format!("pr-for-code-{}", pull_request.number.get()).as_str())
        );
        let artifact = context.artifact.expect("pull request snapshot is present");
        assert_eq!(artifact.number, pull_request.number.get());
        assert_eq!(artifact.title, "Fix failing CI");
        assert_eq!(artifact.body, "Address the failing PR.");
        assert_eq!(artifact.labels, vec!["implementation".to_string()]);
        assert_eq!(artifact.state, "Open");
        assert_eq!(context.action.as_deref(), Some("open_pr"));
        assert_eq!(context.checkout_capability.as_deref(), Some("writable"));
        assert!(context.allowed_verdicts.is_empty());
    }

    #[tokio::test]
    async fn enrich_work_item_job_skips_closed_pull_request() {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "closed PR".to_string(),
                    body: "closed".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-7".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    state: Some(temper_forge::PullRequestUpdateState::Closed),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("pull request is closed");
        let item = work_item(ArtifactSource::PullRequest {
            number: pull_request.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment skip succeeds"),
            EnrichOutcome::SkipTerminalArtifact
        );
    }
}
