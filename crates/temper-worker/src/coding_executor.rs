use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use temper_protocol_worker::{Assign, FailureClass, JobContext, WorkspaceManifest, WorkspaceRepo};

use crate::agent_runner::{
    AgentRunError, AgentRunOutput, AgentRunRequest, AgentRunner, JobProgressReporter,
};
use crate::executor::{JobExecutionContext, JobExecutor, JobOutcome};
use crate::pr_freshness::PrFreshnessGuard;
use crate::workspace::{
    PreparationOutcome, QuarantineManifest, RecoveryContext, RoleGitIdentity, Workspace,
    WorkspaceError, forgejo_remote_url, scoped_workspace_root,
};

mod context;
mod outcome;
mod session;
mod verdict;

use context::build_workspace_context;
use outcome::{WritableOutcomeRequest, writable_outcome};
use session::{attach_agent_session, persist_after_success};
use verdict::verdict_only_outcome;

type ProgressReporterFactory = Arc<dyn Fn(&str, &str) -> JobProgressReporter + Send + Sync>;

/// Configuration for the real coding-job executor.
///
/// The agent turn itself is produced by an [`AgentRunner`] passed alongside this
/// config (in-process `pi`-SDK by default; an external command or a test fake in
/// other contexts), so this struct only carries the workspace/git/identity
/// surface the executor owns.
#[derive(Clone, Debug)]
pub struct CodingExecutorConfig {
    /// Root for `<role>/<safe-coordination-key>/<repo-dir>` workspaces.
    pub workspace_root: PathBuf,
    /// Forge git base URL; `file://` URLs work for tests.
    pub git_base_url: String,
    pub role_identities: BTreeMap<String, RoleGitIdentity>,
}

/// Runs coding jobs by preparing a workspace, driving an [`AgentRunner`], and
/// mapping its product to a [`JobOutcome`].
#[derive(Clone)]
pub struct CodingExecutor<R: AgentRunner> {
    config: CodingExecutorConfig,
    runner: Arc<R>,
    /// Optional host-provided guard for PR-head freshness checks before pushes.
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
    progress_reporter_factory: ProgressReporterFactory,
}

impl<R: AgentRunner + 'static> CodingExecutor<R> {
    pub fn new(config: CodingExecutorConfig, runner: Arc<R>) -> Self {
        Self {
            config,
            runner,
            pr_freshness_guard: None,
            progress_reporter_factory: Arc::new(|_job_id, attempt_id| {
                JobProgressReporter::noop(attempt_id.to_string())
            }),
        }
    }

    /// Installs the PR-head freshness guard used before final pushes.
    pub fn with_pr_freshness_guard(mut self, guard: Arc<dyn PrFreshnessGuard>) -> Self {
        self.pr_freshness_guard = Some(guard);
        self
    }

    /// Installs worker-owned attempt binding for lifecycle delivery. Future
    /// watchdog state may reject stale IDs in the returned reporter.
    pub fn with_progress_reporter_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(&str, &str) -> JobProgressReporter + Send + Sync + 'static,
    {
        self.progress_reporter_factory = Arc::new(factory);
        self
    }

    /// Compatibility entry point for direct executor use. Production worker
    /// execution supplies the same controls from `WorkerShell` through the
    /// `JobExecutor` implementation below.
    pub fn execute(&self, assign: Assign) -> impl std::future::Future<Output = JobOutcome> + Send {
        let attempt_id = assign
            .attempt_id
            .clone()
            .unwrap_or_else(|| assign.job_id.clone());
        let mut execution = JobExecutionContext::unsupervised(&assign);
        execution.progress = (self.progress_reporter_factory)(&assign.job_id, &attempt_id);
        <Self as JobExecutor>::execute(self, assign, execution)
    }
}

impl<R: AgentRunner + 'static> JobExecutor for CodingExecutor<R> {
    fn execute(
        &self,
        assign: Assign,
        execution: JobExecutionContext,
    ) -> impl std::future::Future<Output = JobOutcome> + Send {
        let config = self.config.clone();
        let runner = Arc::clone(&self.runner);
        let pr_freshness_guard = self.pr_freshness_guard.clone();
        async move { execute(config, runner, pr_freshness_guard, assign, execution).await }
    }
}

async fn execute<R: AgentRunner>(
    config: CodingExecutorConfig,
    runner: Arc<R>,
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
    assign: Assign,
    execution: JobExecutionContext,
) -> JobOutcome {
    let artifact_item = assign.artifact.item.clone();
    let job_id = assign.job_id.clone();
    let assignment_trace_context = assign.trace_context.clone();
    let context = match serde_json::from_value::<JobContext>(assign.job_payload) {
        Ok(context) => context,
        Err(error) => {
            return failure(
                FailureClass::Protocol,
                format!("invalid enriched job payload: {error}"),
            );
        }
    };

    let JobContext {
        trace_context: payload_trace_context,
        role,
        repo: _primary_repo,
        queue,
        artifact_kind,
        artifact,
        artifact_context,
        workspace: manifest,
        action,
        checkout_capability,
        allowed_verdicts,
        verdict_contracts,
        source_metadata,
        guidance,
        pull_request_freshness,
    } = context;
    if assignment_trace_context.is_some()
        && payload_trace_context.is_some()
        && assignment_trace_context != payload_trace_context
    {
        return failure(
            FailureClass::Protocol,
            "assignment and job payload carry different W3C trace contexts".to_string(),
        );
    }
    let trace_context = assignment_trace_context.or(payload_trace_context);
    if let Some(context) = &trace_context {
        if let Err(error) = context.validate() {
            return failure(
                FailureClass::Protocol,
                format!("assignment carries invalid W3C trace context: {error}"),
            );
        }
    }

    let manifest = match require_enriched_field(manifest, "workspace") {
        Ok(manifest) => manifest,
        Err(outcome) => return outcome,
    };
    if manifest.repos.is_empty() {
        return failure(
            FailureClass::Protocol,
            "workspace manifest declared no repositories".to_string(),
        );
    }
    let artifact = match require_enriched_field(artifact, "artifact") {
        Ok(artifact) => artifact,
        Err(outcome) => return outcome,
    };
    let action = match require_enriched_field(action, "action") {
        Ok(action) => action,
        Err(outcome) => return outcome,
    };
    let checkout = checkout_capability.unwrap_or_else(|| "writable".to_string());
    let mode = match JobMode::from_checkout(&checkout) {
        Ok(mode) => mode,
        Err(outcome) => return outcome,
    };

    let identity = match config.role_identities.get(&role) {
        Some(identity) => identity.clone(),
        None => {
            return failure(
                FailureClass::Permanent,
                format!("worker has no git identity for role {role}"),
            );
        }
    };
    let coordination_key = manifest.coordination_key.clone();
    // Scoped workspace root: all manifest repos are checked out as flat siblings
    // under this job root (one dir each) so their inter-repo path dependencies
    // resolve. Include the coordination key in the path so standalone workers
    // with capacity > 1 never run distinct jobs for the same role/repo in the
    // same mutable checkout tree; retry/resume still comes from the remote work
    // branch during prepare, not from a role-shared local checkout.
    let workspace_root =
        match scoped_workspace_root(&config.workspace_root, &role, &coordination_key) {
            Ok(path) => path,
            Err(error) => {
                return failure(
                    FailureClass::Protocol,
                    format!("invalid scoped workspace path: {error}"),
                );
            }
        };

    let prepared = match prepare_repos(PrepareRequest {
        git_base_url: &config.git_base_url,
        identity: &identity,
        workspace_root: &workspace_root,
        manifest: &manifest,
        artifact_number: artifact.number,
        mode,
        coordination_key: &coordination_key,
        job_id: &job_id,
        cancellation: &execution.cancellation,
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };

    let mut workspace_context = build_workspace_context(
        &role,
        &queue,
        &action,
        &artifact_kind,
        &manifest,
        &artifact,
        artifact_context.as_ref(),
        assign.artifact.kind.as_str(),
        &checkout,
        &allowed_verdicts,
        &verdict_contracts,
        &source_metadata,
        guidance.as_deref(),
        pull_request_freshness.as_ref(),
        trace_context,
    );
    if !execution.fence.is_open() {
        return cancelled_attempt();
    }
    let agent_session = match attach_agent_session(
        &mut workspace_context,
        &config.workspace_root,
        &role,
        &coordination_key,
        mode,
        &execution.cancellation,
    )
    .await
    {
        Ok(agent_session) => agent_session,
        Err(outcome) => return outcome,
    };
    if !execution.fence.is_open() {
        return cancelled_attempt();
    }

    // Run one agent turn with the cwd set to the workspace root (not a single
    // repo), so the agent can read and build every sibling. The runner owns the
    // agent mechanism; the executor owns the workspace lifecycle around it.
    let attempt_id = execution.attempt.id.clone();
    let progress = execution.progress.clone();
    if progress.attempt_id() != attempt_id {
        return failure(
            FailureClass::Protocol,
            "progress reporter is bound to a different agent attempt".to_string(),
        );
    }
    let AgentRunOutput {
        result,
        accepted_submit,
    } = match runner
        .run_request(AgentRunRequest::new_controlled(
            &job_id,
            attempt_id,
            &workspace_context,
            &workspace_root,
            execution.fence.clone(),
            execution.cancellation.clone(),
            progress,
        ))
        .await
    {
        Ok(output) => output,
        Err(AgentRunError { class, message }) => {
            return failure(class, message);
        }
    };

    if !execution.fence.is_open() {
        return cancelled_attempt();
    }
    if let Err(error) =
        temper_verdict::validate_verdict_result(&result, &verdict_contracts, &source_metadata)
    {
        return failure(
            FailureClass::Protocol,
            format!("agent returned a result that violates its workflow verdict contract: {error}"),
        );
    }

    let latest_self_pushed_sha = None;

    if !execution.fence.is_open() {
        return cancelled_attempt();
    }
    let outcome = match mode {
        JobMode::Writable | JobMode::PullRequestWritable => {
            writable_outcome(WritableOutcomeRequest {
                prepared: &prepared,
                result,
                workspace_context: &workspace_context,
                workspace_root: &workspace_root,
                allowed_verdicts: &allowed_verdicts,
                coordination_key: &coordination_key,
                action: &action,
                artifact_item: &artifact_item,
                pull_request_fix: mode == JobMode::PullRequestWritable,
                pull_request_freshness: pull_request_freshness.as_ref(),
                freshness_guard: pr_freshness_guard.as_deref(),
                latest_self_pushed_sha,
                accepted_submit: accepted_submit.as_ref(),
                fence: &execution.fence,
                cancellation: &execution.cancellation,
            })
            .await
        }
        JobMode::ReadOnly | JobMode::PullRequestReadOnly => {
            verdict_only_outcome(
                &prepared[0].workspace,
                result,
                &allowed_verdicts,
                &coordination_key,
                &execution.fence,
            )
            .await
        }
    };

    if !execution.fence.is_open() {
        return cancelled_attempt();
    }
    if let Some(failure) =
        persist_after_success(agent_session.as_ref(), &outcome, &execution.cancellation).await
    {
        return failure;
    }
    if !execution.fence.is_open() {
        return cancelled_attempt();
    }
    outcome
}

/// One prepared sibling checkout plus its manifest entry.
pub(super) struct PreparedRepo {
    pub(super) repo: String,
    pub(super) writable: bool,
    pub(super) branch_hint: Option<String>,
    /// The commit checked out before the agent ran. PR-head repair jobs use this
    /// as their product-diff baseline: an existing implementation PR already
    /// differs from its base branch, so a clean no-op turn must not be counted
    /// as a successful CI fix merely because the PR branch contains prior work.
    pub(super) start_head_sha: String,
    pub(super) workspace: Workspace,
}

struct PrepareRequest<'a> {
    git_base_url: &'a str,
    identity: &'a RoleGitIdentity,
    workspace_root: &'a Path,
    manifest: &'a WorkspaceManifest,
    artifact_number: u64,
    mode: JobMode,
    coordination_key: &'a str,
    job_id: &'a str,
    cancellation: &'a crate::executor::JobCancellation,
}

async fn prepare_repos(request: PrepareRequest<'_>) -> Result<Vec<PreparedRepo>, JobOutcome> {
    let mut prepared = Vec::new();
    for repo_spec in &request.manifest.repos {
        prepared.push(prepare_repo(&request, repo_spec).await?);

        // PR-scoped jobs (review or in-place fix) act on the single PR head;
        // don't assemble siblings for them.
        if matches!(
            request.mode,
            JobMode::PullRequestReadOnly | JobMode::PullRequestWritable
        ) {
            break;
        }
    }
    Ok(prepared)
}

async fn prepare_repo(
    request: &PrepareRequest<'_>,
    repo_spec: &WorkspaceRepo,
) -> Result<PreparedRepo, JobOutcome> {
    let remote_url = forgejo_remote_url(request.git_base_url, &repo_spec.repo)
        .map_err(|error| workspace_failure("construct git remote URL", error))?;
    let base_branch = normalize_manifest_branch(&repo_spec.base_branch);
    let default_branch = normalize_manifest_branch(&repo_spec.default_branch);
    let checkout_path = request.workspace_root.join(&repo_spec.dir);
    let workspace = Workspace::at(
        checkout_path,
        base_branch,
        request.identity.clone(),
        remote_url,
    )
    .with_recovery_context(RecoveryContext {
        job_id: request.job_id.to_string(),
        correlation_key: request.coordination_key.to_string(),
        repository: repo_spec.repo.clone(),
    })
    .with_attempt_cancellation(request.cancellation.clone());

    prepare_workspace(&workspace, request, repo_spec, &default_branch).await?;
    let start_head_sha = workspace
        .head_sha()
        .await
        .map_err(|error| workspace_failure("inspect prepared workspace head", error))?;
    Ok(PreparedRepo {
        repo: repo_spec.repo.clone(),
        writable: repo_spec.is_writable(),
        branch_hint: repo_spec.branch_hint.clone(),
        start_head_sha,
        workspace,
    })
}

fn normalize_manifest_branch(branch: &str) -> String {
    if branch.trim().is_empty() {
        "main".to_string()
    } else {
        branch.to_string()
    }
}

async fn prepare_workspace(
    workspace: &Workspace,
    request: &PrepareRequest<'_>,
    repo_spec: &WorkspaceRepo,
    default_branch: &str,
) -> Result<(), JobOutcome> {
    let result = match request.mode {
        JobMode::PullRequestReadOnly => {
            let branch_hint = repo_spec
                .branch_hint
                .clone()
                .unwrap_or_else(|| format!("agent/{}", request.coordination_key));
            workspace
                .prepare_pull_request_head(request.artifact_number, &branch_hint)
                .await
        }
        // In-place PR fix: check out the PR's own head branch as a writable work
        // branch (the feed sets `branch_hint` to the PR head ref). `prepare`
        // resumes from the existing remote branch, so the agent's fix commits on
        // top of the PR head and the success path pushes it back, re-running CI.
        JobMode::PullRequestWritable => {
            return prepare_writable(workspace, repo_spec, None).await;
        }
        JobMode::Writable if repo_spec.is_writable() => {
            return prepare_writable(workspace, repo_spec, Some(default_branch)).await;
        }
        // A read-only issue job may still be pointed at a feature branch from
        // workflow metadata (for example architect plan decomposition). Create
        // that branch from the repository default when it is missing, then check
        // it out read-only so the later implementation jobs inherit an existing
        // target branch.
        JobMode::ReadOnly => {
            workspace
                .prepare_read_only_from_default(default_branch)
                .await
        }
        // Read-only sibling in a writable job: the feed gives read-only repos
        // their repository default branch, so no target-branch materialization is
        // required.
        JobMode::Writable => workspace.prepare_read_only().await,
    };
    match result {
        Ok(PreparationOutcome::Quarantined(manifest)) => Err(quarantine_failure(&manifest)),
        Ok(PreparationOutcome::CleanReuse { .. })
        | Ok(PreparationOutcome::RecoveredLocalWork { .. }) => Ok(()),
        Err(error) => Err(workspace_failure("prepare workspace", error)),
    }
}

async fn prepare_writable(
    workspace: &Workspace,
    repo_spec: &WorkspaceRepo,
    default_branch: Option<&str>,
) -> Result<(), JobOutcome> {
    let Some(branch_hint) = repo_spec.branch_hint.clone() else {
        return Err(failure(
            FailureClass::Protocol,
            format!(
                "writable workspace repo {} is missing a branch hint",
                repo_spec.repo
            ),
        ));
    };
    let outcome = match default_branch {
        Some(default_branch) => {
            workspace
                .prepare_from_default(default_branch, &branch_hint)
                .await
        }
        None => workspace.prepare(&branch_hint).await,
    };
    match outcome {
        Ok(PreparationOutcome::Quarantined(manifest)) => {
            return Err(quarantine_failure(&manifest));
        }
        Ok(PreparationOutcome::CleanReuse { .. })
        | Ok(PreparationOutcome::RecoveredLocalWork { .. }) => {}
        Err(error) => return Err(workspace_failure("prepare workspace", error)),
    }
    // Persist the role's git author identity + push credential into this
    // writable checkout's local `.git/config`; the worker owns the final branch
    // push after the agent leaves a product diff.
    workspace
        .configure_local_identity()
        .await
        .map_err(|error| workspace_failure("configure workspace git identity", error))
}

fn quarantine_failure(manifest: &QuarantineManifest) -> JobOutcome {
    const COMMANDS_BEGIN: &str = "--- BEGIN RUNNABLE RECOVERY COMMANDS ---";
    const COMMANDS_END: &str = "--- END RUNNABLE RECOVERY COMMANDS ---";

    let mut message = format!(
        "workspace {} quarantined during {} at {}\nunderlying failure: {}\nrecovery notes:",
        manifest.repository, manifest.failure_phase, manifest.quarantine_path, manifest.failure
    );
    if manifest.recovery_notes.is_empty() {
        message.push_str(" (none recorded in manifest)");
    } else {
        for note in &manifest.recovery_notes {
            message.push_str("\n- ");
            message.push_str(note);
        }
    }

    message.push('\n');
    message.push_str(COMMANDS_BEGIN);
    message.push('\n');
    if !manifest.recovery_commands.is_empty() {
        message.push_str(&manifest.recovery_commands.join("\n"));
        message.push('\n');
    }
    message.push_str(COMMANDS_END);

    failure(FailureClass::Permanent, message)
}

fn require_enriched_field<T>(field: Option<T>, name: &str) -> Result<T, JobOutcome> {
    field.ok_or_else(|| {
        failure(
            FailureClass::Protocol,
            format!("enriched job payload is missing `{name}`"),
        )
    })
}

fn workspace_failure(action: &str, error: WorkspaceError) -> JobOutcome {
    let class = if matches!(
        &error,
        WorkspaceError::Recovery(_) | WorkspaceError::Quarantined { .. }
    ) {
        FailureClass::Permanent
    } else {
        FailureClass::Transient
    };
    failure(class, format!("{action}: {error}"))
}

fn cancelled_attempt() -> JobOutcome {
    failure(
        FailureClass::Transient,
        "job attempt was cancelled by the worker watchdog",
    )
}

fn failure(class: FailureClass, message: impl Into<String>) -> JobOutcome {
    JobOutcome::Failure {
        class,
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JobMode {
    Writable,
    ReadOnly,
    PullRequestReadOnly,
    /// Writable checkout of an existing pull request's head branch: the agent
    /// fixes the PR in place (e.g. a failed CI gate) and the fix is pushed back
    /// to that same head branch, re-running CI. Distinct from `Writable`, which
    /// opens a new PR from a synthetic branch.
    PullRequestWritable,
}

impl JobMode {
    fn from_checkout(checkout: &str) -> Result<Self, JobOutcome> {
        match checkout {
            "writable" => Ok(Self::Writable),
            "read_only" => Ok(Self::ReadOnly),
            "pull_request_read_only" => Ok(Self::PullRequestReadOnly),
            "pull_request_writable" => Ok(Self::PullRequestWritable),
            other => Err(failure(
                FailureClass::Protocol,
                format!("unsupported checkout capability `{other}`"),
            )),
        }
    }
}

fn allowed_verdicts_display(allowed_verdicts: &[String]) -> String {
    if allowed_verdicts.is_empty() {
        "[]".to_string()
    } else {
        allowed_verdicts.join(", ")
    }
}
