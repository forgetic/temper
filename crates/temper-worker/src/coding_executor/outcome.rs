use temper_protocol_agent::WorkspaceResult;
use temper_protocol_worker::{
    Branch, FailureClass, PullRequestFreshness as WorkerPullRequestFreshness, RepoOutcome,
};

use crate::executor::JobOutcome;
use crate::pr_freshness::{PrFreshnessFailure, PrFreshnessGuard};

use super::{PreparedRepo, failure, workspace_failure};

pub(super) async fn writable_outcome(
    prepared: &[PreparedRepo],
    result: WorkspaceResult,
    allowed_verdicts: &[String],
    coordination_key: &str,
    artifact_item: &serde_json::Value,
    pull_request_fix: bool,
    pull_request_freshness: Option<&WorkerPullRequestFreshness>,
    freshness_guard: Option<&dyn PrFreshnessGuard>,
    latest_self_pushed_sha: Option<&str>,
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

    if let Err(outcome) = ensure_fresh_before_pr_push(
        pull_request_fix,
        pull_request_freshness,
        freshness_guard,
        latest_self_pushed_sha,
    )
    .await
    {
        return outcome;
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
        details: None,
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

fn agent_freshness(
    freshness: &WorkerPullRequestFreshness,
    latest_self_pushed_sha: Option<&str>,
) -> temper_protocol_agent::PullRequestFreshness {
    let mut check = temper_protocol_agent::PullRequestFreshness {
        repository_id: freshness.repository_id.clone(),
        repo: freshness.repo.clone(),
        role: freshness.role.clone(),
        queue: freshness.queue.clone(),
        action: freshness.action.clone(),
        number: freshness.number,
        pull_request_id: freshness.pull_request_id.clone(),
        head_sha: freshness.head_sha.clone(),
        queue_condition: freshness.queue_condition.clone(),
        queue_labels: freshness.queue_labels.clone(),
    };
    if let Some(sha) = latest_self_pushed_sha.and_then(non_empty) {
        if Some(sha) != freshness.head_sha.as_deref().and_then(non_empty) {
            check.head_sha = Some(sha.to_string());
            check.queue_condition = None;
            check.queue_labels.clear();
        }
    }
    check
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

async fn ensure_fresh_before_pr_push(
    pull_request_fix: bool,
    pull_request_freshness: Option<&WorkerPullRequestFreshness>,
    freshness_guard: Option<&dyn PrFreshnessGuard>,
    latest_self_pushed_sha: Option<&str>,
) -> Result<(), JobOutcome> {
    if !pull_request_fix {
        return Ok(());
    }
    let Some(freshness) = pull_request_freshness else {
        return Ok(());
    };
    let Some(guard) = freshness_guard else {
        return Ok(());
    };
    match guard
        .check(&agent_freshness(freshness, latest_self_pushed_sha))
        .await
    {
        Ok(()) => Ok(()),
        Err(PrFreshnessFailure::Stale(reason)) => Err(failure(
            FailureClass::Canceled,
            format!("stale pull request job canceled before push: {reason}"),
        )),
        Err(PrFreshnessFailure::Unavailable(reason)) => Err(failure(
            FailureClass::Transient,
            format!("could not revalidate pull request before push: {reason}"),
        )),
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
    if !repo_produced_diff(prepared, has_tree_changes, pull_request_fix).await? {
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
    pull_request_fix: bool,
) -> Result<bool, JobOutcome> {
    if has_tree_changes {
        return Ok(true);
    }
    if pull_request_fix {
        return prepared
            .workspace
            .tree_differs_from_ref(&prepared.start_head_sha)
            .await
            .map_err(|error| workspace_failure("inspect workspace tree diff", error));
    }
    prepared
        .workspace
        .tree_differs_from_base()
        .await
        .map_err(|error| workspace_failure("inspect workspace tree diff", error))
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
