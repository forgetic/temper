use temper_protocol_agent::{WorkspaceContext, WorkspaceResult};
use temper_protocol_worker::{
    Branch, FailureClass, JobChild, PullRequestFreshness as WorkerPullRequestFreshness, RepoOutcome,
};

use crate::agent_runner::AcceptedSubmitProof;
use crate::executor::{AttemptFence, JobOutcome};
use crate::pr_freshness::{PrFreshnessFailure, PrFreshnessGuard};
use crate::pre_push::fingerprint_writable_repos;

use super::{PreparedRepo, cancelled_attempt, failure, workspace_failure};

struct WorkspaceDiffProduced<'a> {
    repo: &'a str,
    repo_root: &'a str,
    file_path: &'a str,
    changed_files: &'a str,
    changed_count: usize,
}

fn emit_workspace_diff_produced(ev: WorkspaceDiffProduced<'_>) {
    tracing::debug!(
        target: "temper::worker",
        service = "worker",
        event = "workspace.diff.produced",
        repo = ev.repo,
        repo.root = ev.repo_root,
        file.path = ev.file_path,
        changed.files = ev.changed_files,
        changed_count = ev.changed_count,
        "worker:  workspace diff produced: {} changed file(s) in {}",
        ev.changed_count,
        ev.repo,
    );
}

pub(super) struct WritableOutcomeRequest<'a> {
    pub(super) prepared: &'a [PreparedRepo],
    pub(super) result: WorkspaceResult,
    pub(super) workspace_context: &'a WorkspaceContext,
    pub(super) workspace_root: &'a std::path::Path,
    pub(super) allowed_verdicts: &'a [String],
    pub(super) coordination_key: &'a str,
    pub(super) action: &'a str,
    pub(super) artifact_item: &'a serde_json::Value,
    pub(super) pull_request_fix: bool,
    pub(super) pull_request_freshness: Option<&'a WorkerPullRequestFreshness>,
    pub(super) freshness_guard: Option<&'a dyn PrFreshnessGuard>,
    pub(super) latest_self_pushed_sha: Option<&'a str>,
    pub(super) accepted_submit: Option<&'a AcceptedSubmitProof>,
    pub(super) fence: &'a AttemptFence,
}

pub(super) async fn writable_outcome(request: WritableOutcomeRequest<'_>) -> JobOutcome {
    let WritableOutcomeRequest {
        prepared,
        result,
        workspace_context,
        workspace_root,
        allowed_verdicts,
        coordination_key,
        action,
        artifact_item,
        pull_request_fix,
        pull_request_freshness,
        freshness_guard,
        latest_self_pushed_sha,
        accepted_submit,
        fence,
    } = request;
    if !fence.is_open() {
        return cancelled_attempt();
    }
    if let Some(verdict) = result.verdict.clone() {
        return writable_verdict_outcome(
            prepared,
            result,
            verdict,
            allowed_verdicts,
            coordination_key,
            fence,
        )
        .await;
    }

    if let Err(outcome) = ensure_fresh_before_pr_push(
        pull_request_fix,
        pull_request_freshness,
        freshness_guard,
        latest_self_pushed_sha,
        fence,
    )
    .await
    {
        if !fence.is_open() {
            return cancelled_attempt();
        }
        return discard_unpublished_work(prepared, outcome, fence).await;
    }
    if let Err(outcome) = ensure_accepted_submit_before_pr_push(
        accepted_submit,
        workspace_context,
        workspace_root,
        fence,
    )
    .await
    {
        return outcome;
    }

    let outcomes = match push_writable_repos(
        prepared,
        coordination_key,
        action,
        artifact_item,
        pull_request_fix,
        fence,
    )
    .await
    {
        Ok(outcomes) => outcomes,
        Err(outcome) => return outcome,
    };
    if !fence.is_open() {
        return cancelled_attempt();
    }
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

    if !fence.is_open() {
        return cancelled_attempt();
    }
    JobOutcome::Success {
        repos: outcomes,
        title: result.title,
        body: result.body,
        summary: result
            .summary
            .or_else(|| Some(format!("implemented {coordination_key}"))),
        details: None,
    }
}

async fn discard_unpublished_work(
    prepared: &[PreparedRepo],
    outcome: JobOutcome,
    fence: &AttemptFence,
) -> JobOutcome {
    for prepared in prepared {
        if !fence.is_open() {
            return cancelled_attempt();
        }
        if let Err(error) = prepared
            .workspace
            .discard_changes_to_ref(&prepared.start_head_sha)
            .await
        {
            return workspace_failure("discard unpublished workspace changes", error);
        }
        if !fence.is_open() {
            return cancelled_attempt();
        }
    }
    outcome
}

async fn writable_verdict_outcome(
    prepared: &[PreparedRepo],
    result: WorkspaceResult,
    verdict: String,
    allowed_verdicts: &[String],
    coordination_key: &str,
    fence: &AttemptFence,
) -> JobOutcome {
    if !fence.is_open() {
        return cancelled_attempt();
    }
    if !allowed_verdicts.contains(&verdict) {
        return failure(
            FailureClass::Permanent,
            format!("verdict routing not supported by the worker yet: {verdict}"),
        );
    }

    for prepared in prepared {
        if !fence.is_open() {
            return cancelled_attempt();
        }
        if let Err(error) = prepared.workspace.discard_changes().await {
            return workspace_failure("discard verdict workspace changes", error);
        }
        if !fence.is_open() {
            return cancelled_attempt();
        }
    }
    if !fence.is_open() {
        return cancelled_attempt();
    }
    let children = result
        .children
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
        title: result.title,
        body: result.body.or(result.review_body),
        summary: result
            .summary
            .or_else(|| Some(format!("implemented {coordination_key}"))),
        children,
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
    fence: &AttemptFence,
) -> Result<(), JobOutcome> {
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
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
        Ok(()) if fence.is_open() => Ok(()),
        Ok(()) => Err(cancelled_attempt()),
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

async fn ensure_accepted_submit_before_pr_push(
    accepted_submit: Option<&AcceptedSubmitProof>,
    context: &WorkspaceContext,
    workspace_root: &std::path::Path,
    fence: &AttemptFence,
) -> Result<(), JobOutcome> {
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
    let Some(proof) = accepted_submit else {
        return Err(failure(
            FailureClass::Permanent,
            "writable success requires an accepted submit_for_pr call before final push; call submit_for_pr after the final workspace changes and retry",
        ));
    };
    if !proof.response.accepted {
        return Err(failure(
            FailureClass::Permanent,
            "writable success carried a rejected submit_for_pr proof; call submit_for_pr again after fixing the workspace",
        ));
    }

    let current = fingerprint_writable_repos(context, workspace_root)
        .await
        .map_err(|error| {
            failure(
                FailureClass::Transient,
                format!("inspect workspace fingerprint before final push: {error}"),
            )
        })?;
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
    if current == proof.fingerprint {
        return Ok(());
    }

    Err(failure(
        FailureClass::Permanent,
        "workspace changed after the accepted submit_for_pr proof; call submit_for_pr again before emitting the final WorkspaceResult JSON",
    ))
}

async fn push_writable_repos(
    prepared: &[PreparedRepo],
    coordination_key: &str,
    action: &str,
    artifact_item: &serde_json::Value,
    pull_request_fix: bool,
    fence: &AttemptFence,
) -> Result<Vec<RepoOutcome>, JobOutcome> {
    let mut outcomes = Vec::new();
    for (index, prepared) in prepared.iter().enumerate() {
        if !fence.is_open() {
            return Err(cancelled_attempt());
        }
        if let Some(outcome) = push_writable_repo(
            prepared,
            index,
            coordination_key,
            action,
            artifact_item,
            pull_request_fix,
            fence,
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
    action: &str,
    artifact_item: &serde_json::Value,
    pull_request_fix: bool,
    fence: &AttemptFence,
) -> Result<Option<RepoOutcome>, JobOutcome> {
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
    if !prepared.writable {
        return Ok(None);
    }
    let branch = prepared
        .branch_hint
        .clone()
        .expect("writable repo carries a branch hint (checked at prepare)");
    let has_tree_changes = repo_has_tree_changes(prepared).await?;
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
    if !repo_produced_diff(prepared, has_tree_changes, pull_request_fix).await? {
        return Ok(None);
    }
    emit_produced_diff(prepared, has_tree_changes, pull_request_fix).await?;
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
    if has_tree_changes {
        let message = if pull_request_fix {
            pr_fix_commit_message(coordination_key, action)
        } else {
            commit_message(coordination_key, artifact_item, index == 0)
        };
        prepared
            .workspace
            .commit_all(&message)
            .await
            .map_err(|error| workspace_failure("commit workspace changes", error))?;
        if !fence.is_open() {
            return Err(cancelled_attempt());
        }
    }
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
    let head_sha = prepared
        .workspace
        .push_branch(&branch)
        .await
        .map_err(|error| workspace_failure("push workspace branch", error))?;
    if !fence.is_open() {
        return Err(cancelled_attempt());
    }
    Ok(Some(RepoOutcome {
        repo: prepared.repo.clone(),
        branch: Branch {
            name: branch,
            head_sha,
        },
    }))
}

async fn emit_produced_diff(
    prepared: &PreparedRepo,
    has_tree_changes: bool,
    pull_request_fix: bool,
) -> Result<(), JobOutcome> {
    let paths = if has_tree_changes {
        prepared
            .workspace
            .status_paths()
            .await
            .map_err(|error| workspace_failure("inspect workspace changed paths", error))?
    } else if pull_request_fix {
        prepared
            .workspace
            .diff_paths_from_ref(&prepared.start_head_sha)
            .await
            .map_err(|error| workspace_failure("inspect workspace diff paths", error))?
    } else {
        prepared
            .workspace
            .diff_paths_from_base()
            .await
            .map_err(|error| workspace_failure("inspect workspace diff paths", error))?
    };
    let first = paths.first().cloned().unwrap_or_default();
    let joined = paths.join(",");
    emit_workspace_diff_produced(WorkspaceDiffProduced {
        repo: &prepared.repo,
        repo_root: &prepared.workspace.path().display().to_string(),
        file_path: &first,
        changed_files: &joined,
        changed_count: paths.len(),
    });
    Ok(())
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

/// Commit message for an in-place PR-head repair pushed to an existing PR head.
/// No `Closes #` trailer: the artifact number is the PR itself, not an issue,
/// and the PR's own coordinating issue is retired when the PR merges.
fn pr_fix_commit_message(coordination_key: &str, action: &str) -> String {
    match action {
        "resolve_merge_conflict" => format!("Resolve merge conflict for {coordination_key}"),
        _ => format!("Fix CI for {coordination_key}"),
    }
}
