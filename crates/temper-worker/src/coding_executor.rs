use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use temper_protocol_worker::{Assign, FailureClass, JobContext};

use crate::agent_runner::{AgentRunOutput, AgentRunRequest, AgentRunner, JobProgressReporter};
use crate::executor::{JobExecutionContext, JobExecutor, JobOutcome};
use crate::pr_freshness::PrFreshnessGuard;
use crate::workspace::{RoleGitIdentity, WorkspaceError, scoped_workspace_root};

mod context;
mod execution;
mod failure;
mod native_validation;
mod outcome;
mod preparation;

pub use native_validation::NativeValidatorCommand;
pub(super) use preparation::PreparedRepo;
use preparation::{PrepareRequest, prepare_repos};
mod session;
mod verdict;

use context::{build_workspace_context, effective_job_guidance};
use failure::failure;
use outcome::{WritableOutcomeRequest, writable_outcome};
use session::{
    agent_failure_outcome, attach_agent_session, persist_after_success, replay_accounted_attempt,
};
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
    native_validator_command: NativeValidatorCommand,
    /// Instance-scoped process containment override for worker-owned commands.
    containment_factory: Option<temper_process_containment::ContainmentFactory>,
    progress_reporter_factory: ProgressReporterFactory,
}

impl<R: AgentRunner + 'static> CodingExecutor<R> {
    pub fn new(config: CodingExecutorConfig, runner: Arc<R>) -> Self {
        Self {
            config,
            runner,
            pr_freshness_guard: None,
            native_validator_command: NativeValidatorCommand::cargo(),
            containment_factory: None,
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

async fn execute<R: AgentRunner>(
    config: CodingExecutorConfig,
    runner: Arc<R>,
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
    native_validator_command: NativeValidatorCommand,
    assign: Assign,
    execution: JobExecutionContext,
) -> JobOutcome {
    let attempt_id = execution.attempt.id.clone();
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
        mut source_metadata,
        guidance,
        structured_guidance,
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

    let guidance = effective_job_guidance(structured_guidance, guidance);
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
    let landing_base = manifest
        .repos
        .first()
        .map(|repo| repo.default_branch.as_str())
        .unwrap_or("main");
    if let Err(outcome) = native_validation::bind_resolved_mapping(
        &mut source_metadata,
        mode,
        &prepared[0],
        landing_base,
    )
    .await
    {
        return outcome;
    }
    if native_validation::configured(&source_metadata) {
        if !execution.fence.is_open() {
            return cancelled_attempt();
        }
        let credential_roles = config.role_identities.keys().cloned().collect::<Vec<_>>();
        let result = match native_validation::run(
            &native_validator_command,
            &source_metadata,
            &prepared[0],
            &prepared[0].repo,
            artifact.number,
            &credential_roles,
            &execution.cancellation,
        )
        .await
        {
            Ok(result) => result,
            Err(outcome) => return outcome,
        };
        let (result, details) = match native_validation::normalize(
            result,
            &source_metadata,
            mode,
            &prepared[0],
        )
        .await
        {
            Ok(normalized) => normalized,
            Err(outcome) => return outcome,
        };
        if let Err(error) =
            temper_verdict::validate_verdict_result(&result, &verdict_contracts, &source_metadata)
        {
            return failure(
                FailureClass::Protocol,
                format!("native validator result violates its workflow contract: {error}"),
            );
        }
        return verdict_only_outcome(
            &prepared[0].workspace,
            result,
            &allowed_verdicts,
            &coordination_key,
            details,
            &execution.fence,
        )
        .await;
    }

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
        guidance.as_ref(),
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
    if let Some(outcome) = replay_accounted_attempt(agent_session.as_ref(), &attempt_id) {
        return outcome;
    }

    // Run one agent turn with the cwd set to the workspace root (not a single
    // repo), so the agent can read and build every sibling. The runner owns the
    // agent mechanism; the executor owns the workspace lifecycle around it.
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
            attempt_id.clone(),
            &workspace_context,
            &workspace_root,
            execution.fence.clone(),
            execution.cancellation.clone(),
            progress,
        ))
        .await
    {
        Ok(output) => output,
        Err(error) => {
            return agent_failure_outcome(
                agent_session.as_ref(),
                &attempt_id,
                error,
                &execution.fence,
                &execution.cancellation,
            )
            .await;
        }
    };

    if !execution.fence.is_open() {
        return cancelled_attempt();
    }
    let (result, native_validation_details) =
        match native_validation::normalize(result, &source_metadata, mode, &prepared[0]).await {
            Ok(normalized) => normalized,
            Err(outcome) => return outcome,
        };
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
                native_validation_details,
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
