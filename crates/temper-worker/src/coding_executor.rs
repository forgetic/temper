use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use temper_agent_protocol::WorkspaceResult;
use temper_worker_protocol::{
    Assign, FailureClass, JobChild, JobContext, WorkspaceManifest, WorkspaceRepo,
};

use crate::agent_runner::{AgentRunError, AgentRunner, ProgressSink};
use crate::executor::{JobExecutor, JobOutcome};
use crate::workspace::{RoleGitIdentity, Workspace, WorkspaceError, forgejo_remote_url};

mod context;
mod outcome;

use context::build_workspace_context;
use outcome::writable_outcome;

/// Configuration for the real coding-job executor.
///
/// The agent turn itself is produced by an [`AgentRunner`] passed alongside this
/// config (in-process `pi`-SDK by default; an external command or a test fake in
/// other contexts), so this struct only carries the workspace/git/identity
/// surface the executor owns.
#[derive(Clone, Debug)]
pub struct CodingExecutorConfig {
    /// Root for persistent per-(repo, role) workspaces (3b-i layout).
    pub workspace_root: PathBuf,
    /// Forge git base URL, e.g. `http://localhost:3000` (joined with the
    /// repo slug via `forgejo_remote_url`; `file://` URLs work for tests).
    pub git_base_url: String,
    /// Role id -> git identity (user, email, push token).
    pub role_identities: BTreeMap<String, RoleGitIdentity>,
}

/// Runs coding/triage/review jobs by preparing a checkout, driving one agent
/// turn through its [`AgentRunner`], and mapping the result to a [`JobOutcome`]
/// (commit/push on the writable head path, verdict routing otherwise).
#[derive(Clone)]
pub struct CodingExecutor<R: AgentRunner> {
    config: CodingExecutorConfig,
    runner: Arc<R>,
    /// Where agent step-progress checkpoints are relayed (logging by default;
    /// the worker→daemon→forge relay plugs in here later).
    progress: Arc<dyn ProgressSink>,
}

impl<R: AgentRunner> CodingExecutor<R> {
    pub fn new(config: CodingExecutorConfig, runner: Arc<R>) -> Self {
        Self {
            config,
            runner,
            progress: Arc::new(crate::agent_runner::LoggingProgressSink),
        }
    }

    /// Overrides the step-progress sink (e.g. a daemon-relay sink, or a test
    /// recorder).
    pub fn with_progress_sink(mut self, progress: Arc<dyn ProgressSink>) -> Self {
        self.progress = progress;
        self
    }
}

impl<R: AgentRunner + 'static> JobExecutor for CodingExecutor<R> {
    fn execute(&self, assign: Assign) -> impl std::future::Future<Output = JobOutcome> + Send {
        let config = self.config.clone();
        let runner = Arc::clone(&self.runner);
        let progress = Arc::clone(&self.progress);
        async move { execute(config, runner, progress, assign).await }
    }
}

async fn execute<R: AgentRunner>(
    config: CodingExecutorConfig,
    runner: Arc<R>,
    progress: Arc<dyn ProgressSink>,
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
        action: _action,
        checkout_capability,
        allowed_verdicts,
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
    // Coordinated workspace root: the manifest repos are checked out as flat
    // siblings under here (one dir each) so their inter-repo path dependencies
    // resolve. Keyed by role so the per-repo checkouts (and their git object
    // caches) persist across this role's jobs; with single-job-per-worker
    // capacity, coordinated jobs never share the root concurrently.
    let workspace_root = config.workspace_root.join(&role);

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

    let workspace_context = build_workspace_context(
        &role,
        &queue,
        &artifact_kind,
        &manifest,
        &artifact,
        assign.artifact.kind.as_str(),
        &checkout,
        &allowed_verdicts,
    );

    // Run one agent turn with the cwd set to the workspace root (not a single
    // repo), so the agent can read and build every sibling. The runner owns the
    // agent mechanism and streams step-progress checkpoints to the sink; the
    // executor owns the workspace lifecycle around it.
    let result = match runner
        .run(&workspace_context, &workspace_root, progress.as_ref())
        .await
    {
        Ok(result) => result,
        Err(AgentRunError { class, message }) => {
            return failure(class, message);
        }
    };

    match mode {
        JobMode::Writable => {
            writable_outcome(
                &prepared,
                result,
                &allowed_verdicts,
                &coordination_key,
                &artifact_item,
            )
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
    }
}

/// One prepared sibling checkout plus its manifest entry.
pub(super) struct PreparedRepo {
    pub(super) repo: String,
    pub(super) writable: bool,
    pub(super) branch_hint: Option<String>,
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

        // Review acts on the single PR head; don't assemble siblings for it.
        if request.mode == JobMode::PullRequestReadOnly {
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
    let base_branch = if repo_spec.base_branch.trim().is_empty() {
        "main".to_string()
    } else {
        repo_spec.base_branch.clone()
    };
    let checkout_path = request.workspace_root.join(&repo_spec.dir);
    let workspace = Workspace::at(
        checkout_path,
        base_branch,
        request.identity.clone(),
        remote_url,
    );

    prepare_workspace(&workspace, request, repo_spec).await?;
    Ok(PreparedRepo {
        repo: repo_spec.repo.clone(),
        writable: repo_spec.is_writable(),
        branch_hint: repo_spec.branch_hint.clone(),
        workspace,
    })
}

async fn prepare_workspace(
    workspace: &Workspace,
    request: &PrepareRequest<'_>,
    repo_spec: &WorkspaceRepo,
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
        JobMode::Writable if repo_spec.is_writable() => {
            return prepare_writable(workspace, repo_spec).await;
        }
        // Read-only sibling in a writable job, or any repo in a read-only
        // (triage) job: materialize the base branch, never push.
        JobMode::Writable | JobMode::ReadOnly => workspace.prepare_read_only().await,
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
        .map_err(|error| workspace_failure("prepare workspace", error))
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
            labels: child.labels,
            depends_on: child.depends_on,
            target_repo: child.target_repo,
        })
        .collect();

    JobOutcome::Verdict {
        verdict,
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
}

impl JobMode {
    fn from_checkout(checkout: &str) -> Result<Self, JobOutcome> {
        match checkout {
            "writable" => Ok(Self::Writable),
            "read_only" => Ok(Self::ReadOnly),
            "pull_request_read_only" => Ok(Self::PullRequestReadOnly),
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
