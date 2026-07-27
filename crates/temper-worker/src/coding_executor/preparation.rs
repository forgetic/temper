// SPDX-License-Identifier: MPL-2.0

//! Workspace preparation for coding and validation jobs.

use std::path::Path;

use temper_protocol_worker::{FailureClass, WorkspaceManifest, WorkspaceRepo};

use crate::executor::{JobCancellation, JobOutcome};
use crate::workspace::{
    PreparationOutcome, QuarantineManifest, RecoveryContext, RoleGitIdentity, Workspace,
    forgejo_remote_url,
};

use super::{JobMode, failure, workspace_failure};

/// One prepared sibling checkout plus its manifest entry.
pub(crate) struct PreparedRepo {
    pub(super) repo: String,
    pub(super) writable: bool,
    pub(super) branch_hint: Option<String>,
    /// The commit checked out before the agent ran. PR-head repair jobs use this
    /// as their product-diff baseline.
    pub(super) start_head_sha: String,
    pub(super) workspace: Workspace,
}

pub(super) struct PrepareRequest<'a> {
    pub(super) git_base_url: &'a str,
    pub(super) identity: &'a RoleGitIdentity,
    pub(super) workspace_root: &'a Path,
    pub(super) manifest: &'a WorkspaceManifest,
    pub(super) artifact_number: u64,
    pub(super) mode: JobMode,
    pub(super) coordination_key: &'a str,
    pub(super) job_id: &'a str,
    pub(super) cancellation: &'a JobCancellation,
}

pub(super) async fn prepare_repos(
    request: PrepareRequest<'_>,
) -> Result<Vec<PreparedRepo>, JobOutcome> {
    let mut prepared = Vec::new();
    for repo_spec in &request.manifest.repos {
        prepared.push(prepare_repo(&request, repo_spec).await?);
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
        JobMode::PullRequestWritable => {
            return prepare_writable(workspace, repo_spec, None).await;
        }
        JobMode::Writable if repo_spec.is_writable() => {
            return prepare_writable(workspace, repo_spec, Some(default_branch)).await;
        }
        JobMode::ReadOnly => {
            workspace
                .prepare_read_only_from_default(default_branch)
                .await
        }
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
