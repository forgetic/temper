use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use temper_protocol_agent::WorkspaceResult;
use temper_protocol_worker::{
    Assign, FailureClass, JobChild, JobContext, WorkspaceManifest, WorkspaceRepo,
};

use crate::agent_runner::{AgentRunError, AgentRunOutput, AgentRunner};
use crate::executor::{JobExecutor, JobOutcome};
use crate::pr_freshness::PrFreshnessGuard;
use crate::workspace::{
    RoleGitIdentity, Workspace, WorkspaceError, forgejo_remote_url, scoped_workspace_root,
};

mod context;
mod outcome;
mod session;

use context::build_workspace_context;
use outcome::{WritableOutcomeRequest, writable_outcome};
use session::{attach_agent_session, persist_after_success};

/// Configuration for the real coding-job executor.
///
/// The agent turn itself is produced by an [`AgentRunner`] passed alongside this
/// config (in-process `pi`-SDK by default; an external command or a test fake in
/// other contexts), so this struct only carries the workspace/git/identity
/// surface the executor owns.
#[derive(Clone, Debug)]
pub struct CodingExecutorConfig {
    /// Top-level root under which the executor creates per-role, per-job
    /// scoped workspaces: `<root>/<role>/<safe-coordination-key>/<repo-dir>`.
    pub workspace_root: PathBuf,
    /// Forge git base URL, e.g. `http://localhost:3000` (joined with the
    /// repo slug via `forgejo_remote_url`; `file://` URLs work for tests).
    pub git_base_url: String,
    /// Role id -> git identity (user, email, push token).
    pub role_identities: BTreeMap<String, RoleGitIdentity>,
}

/// Runs coding/triage/review jobs by preparing a scoped workspace, driving one
/// agent turn through its [`AgentRunner`], and mapping the result to a
/// [`JobOutcome`] (commit/push on the writable head path, verdict routing
/// otherwise).
#[derive(Clone)]
pub struct CodingExecutor<R: AgentRunner> {
    config: CodingExecutorConfig,
    runner: Arc<R>,
    /// Optional host-provided guard for PR-head freshness checks before pushes.
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
}

impl<R: AgentRunner> CodingExecutor<R> {
    pub fn new(config: CodingExecutorConfig, runner: Arc<R>) -> Self {
        Self {
            config,
            runner,
            pr_freshness_guard: None,
        }
    }

    /// Installs a host/daemon freshness guard used by `pull_request_writable`
    /// jobs before final PR-head pushes.
    pub fn with_pr_freshness_guard(mut self, guard: Arc<dyn PrFreshnessGuard>) -> Self {
        self.pr_freshness_guard = Some(guard);
        self
    }
}

impl<R: AgentRunner + 'static> JobExecutor for CodingExecutor<R> {
    fn execute(&self, assign: Assign) -> impl std::future::Future<Output = JobOutcome> + Send {
        let config = self.config.clone();
        let runner = Arc::clone(&self.runner);
        let pr_freshness_guard = self.pr_freshness_guard.clone();
        async move { execute(config, runner, pr_freshness_guard, assign).await }
    }
}

async fn execute<R: AgentRunner>(
    config: CodingExecutorConfig,
    runner: Arc<R>,
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
    assign: Assign,
) -> JobOutcome {
    let artifact_item = assign.artifact.item.clone();
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
        role,
        repo: _primary_repo,
        queue,
        artifact_kind,
        artifact,
        workspace: manifest,
        action,
        checkout_capability,
        allowed_verdicts,
        guidance,
        pull_request_freshness,
    } = context;

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
        assign.artifact.kind.as_str(),
        &checkout,
        &allowed_verdicts,
        guidance.as_deref(),
        pull_request_freshness.as_ref(),
    );
    let agent_session = match attach_agent_session(
        &mut workspace_context,
        &config.workspace_root,
        &role,
        &coordination_key,
        mode,
    )
    .await
    {
        Ok(agent_session) => agent_session,
        Err(outcome) => return outcome,
    };

    // Run one agent turn with the cwd set to the workspace root (not a single
    // repo), so the agent can read and build every sibling. The runner owns the
    // agent mechanism; the executor owns the workspace lifecycle around it.
    let AgentRunOutput {
        result,
        accepted_submit,
    } = match runner.run(&workspace_context, &workspace_root).await {
        Ok(output) => output,
        Err(AgentRunError { class, message }) => {
            return failure(class, message);
        }
    };

    let latest_self_pushed_sha = None;

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
            })
            .await
        }
        JobMode::ReadOnly | JobMode::PullRequestReadOnly => {
            verdict_only_outcome(
                &prepared[0].workspace,
                result,
                &allowed_verdicts,
                &coordination_key,
            )
            .await
        }
    };

    if let Some(failure) = persist_after_success(agent_session.as_ref(), &outcome).await {
        return failure;
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
    );

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
            return prepare_writable(workspace, repo_spec).await;
        }
        JobMode::Writable if repo_spec.is_writable() => {
            workspace
                .ensure_base_branch_exists_from_default(default_branch)
                .await
                .map_err(|error| workspace_failure("prepare workspace target branch", error))?;
            return prepare_writable(workspace, repo_spec).await;
        }
        // A read-only issue job may still be pointed at a feature branch from
        // workflow metadata (for example architect plan decomposition). Create
        // that branch from the repository default when it is missing, then check
        // it out read-only so the later implementation jobs inherit an existing
        // target branch.
        JobMode::ReadOnly => {
            workspace
                .ensure_base_branch_exists_from_default(default_branch)
                .await
                .map_err(|error| workspace_failure("prepare workspace target branch", error))?;
            workspace.prepare_read_only().await
        }
        // Read-only sibling in a writable job: the feed gives read-only repos
        // their repository default branch, so no target-branch materialization is
        // required.
        JobMode::Writable => workspace.prepare_read_only().await,
    };
    result.map_err(|error| workspace_failure("prepare workspace", error))
}

async fn prepare_writable(
    workspace: &Workspace,
    repo_spec: &WorkspaceRepo,
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
    workspace
        .prepare(&branch_hint)
        .await
        .map_err(|error| workspace_failure("prepare workspace", error))?;
    // Persist the role's git author identity + push credential into this
    // writable checkout's local `.git/config`; the worker owns the final branch
    // push after the agent leaves a product diff.
    workspace
        .configure_local_identity()
        .await
        .map_err(|error| workspace_failure("configure workspace git identity", error))
}

async fn verdict_only_outcome(
    workspace: &Workspace,
    result: WorkspaceResult,
    allowed_verdicts: &[String],
    correlation_key: &str,
) -> JobOutcome {
    let WorkspaceResult {
        verdict,
        summary,
        title,
        body,
        review_body,
        children,
        // `labels` is a head-path PR-label override; read-only verdict routing
        // does not consume it.
        labels: _labels,
    } = result;
    let Some(verdict) = verdict else {
        return failure(FailureClass::Permanent, "read-only job returned no verdict");
    };
    if !allowed_verdicts.contains(&verdict) {
        return failure(
            FailureClass::Permanent,
            format!(
                "read-only job returned undeclared verdict `{verdict}`; allowed verdicts: {}",
                allowed_verdicts_display(allowed_verdicts)
            ),
        );
    }

    if let Err(error) = workspace.discard_changes().await {
        return workspace_failure("discard verdict workspace changes", error);
    }

    let children = children
        .into_iter()
        .map(|child| JobChild {
            slug: child.slug,
            title: child.title,
            body: child.body,
            kind: child.kind,
            labels: child.labels,
            depends_on: child.depends_on,
            target_repo: child.target_repo,
        })
        .collect();

    JobOutcome::Verdict {
        verdict,
        title,
        body: body.or(review_body),
        summary: summary.or_else(|| Some(format!("implemented {correlation_key}"))),
        children,
    }
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
    failure(FailureClass::Transient, format!("{action}: {error}"))
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
