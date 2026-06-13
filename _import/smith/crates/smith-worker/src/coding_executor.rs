use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use smith_agent_protocol::{
    WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceResult, WorkspaceWorkItem,
};
use temper_worker_protocol::{
    Assign, Branch, FailureClass, JobArtifactSnapshot, JobChild, JobRepository,
};

use crate::agent_runner::{AgentRunError, AgentRunner, ProgressSink};
use crate::executor::{JobExecutor, JobOutcome};
use crate::workspace::{
    RoleGitIdentity, Workspace, WorkspaceConfig, WorkspaceError, forgejo_remote_url,
};

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
    let context = match serde_json::from_value::<WireJobContext>(assign.job_payload) {
        Ok(context) => context,
        Err(error) => {
            return failure(
                FailureClass::Protocol,
                format!("invalid enriched job payload: {error}"),
            );
        }
    };

    let WireJobContext {
        role,
        repo,
        queue,
        artifact_kind,
        repository,
        base_branch,
        branch_hint,
        correlation_key,
        artifact,
        action: _action,
        checkout_capability,
        allowed_verdicts,
    } = context;

    let repository = match require_enriched_field(repository, "repository") {
        Ok(repository) => repository,
        Err(outcome) => return outcome,
    };
    let base_branch = match require_enriched_field(base_branch, "base_branch") {
        Ok(base_branch) => base_branch,
        Err(outcome) => return outcome,
    };
    let branch_hint = match require_enriched_field(branch_hint, "branch_hint") {
        Ok(branch_hint) => branch_hint,
        Err(outcome) => return outcome,
    };
    let correlation_key = match require_enriched_field(correlation_key, "correlation_key") {
        Ok(correlation_key) => correlation_key,
        Err(outcome) => return outcome,
    };
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
    let token = identity.token.clone();

    let remote_url = match forgejo_remote_url(&config.git_base_url, &repo) {
        Ok(remote_url) => remote_url,
        Err(error) => return workspace_failure("construct git remote URL", error, ""),
    };
    let workspace_config = WorkspaceConfig {
        root: config.workspace_root,
        base_branch: base_branch.clone(),
    };
    let workspace = match Workspace::new(&workspace_config, &repo, &role, identity, remote_url) {
        Ok(workspace) => workspace,
        Err(error) => return workspace_failure("configure workspace", error, &token),
    };
    let prepare_result = match mode {
        JobMode::Writable | JobMode::ReadOnly => workspace.prepare(&branch_hint).await,
        JobMode::PullRequestReadOnly => {
            workspace
                .prepare_pull_request_head(artifact.number, &branch_hint)
                .await
        }
    };
    if let Err(error) = prepare_result {
        return workspace_failure("prepare workspace", error, &token);
    }

    let workspace_context = build_workspace_context(
        &repo,
        &role,
        &queue,
        &artifact_kind,
        &repository,
        &base_branch,
        &branch_hint,
        &correlation_key,
        &artifact,
        assign.artifact.kind.as_str(),
        &checkout,
        &allowed_verdicts,
    );

    // Run one agent turn out-of-process (the prepared checkout is the cwd). The
    // runner owns the agent mechanism (a spawned `anvil-agent`, an external
    // coder, or a test fake) and streams step-progress checkpoints to the sink;
    // the executor owns the workspace lifecycle around it.
    let result = match runner
        .run(&workspace_context, workspace.path(), progress.as_ref())
        .await
    {
        Ok(result) => result,
        Err(AgentRunError { class, message }) => {
            return failure(class, redact_secret(message, &token));
        }
    };

    match mode {
        JobMode::Writable => {
            if let Some(verdict) = result.verdict {
                if allowed_verdicts.contains(&verdict) {
                    if let Err(error) = workspace.discard_changes().await {
                        return workspace_failure(
                            "discard verdict workspace changes",
                            error,
                            &token,
                        );
                    }

                    let summary = result
                        .summary
                        .or_else(|| Some(format!("implemented {correlation_key}")));
                    return JobOutcome::Verdict {
                        verdict,
                        body: result.body.or(result.review_body),
                        summary,
                        children: Vec::new(),
                    };
                }

                return failure(
                    FailureClass::Permanent,
                    format!("verdict routing not supported by smith-worker yet: {verdict}"),
                );
            }

            // The agent may have checkpoint-committed (and pushed) some or
            // all of its work mid-run; only a run that left neither tree
            // changes nor commits beyond base produced no product.
            match workspace.has_changes().await {
                Ok(true) => {
                    if let Err(error) = workspace
                        .commit_all(&commit_message(&correlation_key, &artifact_item))
                        .await
                    {
                        return workspace_failure("commit workspace changes", error, &token);
                    }
                }
                Ok(false) => match workspace.commits_ahead_of_base().await {
                    Ok(true) => {}
                    Ok(false) => {
                        return failure(FailureClass::Permanent, "agent produced no diff");
                    }
                    Err(error) => {
                        return workspace_failure("inspect workspace commits", error, &token);
                    }
                },
                Err(error) => return workspace_failure("inspect workspace changes", error, &token),
            }
            let head_sha = match workspace.push_branch(&branch_hint).await {
                Ok(head_sha) => head_sha,
                Err(error) => return workspace_failure("push workspace branch", error, &token),
            };

            JobOutcome::Success {
                branch: Branch {
                    name: branch_hint,
                    head_sha,
                },
                summary: result
                    .summary
                    .or_else(|| Some(format!("implemented {correlation_key}"))),
            }
        }
        JobMode::ReadOnly | JobMode::PullRequestReadOnly => {
            verdict_only_outcome(
                &workspace,
                result,
                &allowed_verdicts,
                &correlation_key,
                &token,
            )
            .await
        }
    }
}

async fn verdict_only_outcome(
    workspace: &Workspace,
    result: WorkspaceResult,
    allowed_verdicts: &[String],
    correlation_key: &str,
    token: &str,
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
        return workspace_failure("discard verdict workspace changes", error, token);
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

fn workspace_failure(action: &str, error: WorkspaceError, token: &str) -> JobOutcome {
    failure(
        FailureClass::Transient,
        redact_secret(format!("{action}: {error}"), token),
    )
}

/// Builds the implementation commit message.
///
/// Numeric issue artifacts gain a `Closes #<n>` trailer so the forge's native
/// close-on-merge closes the source issue when the implementation PR lands —
/// the daemon applies no issue transition on success, so this trailer is what
/// retires the source issue (and its queue entry) at merge time.
fn commit_message(correlation_key: &str, artifact_item: &serde_json::Value) -> String {
    match artifact_item.as_u64() {
        Some(number) => format!("Implement {correlation_key}\n\nCloses #{number}"),
        None => format!("Implement {correlation_key}"),
    }
}

fn failure(class: FailureClass, message: impl Into<String>) -> JobOutcome {
    JobOutcome::Failure {
        class,
        message: message.into(),
    }
}

/// Assembles the typed [`WorkspaceContext`] the agent turn receives.
///
/// The [`OutOfProcessRunner`](crate::out_of_process_runner::OutOfProcessRunner)
/// serializes this to the JSON document the agent reads from
/// `$TEMPER_CODING_WORKSPACE_CONTEXT`; the struct (and thus the wire shape) is
/// owned by `smith-agent-protocol`. `work_item.context` stays a pretty-printed
/// JSON *string* of the artifact, surfaced to the model verbatim.
#[allow(clippy::too_many_arguments)]
fn build_workspace_context(
    repo: &str,
    role: &str,
    queue: &str,
    artifact_kind: &str,
    repository: &JobRepository,
    base_branch: &str,
    branch_hint: &str,
    correlation_key: &str,
    artifact: &JobArtifactSnapshot,
    artifact_wire_kind: &str,
    checkout: &str,
    allowed_verdicts: &[String],
) -> WorkspaceContext {
    let (artifact_type, target_kind) = match artifact_wire_kind {
        "pull_request" => ("pull_request", "PullRequest"),
        _ => ("issue", "Issue"),
    };
    let work_item_context = serde_json::json!({
        "repository": repo,
        "role": role,
        "queue": queue,
        "kind": artifact_kind,
        "artifact": {
            "type": artifact_type,
            "number": artifact.number,
            "title": artifact.title.as_str(),
            "body": artifact.body.as_str(),
            "labels": &artifact.labels,
            "state": artifact.state.as_str(),
        }
    });
    // `to_string_pretty` on an in-memory `Value` is infallible; fall back to the
    // compact form rather than failing the job on the impossible error path.
    let work_item_context = serde_json::to_string_pretty(&work_item_context)
        .unwrap_or_else(|_| work_item_context.to_string());

    WorkspaceContext {
        repository: WorkspaceRepository {
            id: repo.to_string(),
            owner: repository.owner.clone(),
            name: repository.name.clone(),
            default_branch: repository.default_branch.clone(),
        },
        work_item: WorkspaceWorkItem {
            role: role.to_string(),
            queue: queue.to_string(),
            kind: artifact_kind.to_string(),
            target: format!(
                "{target_kind} {{ number: ItemNumber({}) }}",
                artifact.number
            ),
            context: work_item_context,
        },
        base_branch: base_branch.to_string(),
        branch_hint: branch_hint.to_string(),
        correlation_key: correlation_key.to_string(),
        checkout: Some(checkout.to_string()),
        allowed_verdicts: allowed_verdicts.to_vec(),
        guidance: WorkspaceGuidance::default(),
    }
}

fn redact_secret(text: String, secret: &str) -> String {
    if secret.is_empty() {
        text
    } else {
        text.replace(secret, "<redacted>")
    }
}

#[derive(Debug, Deserialize)]
struct WireJobContext {
    role: String,
    repo: String,
    queue: String,
    artifact_kind: String,
    #[serde(default)]
    repository: Option<JobRepository>,
    #[serde(default)]
    base_branch: Option<String>,
    #[serde(default)]
    branch_hint: Option<String>,
    #[serde(default)]
    correlation_key: Option<String>,
    #[serde(default)]
    artifact: Option<JobArtifactSnapshot>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    checkout_capability: Option<String>,
    #[serde(default)]
    allowed_verdicts: Vec<String>,
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
