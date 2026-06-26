use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use temper_protocol_agent::WorkspaceResult;
use temper_protocol_worker::{
    Assign, FailureClass, JobChild, JobContext, WorkspaceManifest, WorkspaceRepo,
};

use crate::agent_runner::{AgentRunError, AgentRunner, ProgressSink};
use crate::executor::{JobExecutor, JobOutcome};
use crate::pr_freshness::PrFreshnessGuard;
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
    /// Where agent step-progress checkpoints are relayed (logging by default;
    /// the worker→daemon→forge relay plugs in here later).
    progress: Arc<dyn ProgressSink>,
    /// Optional host-provided guard for PR-head freshness checks before pushes.
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
}

impl<R: AgentRunner> CodingExecutor<R> {
    pub fn new(config: CodingExecutorConfig, runner: Arc<R>) -> Self {
        Self {
            config,
            runner,
            progress: Arc::new(crate::agent_runner::LoggingProgressSink),
            pr_freshness_guard: None,
        }
    }

    /// Installs a host/daemon freshness guard used by `pull_request_writable`
    /// jobs before final PR-head pushes.
    pub fn with_pr_freshness_guard(mut self, guard: Arc<dyn PrFreshnessGuard>) -> Self {
        self.pr_freshness_guard = Some(guard);
        self
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
        let pr_freshness_guard = self.pr_freshness_guard.clone();
        async move { execute(config, runner, progress, pr_freshness_guard, assign).await }
    }
}

async fn execute<R: AgentRunner>(
    config: CodingExecutorConfig,
    runner: Arc<R>,
    progress: Arc<dyn ProgressSink>,
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
    let workspace_root = config
        .workspace_root
        .join(&role)
        .join(workspace_scope_component(&coordination_key));

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

    // Run one agent turn with the cwd set to the workspace root (not a single
    // repo), so the agent can read and build every sibling. The runner owns the
    // agent mechanism and streams step-progress checkpoints to the sink; the
    // executor owns the workspace lifecycle around it.
    let result = match runner
        .run(&workspace_context, &workspace_root, Arc::clone(&progress))
        .await
    {
        Ok(result) => result,
        Err(AgentRunError { class, message }) => {
            return failure(class, message);
        }
    };

    match mode {
        JobMode::Writable | JobMode::PullRequestWritable => {
            writable_outcome(
                &prepared,
                result,
                &allowed_verdicts,
                &coordination_key,
                &artifact_item,
                mode == JobMode::PullRequestWritable,
                pull_request_freshness.as_ref(),
                pr_freshness_guard.as_deref(),
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
        // In-place PR fix: check out the PR's own head branch as a writable work
        // branch (the feed sets `branch_hint` to the PR head ref). `prepare`
        // resumes from the existing remote branch, so the agent's fix commits on
        // top of the PR head and the success path pushes it back, re-running CI.
        JobMode::PullRequestWritable => {
            return prepare_writable(workspace, repo_spec).await;
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
        .map_err(|error| workspace_failure("prepare workspace", error))?;
    // Persist the role's git author identity + push credential into this
    // writable checkout's local `.git/config`, so the spawned agent (which holds
    // no token) can commit + push its checkpoints against the prepared branch.
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

/// Percent-encodes a coordination key into one safe path component.
///
/// Keep common queue-generated keys readable (`pr-for-code-7`) while encoding
/// separators, dots, absolute-path markers, percent signs, and non-ASCII bytes so
/// an unusual key cannot escape the role root or create arbitrary nested paths.
fn workspace_scope_component(coordination_key: &str) -> String {
    if coordination_key.is_empty() {
        return "%EMPTY".to_string();
    }

    let mut component = String::with_capacity(coordination_key.len());
    for &byte in coordination_key.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {
                component.push(char::from(byte));
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                component.push('%');
                component.push(char::from(HEX[(byte >> 4) as usize]));
                component.push(char::from(HEX[(byte & 0x0F) as usize]));
            }
        }
    }
    component
}

#[cfg(test)]
mod tests {
    use super::workspace_scope_component;

    #[test]
    fn workspace_scope_component_keeps_common_keys_readable() {
        assert_eq!(
            workspace_scope_component("pr-for-code-448"),
            "pr-for-code-448"
        );
        assert_eq!(
            workspace_scope_component("coord_for_code_448"),
            "coord_for_code_448"
        );
    }

    #[test]
    fn workspace_scope_component_encodes_path_syntax() {
        assert_eq!(
            workspace_scope_component("../agent/pr-for-code-448"),
            "%2E%2E%2Fagent%2Fpr-for-code-448"
        );
        assert_eq!(workspace_scope_component("/absolute"), "%2Fabsolute");
        assert_eq!(workspace_scope_component("windows\\path"), "windows%5Cpath");
        assert_eq!(workspace_scope_component(""), "%EMPTY");
    }

    #[test]
    fn workspace_scope_component_is_collision_resistant_for_encoded_bytes() {
        assert_ne!(
            workspace_scope_component("a/b"),
            workspace_scope_component("a%2Fb")
        );
        assert_ne!(
            workspace_scope_component("a.b"),
            workspace_scope_component("a%2Eb")
        );
    }
}
