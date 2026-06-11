// SPDX-License-Identifier: MPL-2.0

//! Standalone async daemon transport for the Worker/Daemon wire protocol.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
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
    Artifact, Assign, ErrorCode, FailureClass, JobChild, JobResult, Poll, ResultStatus,
    WorkerProtocolMessage,
};
#[cfg(test)]
use temper_worker_registry::daemon_core::QueuedJob;
use temper_worker_registry::DaemonCore;
// Public so out-of-crate `ResultApplier` implementations can name the job type
// the trait passes them.
use temper_io_engine::http::{HttpRequestData, HttpResponder, HttpResponseData};
use temper_io_engine::{
    arm_timer, channel, drive, CqSender, EngineTime, Executor as EngineExecutor, Machine,
};
pub use temper_worker_registry::InFlightJob;
use temper_workflow::{
    find_pull_request_by_correlation, ArtifactKindId, ArtifactSource, Classifier, CompiledWorkflow,
    CreateIssuesChild, Effect, ExecutionContext, ExecutionError, Executor, LeaseError,
    LeaseManager, LeasePolicy, RoleId, ToolManifest, TransitionId, ValidatedWorkflow, VerdictId,
};

pub mod config;
pub mod mechanical;
mod webhook;

pub use config::{parse, DaemonRunConfig, ParseOutcome, USAGE};
pub use mechanical::{
    run_mechanical_backstop_tick, spawn_mechanical_backstop, MechanicalBackstopConfig,
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
        let lookup_labels = implementation_pr_labels(self.workflow.as_ref());
        let create_labels = implementation_pr_create_labels(self.workflow.as_ref());
        let summary = result.summary.unwrap_or_default();
        let input = implementation_pr_pull_request_input(
            repository.id.clone(),
            number,
            &issue.title,
            branch_name,
            base_branch,
            &summary,
            create_labels,
        );
        let correlation_key = pr_correlation_key(&source_kind, number);

        if let Err(error) = Executor::new(self.workflow.as_ref(), self.forge.as_ref())
            .ensure_pull_request_with_lookup(
                &repository.id,
                &correlation_key,
                &lookup_labels,
                input,
            )
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

        let job_context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not parse JobContext for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                    job.job_id, job.repo, job.artifact.kind, job.artifact.item
                );
                return;
            }
        };
        let Some(action) = job_context.action.as_deref() else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} verdict={}: missing action in JobContext",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item, job.role, verdict
            );
            return;
        };

        let role_id = RoleId::new(job.role.as_str());
        let verdict_id = VerdictId::new(verdict.as_str());
        let Some(role) = self.compiled.role(&role_id) else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} action={} verdict={}: role not found in compiled workflow",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item, job.role, action, verdict
            );
            return;
        };
        let Some(tool) = role.tools.iter().find(|tool| tool.name == action) else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} action={} verdict={}: action not found in compiled workflow",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item, job.role, action, verdict
            );
            return;
        };
        let Some(routed) = tool.outcomes.get(&verdict_id).cloned() else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} action={} verdict={}: verdict is not declared for action",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item, job.role, action, verdict
            );
            return;
        };

        match job.artifact.kind.as_str() {
            "issue" => {
                let Some((repository, issue)) = self.resolve_issue(&job).await else {
                    return;
                };
                let number = issue.number;
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

                let mut context = verdict_execution_context(
                    &job_context.artifact_kind,
                    &routed,
                    number,
                    result.body,
                );
                if !result.children.is_empty()
                    && !self
                        .bind_create_issues_children(VerdictChildrenBinding {
                            job: &job,
                            repository_id: &repository.id,
                            artifact_kind: &job_context.artifact_kind,
                            routed: &routed,
                            number,
                            children: result.children,
                            context: &mut context,
                        })
                        .await
                {
                    return;
                }

                self.execute_routed_verdict(RoutedVerdictApply {
                    job: &job,
                    repository_id: &repository.id,
                    source: ArtifactSource::Issue { number },
                    routed: &routed,
                    role_id: &role_id,
                    action,
                    verdict: &verdict,
                    artifact_label: "issue",
                    number,
                    context,
                })
                .await;
            }
            "pull_request" => {
                let Some((repository, pull_request)) = self.resolve_pull_request(&job).await else {
                    return;
                };
                let number = pull_request.number;
                let source_kind = ArtifactKindId::new(job_context.artifact_kind.as_str());
                // Replay after the routed transition changes the PR's identifying kind is
                // stale in the same way as the issue path above. Classifications that fail
                // for ordinary stale/terminal state are left for the executor's stale
                // mapping.
                if matches!(
                    Classifier::new(self.workflow.as_ref()).classify_pull_request(&pull_request),
                    Ok(classified) if classified.kind != source_kind
                ) {
                    return;
                }

                let context = verdict_execution_context(
                    &job_context.artifact_kind,
                    &routed,
                    number,
                    result.body,
                );

                self.execute_routed_verdict(RoutedVerdictApply {
                    job: &job,
                    repository_id: &repository.id,
                    source: ArtifactSource::PullRequest { number },
                    routed: &routed,
                    role_id: &role_id,
                    action,
                    verdict: &verdict,
                    artifact_label: "pull_request",
                    number,
                    context,
                })
                .await;
            }
            _ => {
                eprintln!(
                    "temper-daemon: forge applier ignored unsupported verdict job for job_id={} repo={} artifact.kind={} artifact.item={}",
                    job.job_id, job.repo, job.artifact.kind, job.artifact.item
                );
            }
        }
    }

    async fn bind_create_issues_children(&self, binding: VerdictChildrenBinding<'_>) -> bool {
        let Some(effect_index) = create_issues_effect_index(self.workflow.as_ref(), binding.routed)
        else {
            eprintln!(
                "temper-daemon: forge applier ignored verdict children without create_issues effect for job_id={} repo={} issue={} routed={} children={}",
                binding.job.job_id,
                binding.job.repo,
                binding.number,
                binding.routed,
                binding.children.len()
            );
            return true;
        };

        let mut mapped = Vec::with_capacity(binding.children.len());
        for child in binding.children {
            let Some(mapped_child) = self
                .map_job_child(binding.job, binding.repository_id, binding.number, child)
                .await
            else {
                return false;
            };
            mapped.push(mapped_child);
        }

        let content_key = workspace_content_key(
            &ArtifactKindId::new(binding.artifact_kind),
            binding.routed,
            binding.number,
        );
        binding
            .context
            .set_create_issues_at(binding.routed.clone(), effect_index, mapped);
        binding.context.set_create_issues_correlation_key_at(
            binding.routed.clone(),
            effect_index,
            content_key,
        );
        true
    }

    async fn map_job_child(
        &self,
        job: &InFlightJob,
        source_repo: &RepositoryId,
        number: ItemNumber,
        child: JobChild,
    ) -> Option<CreateIssuesChild> {
        let mut mapped = CreateIssuesChild {
            slug: child.slug,
            title: child.title,
            body: child.body,
            labels: child.labels,
            dependencies: child.depends_on,
            target_repo: None,
        };

        if let Some(target_repo) = child.target_repo {
            let repository = self
                .resolve_child_target_repository(job, source_repo, number, &target_repo)
                .await?;
            mapped = mapped.with_target_repo(repository.id);
        }

        Some(mapped)
    }

    async fn resolve_child_target_repository(
        &self,
        job: &InFlightJob,
        source_repo: &RepositoryId,
        number: ItemNumber,
        target_repo: &str,
    ) -> Option<Repository> {
        let Some(path) = parse_child_target_repo(target_repo) else {
            eprintln!(
                "temper-daemon: forge applier dropped verdict apply with malformed child target_repo for job_id={} repo={} issue={} target_repo={}",
                job.job_id, job.repo, number, target_repo
            );
            return None;
        };

        match self.forge.get_repository_by_path(&path).await {
            Ok(Some(repository)) => Some(repository),
            Ok(None) => {
                eprintln!(
                    "temper-daemon: forge applier dropped verdict apply with unknown child target_repo for job_id={} repo={} issue={} source_repo={} target_repo={}",
                    job.job_id, job.repo, number, source_repo, target_repo
                );
                None
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier dropped verdict apply after child target_repo lookup failed for job_id={} repo={} issue={} source_repo={} target_repo={}: {error}",
                    job.job_id, job.repo, number, source_repo, target_repo
                );
                None
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

        let repository = self.resolve_repository(job, "issue", number).await?;

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

    async fn resolve_pull_request(&self, job: &InFlightJob) -> Option<(Repository, PullRequest)> {
        if job.artifact.kind != "pull_request" {
            eprintln!(
                "temper-daemon: forge applier ignored non-pull-request job for job_id={} repo={} artifact.kind={} artifact.item={}",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item
            );
            return None;
        }

        let Some(number) = job.artifact.item.as_u64().map(ItemNumber::new) else {
            eprintln!(
                "temper-daemon: forge applier ignored job with non-numeric pull request item for job_id={} repo={} artifact.item={}",
                job.job_id, job.repo, job.artifact.item
            );
            return None;
        };

        let repository = self.resolve_repository(job, "pull_request", number).await?;

        let pull_request = match self
            .forge
            .get_pull_request_by_number(&repository.id, number)
            .await
        {
            Ok(Some(pull_request)) => pull_request,
            Ok(None) => {
                eprintln!(
                    "temper-daemon: forge applier source pull request not found for job_id={} repo={} pull_request={}",
                    job.job_id, job.repo, number
                );
                return None;
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier pull request lookup failed for job_id={} repo={} pull_request={}: {error}",
                    job.job_id, job.repo, number
                );
                return None;
            }
        };

        Some((repository, pull_request))
    }

    async fn resolve_repository(
        &self,
        job: &InFlightJob,
        artifact_label: &str,
        number: ItemNumber,
    ) -> Option<Repository> {
        let Some((owner, name)) = job.repo.split_once('/') else {
            eprintln!(
                "temper-daemon: forge applier ignored job with malformed repo path for job_id={} repo={}",
                job.job_id, job.repo
            );
            return None;
        };
        match self
            .forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await
        {
            Ok(Some(repository)) => Some(repository),
            Ok(None) => {
                eprintln!(
                    "temper-daemon: forge applier repository not found for job_id={} repo={} {}={}",
                    job.job_id, job.repo, artifact_label, number
                );
                None
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier repository lookup failed for job_id={} repo={} {}={}: {error}",
                    job.job_id, job.repo, artifact_label, number
                );
                None
            }
        }
    }

    async fn execute_routed_verdict(&self, apply: RoutedVerdictApply<'_>) {
        match Executor::with_context(self.workflow.as_ref(), self.forge.as_ref(), apply.context)
            .execute(
                apply.repository_id,
                apply.source,
                apply.routed,
                apply.role_id,
            )
            .await
        {
            Ok(_) => {}
            Err(error) if is_stale(&error) => {}
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not apply routed verdict transition for job_id={} repo={} {}={} role={} action={} verdict={} routed={}: {error}",
                    apply.job.job_id,
                    apply.job.repo,
                    apply.artifact_label,
                    apply.number,
                    apply.job.role,
                    apply.action,
                    apply.verdict,
                    apply.routed
                );
            }
        }
    }
}

struct RoutedVerdictApply<'a> {
    job: &'a InFlightJob,
    repository_id: &'a RepositoryId,
    source: ArtifactSource,
    routed: &'a TransitionId,
    role_id: &'a RoleId,
    action: &'a str,
    verdict: &'a str,
    artifact_label: &'static str,
    number: ItemNumber,
    context: ExecutionContext,
}

struct VerdictChildrenBinding<'a> {
    job: &'a InFlightJob,
    repository_id: &'a RepositoryId,
    artifact_kind: &'a str,
    routed: &'a TransitionId,
    number: ItemNumber,
    children: Vec<JobChild>,
    context: &'a mut ExecutionContext,
}

fn create_issues_effect_index(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
) -> Option<usize> {
    let declares_create_issues = workflow
        .transitions()
        .iter()
        .find(|candidate| &candidate.id == transition)?
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::CreateIssues { .. }));
    declares_create_issues.then_some(0)
}

fn parse_child_target_repo(target_repo: &str) -> Option<RepositoryPath> {
    let (owner, name) = target_repo.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some(RepositoryPath::new(owner, name))
}

fn verdict_execution_context(
    artifact_kind: &str,
    routed: &TransitionId,
    number: ItemNumber,
    body: Option<String>,
) -> ExecutionContext {
    let mut context = ExecutionContext::new();
    if let Some(body) = body {
        let content_key =
            workspace_content_key(&ArtifactKindId::new(artifact_kind), routed, number);
        context.set_set_body_at(routed.clone(), 0, body.clone());
        context.set_set_body_correlation_key_at(routed.clone(), 0, content_key.clone());
        context.set_attach_review_at(routed.clone(), 0, body);
        context.set_attach_review_correlation_key_at(routed.clone(), 0, content_key);
    }
    context
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

fn implementation_pr_create_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    let Some(kind) = workflow.artifact_kind(&ArtifactKindId::new("implementation_pr")) else {
        return Vec::new();
    };

    let mut labels = Vec::new();
    for label in kind
        .identifying_labels
        .iter()
        .chain(kind.initial_labels.iter())
    {
        let label = label.as_str().to_string();
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
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

/// Worker-protocol + webhook transport handle for one daemon process.
///
/// `Daemon` is a cloneable handle that submits `<io-event-completion>`s to the
/// daemon's engine loop. The logic — protocol handling, long-poll waiters,
/// apply-window bookkeeping, webhook verification — lives in `DaemonMachine`,
/// a pure state machine; all I/O (HTTP responses, timers, result application,
/// wake scans) is performed by `DaemonExecutor` on the engine runtime.
#[derive(Clone)]
pub struct Daemon {
    cq: CqSender<DaemonCompletion>,
    scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>>,
}

/// Type-erased webhook wake scanner installed by [`Daemon::with_webhook`].
trait WakeScanner: Send + Sync {
    fn scan(
        &self,
        hint: temper_runner::ChangeHint,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
}

/// `<io-event-completion>`s observed by the daemon machine.
enum DaemonCompletion {
    /// One inbound HTTP request (worker protocol or webhook).
    Http {
        request: HttpRequestData,
        responder: HttpResponder,
    },
    /// A long-poll waiter's max-wait deadline elapsed.
    PollDeadline { id: u64 },
    /// A result applier finished off-loop.
    ApplyFinished { job_id: String },
    /// Daemon API: enqueue one job (scans, backstops, tests).
    Enqueue {
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    },
    /// A webhook wake scan completed; release the held `202` response.
    WakeScanFinished { token: u64 },
    /// Adjust the post-apply re-enqueue grace window.
    SetApplyGrace { apply_grace: Duration },
    /// Enable webhook intake with the given verification config.
    ConfigureWebhook { config: WebhookConfig },
    #[cfg(test)]
    QueuedJobs {
        reply: temper_io_engine::OneshotSender<Vec<QueuedJob>>,
    },
}

/// `<io-event-request>`s the daemon machine may issue.
enum DaemonRequest {
    Respond {
        responder: HttpResponder,
        response: HttpResponseData,
    },
    StartPollTimer {
        id: u64,
        delay: Duration,
    },
    RunApply {
        job: InFlightJob,
        result: JobResult,
    },
    RunWakeScan {
        token: u64,
        hint: temper_runner::ChangeHint,
    },
    Log(String),
    #[cfg(test)]
    QueuedJobsReply(
        temper_io_engine::OneshotSender<Vec<QueuedJob>>,
        Vec<QueuedJob>,
    ),
}

struct PollWaiter {
    poll: Poll,
    responder: HttpResponder,
}

/// The daemon's functional core: deterministic worker-protocol, long-poll,
/// apply-window, and webhook-verification logic. No I/O, no clocks — time
/// arrives as data on completions; everything it wants done leaves as
/// [`DaemonRequest`] values.
struct DaemonMachine {
    core: DaemonCore,
    max_poll_wait_ms: u64,
    webhook: Option<WebhookConfig>,
    waiters: BTreeMap<u64, PollWaiter>,
    webhook_waiters: BTreeMap<u64, HttpResponder>,
    applying: BTreeSet<String>,
    recently_applied: BTreeMap<String, EngineTime>,
    apply_grace: Duration,
    /// The engine's once-per-delivery clock snapshot; updated as the first
    /// act of every transition, before any handler logic runs.
    now: EngineTime,
    next_id: u64,
}

impl DaemonMachine {
    fn new(apply_grace: Duration, max_poll_wait_ms: u64) -> Self {
        Self {
            core: DaemonCore::new(),
            max_poll_wait_ms,
            webhook: None,
            waiters: BTreeMap::new(),
            webhook_waiters: BTreeMap::new(),
            applying: BTreeSet::new(),
            recently_applied: BTreeMap::new(),
            apply_grace,
            now: EngineTime::ZERO,
            next_id: 0,
        }
    }

    fn next_token(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    fn handle_http(
        &mut self,
        request: HttpRequestData,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        match (request.method.as_str(), request.uri.as_str()) {
            ("POST", "/v1/message") => self.handle_protocol_message(&request.body, responder),
            ("POST", "/forgejo/webhook") if self.webhook.is_some() => {
                self.handle_webhook_delivery(&request, responder)
            }
            _ => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(404),
            }],
        }
    }

    fn handle_protocol_message(
        &mut self,
        body: &[u8],
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let Ok(msg) = serde_json::from_slice::<WorkerProtocolMessage>(body) else {
            return vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(400),
            }];
        };

        match msg {
            WorkerProtocolMessage::Poll(poll) => {
                let response = self
                    .core
                    .handle(WorkerProtocolMessage::Poll(poll.clone()))
                    .expect("poll messages produce a response");

                if is_poll_timeout(&response) {
                    let requested = poll.max_wait_ms.unwrap_or(self.max_poll_wait_ms);
                    let wait_ms = requested.min(self.max_poll_wait_ms);
                    let id = self.next_token();
                    self.waiters.insert(id, PollWaiter { poll, responder });
                    vec![DaemonRequest::StartPollTimer {
                        id,
                        delay: Duration::from_millis(wait_ms),
                    }]
                } else {
                    let mut requests = Vec::new();
                    if let WorkerProtocolMessage::Assign(assign) = &response {
                        requests.push(DaemonRequest::Log(assignment_log_line(
                            assign,
                            &poll.worker_id,
                        )));
                    }
                    requests.push(DaemonRequest::Respond {
                        responder,
                        response: protocol_response(Some(response)),
                    });
                    requests
                }
            }
            WorkerProtocolMessage::Result(result) => {
                let mut requests = Vec::new();
                // Capture full job context before the core completes and
                // forgets the job.
                let in_flight = self.core.in_flight_job(&result.job_id);
                let response = self
                    .core
                    .handle(WorkerProtocolMessage::Result(result.clone()));

                // Route only when the core accepted/completed the in-flight
                // job. Unknown, never-assigned, version-mismatched, and
                // double-sent results must not apply, retry, or drop beyond
                // the core response.
                if let (Some(job), Some(WorkerProtocolMessage::Release(_))) =
                    (in_flight, response.as_ref())
                {
                    let disposition = result_disposition(&result);
                    requests.push(DaemonRequest::Log(result_received_log_line(
                        &result,
                        result_disposition_log_value(disposition),
                    )));

                    match disposition {
                        ResultDisposition::Apply => {
                            self.applying.insert(job.job_id.clone());
                            requests.push(DaemonRequest::RunApply { job, result });
                        }
                        ResultDisposition::DropForRescan => {
                            // Let the next webhook wake or poll-backstop tick
                            // re-feed this through the guarded scan path
                            // instead of hot re-enqueuing.
                        }
                        ResultDisposition::Drop => {}
                    }
                }

                requests.push(DaemonRequest::Respond {
                    responder,
                    response: protocol_response(response),
                });
                requests
            }
            other => {
                let response = self.core.handle(other);
                vec![DaemonRequest::Respond {
                    responder,
                    response: protocol_response(response),
                }]
            }
        }
    }

    fn handle_webhook_delivery(
        &mut self,
        request: &HttpRequestData,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let config = self.webhook.as_ref().expect("webhook config checked");
        let headers: BTreeMap<String, String> = request
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();

        match parse_verified_webhook(&headers, &request.body, &config.secret) {
            Ok(hint) => {
                let token = self.next_token();
                self.webhook_waiters.insert(token, responder);
                vec![
                    DaemonRequest::Log(webhook_accepted_log_line(&hint)),
                    DaemonRequest::RunWakeScan { token, hint },
                ]
            }
            Err(WebhookError::InvalidSignature) => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(401),
            }],
            Err(WebhookError::BadPayload(_)) => vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(400),
            }],
        }
    }

    fn handle_enqueue(
        &mut self,
        job_id: String,
        role: String,
        repo: String,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) -> Vec<DaemonRequest> {
        let mut requests = Vec::new();
        let now = self.now;
        self.recently_applied.retain(|_, deadline| *deadline > now);
        if self.applying.contains(&job_id) {
            requests.push(DaemonRequest::Log(format!(
                "temper-daemon: skipped enqueue for job in apply window job_id={job_id}"
            )));
            return requests;
        }
        if self
            .recently_applied
            .get(&job_id)
            .is_some_and(|deadline| *deadline > now)
        {
            requests.push(DaemonRequest::Log(format!(
                "temper-daemon: skipped enqueue for recently applied job job_id={job_id}"
            )));
            return requests;
        }
        self.core
            .enqueue_job(job_id, role, repo, artifact, job_payload);
        requests.extend(self.fulfil_waiters());
        requests
    }

    fn fulfil_waiters(&mut self) -> Vec<DaemonRequest> {
        let mut requests = Vec::new();
        let ids = self.waiters.keys().copied().collect::<Vec<_>>();

        for id in ids {
            let Some(waiter) = self.waiters.get(&id) else {
                continue;
            };

            let response = self
                .core
                .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
                .expect("poll messages produce a response");

            if is_poll_timeout(&response) {
                continue;
            }

            let waiter = self
                .waiters
                .remove(&id)
                .expect("waiter exists after successful poll response");
            if let WorkerProtocolMessage::Assign(assign) = &response {
                requests.push(DaemonRequest::Log(assignment_log_line(
                    assign,
                    &waiter.poll.worker_id,
                )));
            }
            requests.push(DaemonRequest::Respond {
                responder: waiter.responder,
                response: protocol_response(Some(response)),
            });
        }
        requests
    }
}

impl Machine for DaemonMachine {
    type Completion = DaemonCompletion;
    type Request = DaemonRequest;

    fn on_completion(
        &mut self,
        now: EngineTime,
        completion: DaemonCompletion,
    ) -> Vec<DaemonRequest> {
        self.now = now;
        match completion {
            DaemonCompletion::Http { request, responder } => self.handle_http(request, responder),
            DaemonCompletion::PollDeadline { id } => {
                let Some(waiter) = self.waiters.remove(&id) else {
                    return Vec::new();
                };
                let response = self
                    .core
                    .handle(WorkerProtocolMessage::Poll(waiter.poll.clone()))
                    .expect("poll messages produce a response");
                vec![DaemonRequest::Respond {
                    responder: waiter.responder,
                    response: protocol_response(Some(response)),
                }]
            }
            DaemonCompletion::ApplyFinished { job_id } => {
                self.applying.remove(&job_id);
                self.recently_applied
                    .insert(job_id, self.now + self.apply_grace);
                Vec::new()
            }
            DaemonCompletion::Enqueue {
                job_id,
                role,
                repo,
                artifact,
                job_payload,
            } => self.handle_enqueue(job_id, role, repo, artifact, job_payload),
            DaemonCompletion::WakeScanFinished { token } => {
                match self.webhook_waiters.remove(&token) {
                    Some(responder) => vec![DaemonRequest::Respond {
                        responder,
                        response: HttpResponseData::status_only(202),
                    }],
                    None => Vec::new(),
                }
            }
            DaemonCompletion::SetApplyGrace { apply_grace } => {
                self.apply_grace = apply_grace;
                Vec::new()
            }
            DaemonCompletion::ConfigureWebhook { config } => {
                self.webhook = Some(config);
                Vec::new()
            }
            #[cfg(test)]
            DaemonCompletion::QueuedJobs { reply } => {
                vec![DaemonRequest::QueuedJobsReply(
                    reply,
                    self.core.queued_jobs(),
                )]
            }
        }
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

/// Renders a worker-protocol core response as an HTTP response: `200` with a
/// JSON body, or `204` when the core had nothing to say.
fn protocol_response(message: Option<WorkerProtocolMessage>) -> HttpResponseData {
    match message {
        Some(message) => HttpResponseData::json(
            200,
            &serde_json::to_value(&message).expect("protocol messages serialize"),
        ),
        None => HttpResponseData::status_only(204),
    }
}

/// The daemon's imperative shell: performs each machine request on the engine
/// runtime and feeds the resulting completions back into the queue.
struct DaemonExecutor {
    handle: asupersync::runtime::RuntimeHandle,
    cq: CqSender<DaemonCompletion>,
    applier: Arc<dyn ResultApplier>,
    scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>>,
}

impl EngineExecutor<DaemonMachine> for DaemonExecutor {
    fn execute(&self, request: DaemonRequest) {
        match request {
            DaemonRequest::Respond {
                responder,
                response,
            } => responder.respond(response),
            DaemonRequest::StartPollTimer { id, delay } => {
                arm_timer(&self.handle, &self.cq, delay, move || {
                    DaemonCompletion::PollDeadline { id }
                });
            }
            DaemonRequest::RunApply { job, result } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                let job_id = job.job_id.clone();
                self.handle.spawn(async move {
                    applier.apply(job, result).await;
                    let _ = cq.send(DaemonCompletion::ApplyFinished { job_id });
                });
            }
            DaemonRequest::RunWakeScan { token, hint } => {
                let scanner = self.scanner_slot.lock().expect("scanner slot").clone();
                let cq = self.cq.clone();
                match scanner {
                    Some(scanner) => {
                        self.handle.spawn(async move {
                            scanner.scan(hint).await;
                            let _ = cq.send(DaemonCompletion::WakeScanFinished { token });
                        });
                    }
                    None => {
                        let _ = cq.send(DaemonCompletion::WakeScanFinished { token });
                    }
                }
            }
            DaemonRequest::Log(line) => eprintln!("{line}"),
            #[cfg(test)]
            DaemonRequest::QueuedJobsReply(reply, jobs) => reply.send(jobs),
        }
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
            .cq
            .send(DaemonCompletion::SetApplyGrace { apply_grace });
        self
    }

    fn with_applier_and_apply_grace(
        applier: Arc<dyn ResultApplier>,
        apply_grace: Duration,
    ) -> Self {
        let handle = asupersync::runtime::Runtime::current_handle()
            .expect("Daemon requires a running engine runtime");
        let (cq_tx, cq_rx) = channel();
        let scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let executor = DaemonExecutor {
            handle: handle.clone(),
            cq: cq_tx.clone(),
            applier,
            scanner_slot: Arc::clone(&scanner_slot),
        };
        let machine = DaemonMachine::new(apply_grace, DEFAULT_MAX_POLL_WAIT_MS);
        handle.spawn(async move {
            let _ = drive(machine, &executor, cq_rx).await;
        });

        Self {
            cq: cq_tx,
            scanner_slot,
        }
    }

    /// Enables `POST /forgejo/webhook` intake on this daemon's HTTP surface:
    /// deliveries are verified and parsed by the daemon machine, then executed
    /// as wake scans against the given forge/workflow before the held `202`
    /// response is released.
    pub fn with_webhook<F: Forge + Send + Sync + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        config: Arc<WebhookConfig>,
    ) -> Self {
        struct ForgeWakeScanner<F: Forge + Send + Sync + 'static> {
            daemon: Daemon,
            forge: Arc<F>,
            workflow: Arc<ValidatedWorkflow>,
            compiled: Arc<CompiledWorkflow>,
            config: Arc<WebhookConfig>,
        }

        impl<F: Forge + Send + Sync + 'static> WakeScanner for ForgeWakeScanner<F> {
            fn scan(
                &self,
                hint: temper_runner::ChangeHint,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                let daemon = self.daemon.clone();
                let forge = Arc::clone(&self.forge);
                let workflow = Arc::clone(&self.workflow);
                let compiled = Arc::clone(&self.compiled);
                let config = Arc::clone(&self.config);
                Box::pin(async move {
                    run_wake_scan(
                        &daemon,
                        forge.as_ref(),
                        workflow.as_ref(),
                        compiled.as_ref(),
                        Utc::now(),
                        config.as_ref(),
                        &hint,
                    )
                    .await;
                })
            }
        }

        let scanner = Arc::new(ForgeWakeScanner {
            daemon: self.clone(),
            forge,
            workflow,
            compiled,
            config: Arc::clone(&config),
        });
        *self.scanner_slot.lock().expect("scanner slot") = Some(scanner);
        let _ = self.cq.send(DaemonCompletion::ConfigureWebhook {
            config: (*config).clone(),
        });
        self
    }

    pub async fn enqueue_job(
        &self,
        job_id: impl Into<String>,
        role: impl Into<String>,
        repo: impl Into<String>,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) {
        let _ = self.cq.send(DaemonCompletion::Enqueue {
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
        let (reply, rx) = temper_io_engine::oneshot();
        if self
            .cq
            .send(DaemonCompletion::QueuedJobs { reply })
            .is_err()
        {
            return Vec::new();
        }
        rx.recv()
            .await
            .unwrap_or_default()
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

/// Spawns a machine-driven fixed-delay poll backstop onto the engine runtime.
///
/// Replaces the previous `run_poll_backstop` sleep loop: a cadence machine
/// requests one tick, the shell executes the scan, and the next tick is
/// scheduled one cadence after the previous tick completed.
pub fn spawn_poll_backstop<F: Forge + Send + Sync + 'static>(
    daemon: Daemon,
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    config: PollBackstopConfig,
) {
    let handle = asupersync::runtime::Runtime::current_handle()
        .expect("poll backstop requires a running engine runtime");
    let cadence = config.cadence;
    temper_io_engine::spawn_cadence_loop(&handle, cadence, move || {
        let daemon = daemon.clone();
        let forge = Arc::clone(&forge);
        let workflow = Arc::clone(&workflow);
        let compiled = Arc::clone(&compiled);
        let config = config.clone();
        async move {
            run_poll_backstop_tick(
                &daemon,
                forge.as_ref(),
                workflow.as_ref(),
                compiled.as_ref(),
                Utc::now(),
                &config,
            )
            .await;
        }
    });
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

/// Binds and serves the daemon's HTTP surface (`POST /v1/message`, plus
/// `POST /forgejo/webhook` when [`Daemon::with_webhook`] was used) on the
/// engine runtime. Returns once bound; the connections are served by engine
/// tasks. Use the returned server handle for the bound address and graceful
/// drain.
pub async fn serve(
    daemon: &Daemon,
    bind: SocketAddr,
) -> std::io::Result<temper_io_engine::http::EngineHttpServer> {
    let handle = asupersync::runtime::Runtime::current_handle()
        .expect("serve requires a running engine runtime");
    let server = temper_io_engine::http::serve_http(
        &handle,
        bind,
        daemon.cq.clone(),
        |request, responder| DaemonCompletion::Http { request, responder },
    )
    .await?;
    eprintln!("temper-daemon: serving on {}", server.local_addr());
    Ok(server)
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
            children: Vec::new(),
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

    #[test]
    fn enrich_work_item_job_skips_merged_correlated_implementation_pr() {
        temper_io_engine::block_on(async move {
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
            let workflow: RawWorkflowSpec = serde_json::from_str(BASIC_DELIVERY_FIXTURE)
                .expect("basic-delivery workflow parses");
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
        })
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

    #[test]
    fn enqueue_work_item_stores_mapped_job() {
        temper_io_engine::block_on(async move {
            let daemon = Daemon::new();
            let item = work_item(ArtifactSource::Issue {
                number: ItemNumber::new(103),
            });
            let expected = job_from_work_item("ai/temper", &item);

            daemon.enqueue_work_item("ai/temper", &item).await;

            assert_eq!(daemon.queued_jobs().await, vec![expected]);
        })
    }

    #[test]
    fn enrich_work_item_job_skips_closed_issue() {
        temper_io_engine::block_on(async move {
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
            let workflow: RawWorkflowSpec = serde_json::from_str(BASIC_DELIVERY_FIXTURE)
                .expect("basic-delivery workflow parses");
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
        })
    }

    #[test]
    fn enrich_work_item_job_enriches_open_pull_request_artifact_snapshot() {
        temper_io_engine::block_on(async move {
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
            let workflow: RawWorkflowSpec = serde_json::from_str(BASIC_DELIVERY_FIXTURE)
                .expect("basic-delivery workflow parses");
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
        })
    }

    #[test]
    fn enrich_work_item_job_skips_closed_pull_request() {
        temper_io_engine::block_on(async move {
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
            let workflow: RawWorkflowSpec = serde_json::from_str(BASIC_DELIVERY_FIXTURE)
                .expect("basic-delivery workflow parses");
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
        })
    }
}
