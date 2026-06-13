//! Production coding-workspace provider.
//!
//! The provider is intentionally narrow: it prepares one git worktree, runs one
//! operator-configured edit command with the work-item context in a temp file,
//! commits the resulting branch, checks that the diff is not Temper
//! bookkeeping-only, and optionally pushes the branch for Forgejo PR creation.
//! The LLM never receives shell or filesystem tools; it can only choose a
//! workflow action and the adapter invokes this provider when the workflow
//! declared and the runner bound `coding_workspace`.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use temper_runner::{
    CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
    WorkspaceCheckout,
};
use temper_workflow::{CreateIssuesChild, VerdictId};

use crate::pr_diff_guard::{safety_for_files, DiffSafety};

pub const WORKSPACE_ROOT_ENV: &str = "TEMPER_CODING_WORKSPACE_ROOT";
pub const WORKSPACE_COMMAND_ENV: &str = "TEMPER_CODING_WORKSPACE_COMMAND";
pub const WORKSPACE_REMOTE_ENV: &str = "TEMPER_CODING_WORKSPACE_REMOTE";
pub const WORKSPACE_PUSH_ENV: &str = "TEMPER_CODING_WORKSPACE_PUSH";
pub const WORKSPACE_PR_LABELS_ENV: &str = "TEMPER_CODING_WORKSPACE_PR_LABELS";

/// Env var naming the provider-created result file the edit command may write to
/// report a verdict and/or content back to the workspace. The command always
/// receives this path alongside [`WORKSPACE_CONTEXT_ENV`]; writing the file is
/// optional. When the file is absent, empty, or carries no `verdict`, the
/// provider keeps today's head flow.
pub const WORKSPACE_RESULT_ENV: &str = "TEMPER_CODING_WORKSPACE_RESULT";

/// The shared result protocol an edit command may write to
/// `TEMPER_CODING_WORKSPACE_RESULT`. Every field is optional; an empty object is
/// equivalent to writing no file at all. Defined once with serde so later parts
/// of the operator path (e.g. T4 (#46) `children` wiring) can extend it cleanly.
///
/// See `docs/how-to/configure-coding-workspace.md` for the operator-facing
/// contract.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceResult {
    /// Typed verdict selecting the outcome transition declared by the action.
    /// `None` (omitted/null) keeps the default head path: commit, push, open PR.
    /// When present, the provider skips the commit/push and tolerates an empty
    /// working tree, returning a verdict-only output.
    pub verdict: Option<String>,
    /// One-line summary for the PR body and logs on the head path. Falls back to
    /// the changed-files list when absent.
    pub summary: Option<String>,
    /// Agent-rewritten issue body consumed by a routed `set_body` effect.
    pub body: Option<String>,
    /// Agent-authored review prose consumed by a routed `attach_review` effect.
    pub review_body: Option<String>,
    /// Overrides the default PR labels on the head path. `None` keeps the
    /// provider-configured labels.
    pub labels: Option<Vec<String>>,
    /// Dependent child artifacts consumed by a routed `create_issues` effect
    /// (e.g. an architect `needs_breakdown` verdict fanning an intake out into
    /// authored code issues). Each child carries its own title/body/labels and
    /// may name sibling children it `depends_on`. Threaded into the output's
    /// `children` on the verdict path.
    #[serde(default)]
    pub children: Vec<WorkspaceResultChild>,
}

/// One workspace-authored child artifact in the result protocol.
///
/// This is the operator-facing wire shape; it maps to the runtime
/// [`CreateIssuesChild`] the routed `create_issues` effect consumes. The
/// operator-facing dependency field is `depends_on` (sibling slugs); it maps to
/// `CreateIssuesChild::dependencies`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceResultChild {
    /// Stable identifier for this child within the effect. Seeds the child's
    /// per-child correlation key and lets siblings reference it in `depends_on`.
    pub slug: String,
    /// Authored child title.
    pub title: String,
    /// Authored child body.
    pub body: String,
    /// Labels to create the child with (e.g. `code`, `ready`).
    pub labels: Vec<String>,
    /// Slugs of sibling children in the same effect that must land before this
    /// one.
    pub depends_on: Vec<String>,
}

impl WorkspaceResultChild {
    /// Maps the wire child onto the runtime [`CreateIssuesChild`] the
    /// `create_issues` effect consumes.
    fn into_create_issues_child(self) -> CreateIssuesChild {
        CreateIssuesChild::new(self.slug, self.title, self.body)
            .with_labels(self.labels)
            .with_dependencies(self.depends_on)
    }
}

impl WorkspaceResult {
    /// Parses a result file's contents. An empty/whitespace-only file is treated
    /// as an absent result (the default head path).
    fn parse(contents: &str) -> Result<Option<Self>, String> {
        if contents.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(contents)
            .map(Some)
            .map_err(|error| format!("failed to parse coding workspace result file: {error}"))
    }

    /// The verdict as a typed [`VerdictId`], if a non-empty verdict was set.
    fn verdict_id(&self) -> Option<VerdictId> {
        self.verdict
            .as_deref()
            .map(str::trim)
            .filter(|verdict| !verdict.is_empty())
            .map(VerdictId::new)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalGitCodingWorkspace {
    root: PathBuf,
    command: Vec<String>,
    remote: String,
    push: bool,
    pr_labels: Vec<String>,
}

impl LocalGitCodingWorkspace {
    pub fn new(root: impl Into<PathBuf>, command: Vec<String>) -> Self {
        Self {
            root: root.into(),
            command,
            remote: "origin".to_string(),
            push: true,
            pr_labels: default_pr_labels(),
        }
    }

    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = remote.into();
        self
    }

    pub fn with_push(mut self, push: bool) -> Self {
        self.push = push;
        self
    }

    pub fn with_pr_labels(mut self, labels: Vec<String>) -> Self {
        self.pr_labels = labels;
        self
    }

    pub fn from_env<E>(env: E) -> Result<Option<Self>, String>
    where
        E: Fn(&str) -> Option<String>,
    {
        let Some(root) = non_empty(env(WORKSPACE_ROOT_ENV)) else {
            return Ok(None);
        };
        let command = non_empty(env(WORKSPACE_COMMAND_ENV)).ok_or_else(|| {
            format!("{WORKSPACE_ROOT_ENV} is set but {WORKSPACE_COMMAND_ENV} is missing")
        })?;
        let remote = non_empty(env(WORKSPACE_REMOTE_ENV)).unwrap_or_else(|| "origin".to_string());
        let push = parse_bool_env(non_empty(env(WORKSPACE_PUSH_ENV)).as_deref())?;
        let labels = non_empty(env(WORKSPACE_PR_LABELS_ENV))
            .map(|raw| parse_labels(&raw))
            .unwrap_or_else(default_pr_labels);
        Ok(Some(
            Self::new(root, vec!["/bin/sh".to_string(), "-c".to_string(), command])
                .with_remote(remote)
                .with_push(push)
                .with_pr_labels(labels),
        ))
    }

    fn produce(&self, request: CodingWorkspaceRequest) -> Result<CodingWorkspaceOutput, String> {
        if self.command.is_empty() || self.command[0].trim().is_empty() {
            return Err("coding workspace command is empty".to_string());
        }
        ensure_git_worktree(&self.root)?;
        ensure_clean(&self.root)?;
        let branch = request.branch_hint.clone();
        // The checkout capability bounds what the provider may do. A read-only
        // checkout never pushes even when the workspace is push-configured, and
        // a PR-targeted read-only checkout additionally fetches the pull
        // request's head so the command can compute the real diff.
        let checkout = request.checkout;
        let fetch_remote = self.push;
        checkout_branch(
            &self.root,
            &self.remote,
            fetch_remote,
            &branch,
            &request.base_branch,
        )?;
        if checkout.needs_pull_request_head() {
            fetch_pull_request_head(&self.root, &self.remote, fetch_remote, &request)?;
        }
        let context_file = write_context_file(&request)?;
        let result_file = result_file_path();
        let command_result = run_edit_command(
            &self.root,
            &self.command,
            &request,
            &context_file,
            &result_file,
        );
        let _ = std::fs::remove_file(&context_file);
        let result = read_result_file(&result_file);
        let _ = std::fs::remove_file(&result_file);
        command_result?;
        let result = result?.unwrap_or_default();

        // Verdict path: an external command emitted a typed verdict (e.g. a
        // reviewer's approve, an architect's rewrite or breakdown). Skip the diff
        // guard, the commit, and the push; tolerate an empty working tree; return
        // a verdict-only output carrying any routed body/review_body/children.
        if let Some(verdict) = result.verdict_id() {
            // Reject an out-of-vocabulary verdict here, at the provider, with a
            // message that names the declared option set. The runner re-checks
            // the verdict against the action's `outcomes` map and would also
            // reject it, but only with the routing key; failing here surfaces
            // the operator's mistake earlier and points at the allowed
            // vocabulary the workspace was handed. When the action declares no
            // verdict vocabulary (a pure head action) there is nothing to
            // validate against, so leave that case to the runner.
            if !request.allowed_verdicts.is_empty()
                && !request
                    .allowed_verdicts
                    .iter()
                    .any(|allowed| allowed == verdict.as_str())
            {
                return Err(format!(
                    "coding workspace command returned verdict `{}` for {}, which is not in the \
                     action's declared verdicts [{}]",
                    verdict.as_str(),
                    request.correlation_key,
                    request.allowed_verdicts.join(", "),
                ));
            }
            let mut output = CodingWorkspaceOutput::new(
                String::new(),
                request.base_branch,
                verdict_summary(&result, &verdict),
                Vec::new(),
                self.pr_labels.clone(),
            )
            .with_verdict(verdict);
            if let Some(body) = result.body {
                output = output.with_body(body);
            }
            if let Some(review_body) = result.review_body {
                output = output.with_review_body(review_body);
            }
            if !result.children.is_empty() {
                output = output.with_children(
                    result
                        .children
                        .into_iter()
                        .map(WorkspaceResultChild::into_create_issues_child),
                );
            }
            return Ok(output);
        }

        // Head path: no verdict. A read-only checkout has no head to produce, so
        // a missing verdict there is a misconfigured command, not a head; fail
        // loudly rather than committing into a tree the operator declared
        // read-only.
        if !checkout.is_writable() {
            return Err(format!(
                "read-only workspace checkout returned no verdict for {}; a read-only \
                 command must write a verdict to the result file",
                request.correlation_key
            ));
        }

        // Enforce the diff guard, commit, push, and open a PR exactly as before,
        // applying any `labels`/`summary` override from the result file.
        let changed_files = changed_files(&self.root)?;
        ensure_meaningful_diff(&changed_files)?;
        git(&self.root, &["add", "--all"])?;
        git(
            &self.root,
            &[
                "commit",
                "-m",
                &format!("Implement {}", request.correlation_key),
            ],
        )?;
        if self.push {
            git(
                &self.root,
                &[
                    "push",
                    "--force-with-lease",
                    &self.remote,
                    &format!("HEAD:refs/heads/{branch}"),
                ],
            )?;
        }
        let summary = result
            .summary
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or_else(|| format!("updated {}", changed_files.join(", ")));
        let labels = result.labels.unwrap_or_else(|| self.pr_labels.clone());
        Ok(CodingWorkspaceOutput::new(
            branch,
            request.base_branch,
            summary,
            changed_files,
            labels,
        ))
    }
}

/// A provider-created result-file path in the system temp dir, unique per
/// process and call so concurrent workspaces never collide.
fn result_file_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "temper-coding-workspace-result-{}-{}.json",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ))
}

/// Reads and parses the result file the edit command may have written. A missing
/// file is a normal absent result (the default head path); an unreadable but
/// present file, or malformed JSON, is an error.
fn read_result_file(path: &Path) -> Result<Option<WorkspaceResult>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => WorkspaceResult::parse(&contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to read coding workspace result file {}: {error}",
            path.display()
        )),
    }
}

/// One-line summary for a verdict-only output: the result's own summary if it
/// set one, else a generic note naming the verdict.
fn verdict_summary(result: &WorkspaceResult, verdict: &VerdictId) -> String {
    result
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("workspace verdict {}", verdict.as_str()))
}

#[async_trait]
impl CodingWorkspace for LocalGitCodingWorkspace {
    async fn produce_head(
        &self,
        request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
        self.produce(request).map_err(CodingWorkspaceError::new)
    }
}

fn ensure_git_worktree(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!(
            "coding workspace root {} is not a directory",
            root.display()
        ));
    }
    let out = git(root, &["rev-parse", "--is-inside-work-tree"])?;
    if out.trim() != "true" {
        return Err(format!("{} is not a git worktree", root.display()));
    }
    Ok(())
}

fn ensure_clean(root: &Path) -> Result<(), String> {
    let files = changed_files(root)?;
    if files.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "coding workspace root {} has pre-existing changes: {}",
            root.display(),
            files.join(", ")
        ))
    }
}

fn checkout_branch(
    root: &Path,
    remote: &str,
    fetch_remote: bool,
    branch: &str,
    base_branch: &str,
) -> Result<(), String> {
    if branch.trim().is_empty() || base_branch.trim().is_empty() {
        return Err("workspace branch and base branch must be non-empty".to_string());
    }
    let base_ref = if fetch_remote {
        git(root, &["fetch", remote, base_branch])?;
        format!("{remote}/{base_branch}")
    } else {
        base_branch.to_string()
    };
    git(root, &["checkout", "-B", branch, &base_ref]).map(|_| ())
}

/// Fetches the pull request's head into `TEMPER_CODING_WORKSPACE_PR_HEAD_REF` so
/// a PR-targeted read-only command can compute the real diff against the base
/// (`git diff <base> <pr-head-ref>`). Uses Forgejo/Gitea's `refs/pull/N/head`
/// convention. When the workspace is configured without a remote fetch (local
/// tests), this is a no-op: there is no remote to fetch the PR head from.
fn fetch_pull_request_head(
    root: &Path,
    remote: &str,
    fetch_remote: bool,
    request: &CodingWorkspaceRequest,
) -> Result<(), String> {
    if !fetch_remote {
        return Ok(());
    }
    let Some(number) = pull_request_number(request) else {
        // A PR-read-only checkout for a non-PR target has no head to fetch; the
        // base checkout already happened, so leave it to the command.
        return Ok(());
    };
    let local_ref = pull_request_local_ref(number);
    git(
        root,
        &[
            "fetch",
            remote,
            &format!("refs/pull/{number}/head:{local_ref}"),
        ],
    )
    .map(|_| ())
}

/// The local ref name a fetched pull-request head is stored under.
fn pull_request_local_ref(number: u64) -> String {
    format!("refs/temper/pr/{number}/head")
}

/// The pull-request number a request targets, if its work item points at a PR.
fn pull_request_number(request: &CodingWorkspaceRequest) -> Option<u64> {
    match request.work_item.target {
        temper_workflow::ArtifactSource::PullRequest { number } => Some(number.get()),
        temper_workflow::ArtifactSource::Issue { .. } => None,
    }
}

/// Stable token naming a checkout capability for the command's environment.
fn checkout_mode_token(checkout: WorkspaceCheckout) -> &'static str {
    match checkout {
        WorkspaceCheckout::Writable => "writable",
        WorkspaceCheckout::ReadOnly => "read_only",
        WorkspaceCheckout::PullRequestReadOnly => "pull_request_read_only",
    }
}

fn run_edit_command(
    root: &Path,
    command: &[String],
    request: &CodingWorkspaceRequest,
    context_file: &Path,
    result_file: &Path,
) -> Result<(), String> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .current_dir(root)
        .env("TEMPER_CODING_WORKSPACE_CONTEXT", context_file)
        .env(WORKSPACE_RESULT_ENV, result_file)
        .env("TEMPER_CODING_WORKSPACE_BRANCH", &request.branch_hint)
        .env("TEMPER_CODING_WORKSPACE_BASE", &request.base_branch)
        .env(
            "TEMPER_CODING_WORKSPACE_CHECKOUT",
            checkout_mode_token(request.checkout),
        )
        .env(
            "TEMPER_CODING_WORKSPACE_REPOSITORY",
            format!("{}/{}", request.repository.owner, request.repository.name),
        )
        .env(
            "TEMPER_CODING_WORKSPACE_CORRELATION_KEY",
            &request.correlation_key,
        );
    // A PR-targeted read-only checkout fetched the pull request's head; tell the
    // command the local ref so it can compute `git diff <base> <pr-head>`.
    if request.checkout.needs_pull_request_head()
        && let Some(number) = pull_request_number(request) {
            cmd.env(
                "TEMPER_CODING_WORKSPACE_PR_HEAD_REF",
                pull_request_local_ref(number),
            );
        }
    let output = cmd
        .output()
        .map_err(|error| format!("failed to run coding workspace command: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "coding workspace command exited with {}; stderr: {}",
            output.status,
            snippet(&String::from_utf8_lossy(&output.stderr))
        ))
    }
}

fn write_context_file(request: &CodingWorkspaceRequest) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "temper-coding-workspace-{}-{}.json",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let payload = json!({
        "repository": {
            "id": request.repository.id.as_str(),
            "owner": request.repository.owner,
            "name": request.repository.name,
            "default_branch": request.repository.default_branch,
        },
        "work_item": {
            "role": request.work_item.role.as_str(),
            "queue": request.work_item.queue.as_str(),
            "kind": request.work_item.kind.as_str(),
            "target": format!("{:?}", request.work_item.target),
            "context": request.work_item.context_json,
        },
        "base_branch": request.base_branch,
        "branch_hint": request.branch_hint,
        "correlation_key": request.correlation_key,
        "checkout": checkout_mode_token(request.checkout),
        // The verdict vocabulary the action declares. A workspace command that
        // emits a verdict must pick one of these; emitting anything else fails
        // the tick. The bound agent reads this to constrain itself to the
        // workflow's declared option set rather than guessing a verdict.
        "allowed_verdicts": request.allowed_verdicts,
        "guidance": {
            "role_guidance": request.guidance.role_guidance,
            "tool_guidance": request.guidance.tool_guidance,
            "tool_constraints": request.guidance.tool_constraints,
        }
    });
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&payload)
            .map_err(|error| format!("failed to serialize workspace context: {error}"))?,
    )
    .map_err(|error| format!("failed to write workspace context: {error}"))?;
    Ok(path)
}

fn changed_files(root: &Path) -> Result<Vec<String>, String> {
    let out = git(root, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    let mut files = Vec::new();
    for line in out.lines() {
        if line.len() < 4 {
            continue;
        }
        let raw = &line[3..];
        let path = raw
            .rsplit(" -> ")
            .next()
            .unwrap_or(raw)
            .trim_matches('"')
            .to_string();
        if !path.is_empty() {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn ensure_meaningful_diff(files: &[String]) -> Result<(), String> {
    match safety_for_files(files.to_vec()) {
        DiffSafety::Meaningful { .. } => Ok(()),
        DiffSafety::BookkeepingOnly { files } => Err(format!(
            "coding workspace produced no meaningful product diff; changed files: {}",
            if files.is_empty() {
                "(none)".to_string()
            } else {
                files.join(", ")
            }
        )),
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!(
            "git {} failed with {}; stderr: {}",
            args.join(" "),
            output.status,
            snippet(&String::from_utf8_lossy(&output.stderr))
        ))
    }
}

fn default_pr_labels() -> Vec<String> {
    vec!["implementation".to_string(), "needs-reviewer".to_string()]
}

fn parse_labels(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_bool_env(raw: Option<&str>) -> Result<bool, String> {
    match raw.unwrap_or("1") {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        other => Err(format!(
            "{WORKSPACE_PUSH_ENV} must be 1/0/true/false, got {other}"
        )),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.trim().is_empty()).then_some(value))
}

fn snippet(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() > 300 {
        let head = trimmed.chars().take(300).collect::<String>();
        format!("{head}…")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
#[path = "coding_workspace_tests.rs"]
mod tests;
