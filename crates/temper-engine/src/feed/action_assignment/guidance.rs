// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_forge::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobStatus, Forge, ForgeError, PullRequest,
    PullRequestReview, RepositoryId, ReviewDecision,
};
use temper_runner::{ScanError, WorkItem};
use temper_workflow::{ArtifactSource, CiStatus, CompiledWorkflow, GateCondition};

pub(super) async fn pull_request_writable_guidance<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    compiled: &CompiledWorkflow,
    item: &WorkItem,
    query: &CiJobQuery,
    action: &str,
    pull_request: &PullRequest,
    head_branch: &str,
    base_branch: &str,
) -> Result<String, ScanError> {
    let handoff = current_pull_request_handoff(pull_request, head_branch, base_branch);
    let gate_feedback = if is_ci_failed_pull_request_queue(item, compiled) {
        ci_failure_clause(forge, repo, query).await?
    } else if is_review_changes_requested_pull_request_queue(item, compiled) {
        review_changes_requested_clause(forge, pull_request).await
    } else if is_merge_conflict_pull_request_queue(item, compiled) {
        merge_conflict_clause(compiled, item, base_branch)
    } else {
        queue_match_clause(compiled, item)
    };

    Ok(format!(
        "Assigned workflow action `{action}` for queue `{}` requires updating this existing implementation pull request head.\n\n\
         {handoff}\n\n\
         {gate_feedback}\n\n\
         You are checked out on the PR head branch `{head_branch}` in WRITABLE mode: make the smallest fix, commit it, \
         and Temper will push it back to that branch so workflow gates can re-evaluate. Do NOT report success without changing files. \
         On PR repair success, emit no verdict and include an updated current PR `title`, a compact implementation-report `body` \
         (preserving the Temper workflow metadata block if present), and a short `summary` describing the fix. Do not create a hidden implementation-report block.",
        item.queue.as_str()
    ))
}

fn current_pull_request_handoff(
    pull_request: &PullRequest,
    head_branch: &str,
    base_branch: &str,
) -> String {
    let labels = comma_list(&pull_request.labels);
    let head_sha = optional_value(pull_request.head_sha.as_deref());
    let base_sha = optional_value(pull_request.base_sha.as_deref());
    let body = if pull_request.body.trim().is_empty() {
        "(empty)".to_string()
    } else {
        pull_request.body.trim_end().to_string()
    };

    format!(
        "Current implementation PR handoff from Forge (durable title/body/report to preserve and update; not a local resume packet):\n\
         - pr_number: #{}\n\
         - title: {}\n\
         - head_branch: {}\n\
         - base_branch: {}\n\
         - head_sha: {head_sha}\n\
         - base_sha: {base_sha}\n\
         - labels: {labels}\n\
         - body/report:\n\
         --- BEGIN CURRENT PR BODY ---\n{}\n--- END CURRENT PR BODY ---",
        pull_request.number.get(),
        pull_request.title,
        optional_value(Some(head_branch.trim())),
        optional_value(Some(base_branch.trim())),
        body
    )
}

fn is_ci_failed_pull_request_queue(item: &WorkItem, compiled: &CompiledWorkflow) -> bool {
    is_pull_request_queue_with_condition(item, compiled, |condition| {
        matches!(condition, GateCondition::CiFailed)
    })
}

fn is_review_changes_requested_pull_request_queue(
    item: &WorkItem,
    compiled: &CompiledWorkflow,
) -> bool {
    is_pull_request_queue_with_condition(item, compiled, |condition| {
        matches!(condition, GateCondition::ReviewChangesRequested)
    })
}

fn is_pull_request_queue_with_condition(
    item: &WorkItem,
    compiled: &CompiledWorkflow,
    predicate: impl FnOnce(&GateCondition) -> bool,
) -> bool {
    if !matches!(item.target, ArtifactSource::PullRequest { .. }) {
        return false;
    }
    compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == item.queue.as_str())
        .and_then(|queue| queue.condition.as_ref())
        .is_some_and(predicate)
}

fn is_merge_conflict_pull_request_queue(item: &WorkItem, compiled: &CompiledWorkflow) -> bool {
    if !matches!(item.target, ArtifactSource::PullRequest { .. }) {
        return false;
    }
    compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == item.queue.as_str())
        .is_some_and(|queue| {
            queue
                .labels
                .iter()
                .any(|label| label.as_str() == "merge-conflict")
        })
}

fn merge_conflict_clause(
    compiled: &CompiledWorkflow,
    item: &WorkItem,
    base_branch: &str,
) -> String {
    let base_branch = base_branch.trim();
    let base_branch = if base_branch.is_empty() {
        "the target branch"
    } else {
        base_branch
    };
    format!(
        "Fresh assignment-time gate feedback from Forge:\n\
         - reason: merge_conflict\n\
         - matched_labels: {}\n\
         - guidance: Mechanical landing found a merge conflict with {base_branch}. Rebase or merge {base_branch} into the PR head, resolve conflicts, keep the repair scoped to the conflict resolution, and push the updated head; CI will rerun before landing.",
        comma_list(&queue_labels(compiled, item))
    )
}

fn queue_match_clause(compiled: &CompiledWorkflow, item: &WorkItem) -> String {
    let Some(queue) = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == item.queue.as_str())
    else {
        return "Fresh assignment-time gate feedback from Forge:\n- reason: queue_match\n- detail: The queue matched workflow state requiring a PR-head update.".to_string();
    };

    let mut lines = vec![
        "Fresh assignment-time gate feedback from Forge:".to_string(),
        format!(
            "- reason: {}",
            queue
                .condition
                .as_ref()
                .and_then(condition_token)
                .unwrap_or_else(|| "queue_match".to_string())
        ),
    ];
    if !queue.labels.is_empty() {
        lines.push(format!(
            "- matched_labels: {}",
            comma_list(
                &queue
                    .labels
                    .iter()
                    .map(|label| label.as_str().to_string())
                    .collect::<Vec<_>>()
            )
        ));
    }
    if !queue.excluded_labels.is_empty() {
        lines.push(format!(
            "- excluded_labels: {}",
            comma_list(
                &queue
                    .excluded_labels
                    .iter()
                    .map(|label| label.as_str().to_string())
                    .collect::<Vec<_>>()
            )
        ));
    }
    lines
        .push("- detail: The queue matched workflow state requiring a PR-head update.".to_string());
    lines.join("\n")
}

async fn ci_failure_clause<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    query: &CiJobQuery,
) -> Result<String, ScanError> {
    let jobs = forge.list_ci_jobs(repo, query.clone()).await?;
    let status = CiStatus::from_jobs_for_head(&jobs, query.commit_sha.as_deref());
    if !status.is_failed() {
        return Err(ScanError::Forge(ForgeError::Conflict(format!(
            "current-head CI is {:?}; refusing stale writable code-repair guidance without explicit ordinary failure evidence",
            status.state()
        ))));
    }
    let ordinary_failure_ids = status
        .terminal_evidence()
        .iter()
        .filter(|evidence| evidence.conclusion == CiJobConclusion::Failure)
        .map(|evidence| evidence.job_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut failing = jobs
        .into_iter()
        .filter(|job| ordinary_failure_ids.contains(&job.id))
        .collect::<Vec<_>>();
    failing.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    let jobs = failing
        .iter()
        .map(format_ci_job)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "Fresh assignment-time gate feedback from Forge:\n\
         - reason: ci_failed\n\
         - failing_jobs:\n{jobs}\n\
         - guidance: Inspect the failing CI job details above and make CI pass."
    ))
}

async fn review_changes_requested_clause<F: Forge + ?Sized>(
    forge: &F,
    pull_request: &PullRequest,
) -> String {
    let changes_requested = match forge.list_pull_request_reviews(&pull_request.id).await {
        Ok(reviews) => latest_changes_requested_reviews(reviews),
        Err(error) => {
            return format!(
                "Fresh assignment-time gate feedback from Forge:\n\
                 - reason: review_changes_requested\n\
                 - changes_requested_reviews: unavailable ({error})\n\
                 - guidance: A native pull request review has requested changes; inspect Forge and address it."
            );
        }
    };

    if changes_requested.is_empty() {
        return "Fresh assignment-time gate feedback from Forge:\n\
                - reason: review_changes_requested\n\
                - changes_requested_reviews: []\n\
                - guidance: A native pull request review has requested changes, but Forge returned no latest changes-requested review body. Inspect the PR review thread and address the requested changes."
            .to_string();
    }

    let reviews = changes_requested
        .iter()
        .map(format_changes_requested_review)
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Fresh assignment-time gate feedback from Forge:\n\
         - reason: review_changes_requested\n\
         - changes_requested_reviews:\n{reviews}\n\
         - guidance: Address the latest changes-requested review(s) above."
    )
}

fn format_ci_job(job: &CiJob) -> String {
    let name = if job.name.trim().is_empty() {
        "(unnamed)"
    } else {
        job.name.trim()
    };
    format!(
        "  - name: {name}\n    status: {}\n    conclusion: {}\n    provider_conclusion: {}\n    provider_reason: {}\n    run_id: {}\n    attempt: {}\n    commit_sha: {}\n    url: {}",
        ci_status_token(job.status),
        ci_conclusion_token(job.conclusion),
        optional_value(job.provider_conclusion.as_deref().map(str::trim)),
        optional_value(job.provider_reason.as_deref().map(str::trim)),
        optional_value(job.run_id.as_deref().map(str::trim)),
        optional_value(job.attempt.as_deref().map(str::trim)),
        optional_value(Some(job.commit_sha.trim())),
        optional_value(job.url.as_deref().map(str::trim))
    )
}

fn latest_changes_requested_reviews(reviews: Vec<PullRequestReview>) -> Vec<PullRequestReview> {
    let mut latest_by_reviewer = BTreeMap::new();
    for review in reviews {
        if review.decision == ReviewDecision::Commented {
            continue;
        }
        latest_by_reviewer
            .entry(review.reviewer_id.clone())
            .and_modify(|current| {
                if review_is_newer(&review, current) {
                    *current = review.clone();
                }
            })
            .or_insert(review);
    }
    latest_by_reviewer
        .into_values()
        .filter(|review| review.decision == ReviewDecision::ChangesRequested)
        .collect()
}

fn review_is_newer(candidate: &PullRequestReview, current: &PullRequestReview) -> bool {
    candidate.submitted_at > current.submitted_at
        || (candidate.submitted_at == current.submitted_at && candidate.id >= current.id)
}

fn format_changes_requested_review(review: &PullRequestReview) -> String {
    let body = review
        .body
        .as_deref()
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .unwrap_or("(no body)");
    format!(
        "  - reviewer: {}\n    submitted_at: {}\n    body: |\n{}",
        review.reviewer_id.as_str(),
        review.submitted_at.to_rfc3339(),
        indent_block(body, "      ")
    )
}

fn queue_labels(compiled: &CompiledWorkflow, item: &WorkItem) -> Vec<String> {
    compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == item.queue.as_str())
        .map(|queue| {
            queue
                .labels
                .iter()
                .map(|label| label.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn condition_token(condition: &GateCondition) -> Option<String> {
    let token = match condition {
        GateCondition::CiPassed => "ci_passed",
        GateCondition::CiFailed => "ci_failed",
        GateCondition::CiRecoveryRequired => "ci_recovery_required",
        GateCondition::ReviewApproved => "review_approved",
        GateCondition::ReviewChangesRequested => "review_changes_requested",
        GateCondition::ExactHeadValidation => "exact_head_validation",
        GateCondition::DependenciesResolved
        | GateCondition::LabelPresent(_)
        | GateCondition::StateEquals { .. } => return None,
    };
    Some(token.to_string())
}

fn optional_value(value: Option<&str>) -> &str {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("(unknown)")
}

fn comma_list(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

fn indent_block(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ci_status_token(status: CiJobStatus) -> &'static str {
    match status {
        CiJobStatus::Queued => "queued",
        CiJobStatus::Running => "running",
        CiJobStatus::Completed => "completed",
    }
}

fn ci_conclusion_token(conclusion: Option<CiJobConclusion>) -> &'static str {
    match conclusion {
        Some(CiJobConclusion::Success) => "success",
        Some(CiJobConclusion::Failure) => "failure",
        Some(CiJobConclusion::Cancelled) => "cancelled",
        Some(CiJobConclusion::Interrupted) => "interrupted",
        Some(CiJobConclusion::TimedOut) => "timed_out",
        Some(CiJobConclusion::RunnerLost) => "runner_lost",
        Some(CiJobConclusion::StartupFailure) => "startup_failure",
        Some(CiJobConclusion::ActionRequired) => "action_required",
        Some(CiJobConclusion::Neutral) => "neutral",
        Some(CiJobConclusion::Skipped) => "skipped",
        Some(CiJobConclusion::Unknown) => "unknown",
        None => "none",
    }
}
