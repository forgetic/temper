use temper_agent_protocol::WorkspaceResult;
use temper_worker_protocol::{Branch, FailureClass, RepoOutcome};

use crate::executor::JobOutcome;

use super::{PreparedRepo, failure, workspace_failure};

pub(super) async fn writable_outcome(
    prepared: &[PreparedRepo],
    result: WorkspaceResult,
    allowed_verdicts: &[String],
    coordination_key: &str,
    artifact_item: &serde_json::Value,
    pull_request_fix: bool,
) -> JobOutcome {
    if let Some(verdict) = result.verdict.clone() {
        return writable_verdict_outcome(
            prepared,
            result,
            verdict,
            allowed_verdicts,
            coordination_key,
        )
        .await;
    }

    let outcomes = match push_writable_repos(
        prepared,
        coordination_key,
        artifact_item,
        pull_request_fix,
    )
    .await
    {
        Ok(outcomes) => outcomes,
        Err(outcome) => return outcome,
    };
    if outcomes.is_empty() {
        // A PR-fix job that changes nothing leaves CI red — surface it as a
        // failure so it does not read as "addressed". An ordinary writable job
        // with no diff is likewise nothing to apply.
        let message = if pull_request_fix {
            "agent made no change to the pull request head; CI stays red".to_string()
        } else {
            "agent produced no diff in any writable repo".to_string()
        };
        return failure(FailureClass::Permanent, message);
    }

    JobOutcome::Success {
        repos: outcomes,
        summary: result
            .summary
            .or_else(|| Some(format!("implemented {coordination_key}"))),
    }
}

async fn writable_verdict_outcome(
    prepared: &[PreparedRepo],
    result: WorkspaceResult,
    verdict: String,
    allowed_verdicts: &[String],
    coordination_key: &str,
) -> JobOutcome {
    if !allowed_verdicts.contains(&verdict) {
        return failure(
            FailureClass::Permanent,
            format!("verdict routing not supported by the worker yet: {verdict}"),
        );
    }

    for prepared in prepared {
        if let Err(error) = prepared.workspace.discard_changes().await {
            return workspace_failure("discard verdict workspace changes", error);
        }
    }
    JobOutcome::Verdict {
        verdict,
        body: result.body.or(result.review_body),
        summary: result
            .summary
            .or_else(|| Some(format!("implemented {coordination_key}"))),
        children: Vec::new(),
    }
}

async fn push_writable_repos(
    prepared: &[PreparedRepo],
    coordination_key: &str,
    artifact_item: &serde_json::Value,
    pull_request_fix: bool,
) -> Result<Vec<RepoOutcome>, JobOutcome> {
    let mut outcomes = Vec::new();
    for (index, prepared) in prepared.iter().enumerate() {
        if let Some(outcome) = push_writable_repo(
            prepared,
            index,
            coordination_key,
            artifact_item,
            pull_request_fix,
        )
        .await?
        {
            outcomes.push(outcome);
        }
    }
    Ok(outcomes)
}

async fn push_writable_repo(
    prepared: &PreparedRepo,
    index: usize,
    coordination_key: &str,
    artifact_item: &serde_json::Value,
    pull_request_fix: bool,
) -> Result<Option<RepoOutcome>, JobOutcome> {
    if !prepared.writable {
        return Ok(None);
    }
    let branch = prepared
        .branch_hint
        .clone()
        .expect("writable repo carries a branch hint (checked at prepare)");
    let has_tree_changes = repo_has_tree_changes(prepared).await?;
    if !repo_produced_diff(prepared, has_tree_changes).await? {
        return Ok(None);
    }
    if has_tree_changes {
        let message = if pull_request_fix {
            ci_fix_commit_message(coordination_key)
        } else {
            commit_message(coordination_key, artifact_item, index == 0)
        };
        prepared
            .workspace
            .commit_all(&message)
            .await
            .map_err(|error| workspace_failure("commit workspace changes", error))?;
    }
    let head_sha = prepared
        .workspace
        .push_branch(&branch)
        .await
        .map_err(|error| workspace_failure("push workspace branch", error))?;
    Ok(Some(RepoOutcome {
        repo: prepared.repo.clone(),
        branch: Branch {
            name: branch,
            head_sha,
        },
    }))
}

async fn repo_has_tree_changes(prepared: &PreparedRepo) -> Result<bool, JobOutcome> {
    prepared
        .workspace
        .has_changes()
        .await
        .map_err(|error| workspace_failure("inspect workspace changes", error))
}

async fn repo_produced_diff(
    prepared: &PreparedRepo,
    has_tree_changes: bool,
) -> Result<bool, JobOutcome> {
    if has_tree_changes {
        return Ok(true);
    }
    prepared
        .workspace
        .commits_ahead_of_base()
        .await
        .map_err(|error| workspace_failure("inspect workspace commits", error))
}

/// Builds the implementation commit message.
///
/// The primary repo's commit gains a `Closes #<n>` trailer so the forge's
/// native close-on-merge retires the coordinating issue when that PR lands (the
/// daemon applies no issue transition on success, so this trailer is what
/// retires the source issue and its queue entry at merge time). Secondary repos
/// cannot close a cross-repo issue, so they omit the trailer.
fn commit_message(
    coordination_key: &str,
    artifact_item: &serde_json::Value,
    is_primary: bool,
) -> String {
    match (is_primary, artifact_item.as_u64()) {
        (true, Some(number)) => format!("Implement {coordination_key}\n\nCloses #{number}"),
        _ => format!("Implement {coordination_key}"),
    }
}

/// Commit message for an in-place CI-failure fix pushed to an existing PR head.
/// No `Closes #` trailer: the artifact number is the PR itself, not an issue,
/// and the PR's own coordinating issue is retired when the PR merges.
fn ci_fix_commit_message(coordination_key: &str) -> String {
    format!("Fix CI for {coordination_key}")
}
