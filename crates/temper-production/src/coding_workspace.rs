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
use serde_json::json;
use temper_runner::{
    CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
};

use crate::pr_diff_guard::{safety_for_files, DiffSafety};

pub const WORKSPACE_ROOT_ENV: &str = "TEMPER_CODING_WORKSPACE_ROOT";
pub const WORKSPACE_COMMAND_ENV: &str = "TEMPER_CODING_WORKSPACE_COMMAND";
pub const WORKSPACE_REMOTE_ENV: &str = "TEMPER_CODING_WORKSPACE_REMOTE";
pub const WORKSPACE_PUSH_ENV: &str = "TEMPER_CODING_WORKSPACE_PUSH";
pub const WORKSPACE_PR_LABELS_ENV: &str = "TEMPER_CODING_WORKSPACE_PR_LABELS";

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
        checkout_branch(
            &self.root,
            &self.remote,
            self.push,
            &branch,
            &request.base_branch,
        )?;
        let context_file = write_context_file(&request)?;
        let command_result = run_edit_command(&self.root, &self.command, &request, &context_file);
        let _ = std::fs::remove_file(&context_file);
        command_result?;
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
        let summary = format!("updated {}", changed_files.join(", "));
        Ok(CodingWorkspaceOutput::new(
            branch,
            request.base_branch,
            summary,
            changed_files,
            self.pr_labels.clone(),
        ))
    }
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
    push: bool,
    branch: &str,
    base_branch: &str,
) -> Result<(), String> {
    if branch.trim().is_empty() || base_branch.trim().is_empty() {
        return Err("workspace branch and base branch must be non-empty".to_string());
    }
    let base_ref = if push {
        git(root, &["fetch", remote, base_branch])?;
        format!("{remote}/{base_branch}")
    } else {
        base_branch.to_string()
    };
    git(root, &["checkout", "-B", branch, &base_ref]).map(|_| ())
}

fn run_edit_command(
    root: &Path,
    command: &[String],
    request: &CodingWorkspaceRequest,
    context_file: &Path,
) -> Result<(), String> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .current_dir(root)
        .env("TEMPER_CODING_WORKSPACE_CONTEXT", context_file)
        .env("TEMPER_CODING_WORKSPACE_BRANCH", &request.branch_hint)
        .env("TEMPER_CODING_WORKSPACE_BASE", &request.base_branch)
        .env(
            "TEMPER_CODING_WORKSPACE_REPOSITORY",
            format!("{}/{}", request.repository.owner, request.repository.name),
        )
        .env(
            "TEMPER_CODING_WORKSPACE_CORRELATION_KEY",
            &request.correlation_key,
        );
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
    vec![
        "implementation".to_string(),
        "needs-reviewer".to_string(),
        "needs-merge".to_string(),
    ]
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
