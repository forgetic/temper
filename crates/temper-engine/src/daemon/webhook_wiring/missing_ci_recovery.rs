// SPDX-License-Identifier: MPL-2.0

//! Final, fail-closed validation and idempotent parking for expired missing-CI
//! wake intents.

use chrono::{DateTime, Utc};
use temper_forge::{
    CiJobQuery, CreateComment, Forge, HintArtifactKind, ItemListDetails, PullRequestState,
    Repository, UpdatePullRequest,
};
use temper_runner::ArtifactAddress;
use temper_workflow::{
    Classifier, CompiledWorkflow, GateCondition, MissingCiRecoveryState, NEEDS_HUMAN_LABEL,
    ValidatedWorkflow, WorkflowMetadata, matches_queue_cheap, parse_metadata_block,
    replace_metadata_block, requires_human_attention,
};

use crate::daemon::wake_coordinator::MissingCiRecoveryIntent;

const MISSING_CI_COMMENT_KEY_PREFIX: &str = "missing_current_head_ci:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum MissingCiRecoveryOutcome {
    Parked,
    Suppressed,
    Retryable { reason: String },
}

/// Revalidates an expired observation against authoritative Forge state and
/// parks the PR only while every safety predicate still holds.
pub(super) async fn recover_missing_current_head_ci<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    address: ArtifactAddress,
    intent: &MissingCiRecoveryIntent,
) -> MissingCiRecoveryOutcome {
    if address.kind != HintArtifactKind::PullRequest {
        return suppress(
            repository,
            address,
            intent,
            "intent_target_is_not_pull_request",
        );
    }

    let pull_request = match forge
        .get_pull_request_by_number_with_details(
            &repository.id,
            address.number,
            ItemListDetails::summary(),
        )
        .await
    {
        Ok(Some(pull_request)) => pull_request,
        Ok(None) => return suppress(repository, address, intent, "pull_request_missing"),
        Err(error) => {
            return retry(
                repository,
                address,
                intent,
                format!("pull_request_read_failed: {error}"),
            );
        }
    };
    if pull_request.state != PullRequestState::Open {
        return suppress(repository, address, intent, "pull_request_not_open");
    }

    let Some(current_head) = pull_request
        .head_sha
        .as_deref()
        .map(str::trim)
        .filter(|head| !head.is_empty())
    else {
        return suppress(repository, address, intent, "current_head_missing");
    };
    if current_head != intent.expected_head_sha {
        return suppress(repository, address, intent, "current_head_changed");
    }

    let jobs = match forge
        .list_ci_jobs(
            &repository.id,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                commit_sha: Some(current_head.to_string()),
                ..CiJobQuery::default()
            },
        )
        .await
    {
        Ok(jobs) => jobs,
        Err(error) => {
            return retry(
                repository,
                address,
                intent,
                format!("current_head_jobs_read_failed: {error}"),
            );
        }
    };
    if jobs.iter().any(|job| {
        job.pull_request_id.as_ref() == Some(&pull_request.id)
            && sha_identifies_head(&job.commit_sha, current_head)
    }) {
        return suppress(repository, address, intent, "current_head_job_visible");
    }

    let parsed_metadata = match parse_metadata_block(&pull_request.body) {
        Ok(metadata) => metadata.unwrap_or_default(),
        Err(error) => {
            return suppress(
                repository,
                address,
                intent,
                &format!("workflow_metadata_or_classification_invalid: {error}"),
            );
        }
    };
    let mut classification_pull_request = pull_request.clone();
    if parsed_metadata.missing_ci_recovery.is_some() {
        classification_pull_request
            .labels
            .retain(|label| label != NEEDS_HUMAN_LABEL);
    }
    let classified =
        match Classifier::new(workflow).classify_pull_request(&classification_pull_request) {
            Ok(classified) => classified,
            Err(error) => {
                return suppress(
                    repository,
                    address,
                    intent,
                    &format!("workflow_metadata_or_classification_invalid: {error}"),
                );
            }
        };
    if classified.metadata.staged {
        return suppress(repository, address, intent, "workflow_metadata_staged");
    }
    if !compiled.queues().iter().any(|queue| {
        matches!(
            queue.condition.as_ref(),
            Some(GateCondition::CiPassed | GateCondition::CiFailed)
        ) && matches_queue_cheap(queue, &classified)
    }) {
        return suppress(repository, address, intent, "not_ci_gated_workflow_queue");
    }
    if let Err(reason) = validate_repaired_head(&classified.metadata) {
        return suppress(repository, address, intent, reason);
    }
    match has_live_or_ambiguous_ownership(&classified.metadata, now) {
        Ok(true) => return suppress(repository, address, intent, "live_assignment_or_lease"),
        Ok(false) => {}
        Err(reason) => return suppress(repository, address, intent, &reason),
    }

    let marker = missing_ci_comment_marker(current_head);
    let comments = match forge.list_pull_request_comments(&pull_request.id).await {
        Ok(comments) => comments,
        Err(error) => {
            return retry(
                repository,
                address,
                intent,
                format!("audit_comments_read_failed: {error}"),
            );
        }
    };
    let audit_exists = comments
        .iter()
        .any(|comment| comment.body.contains(&marker));
    let attention_installed = requires_human_attention(&pull_request.labels);
    let mut metadata = classified.metadata.clone();
    let durable_recovery = metadata.missing_ci_recovery.clone();

    let recovery = match durable_recovery {
        Some(recovery) => {
            if recovery.head_sha.trim() != recovery.head_sha
                || recovery.head_sha.is_empty()
                || recovery.head_sha != current_head
            {
                return suppress(
                    repository,
                    address,
                    intent,
                    "different_missing_ci_recovery_in_progress",
                );
            }
            recovery
        }
        None => {
            if audit_exists {
                return suppress(repository, address, intent, "head_already_parked");
            }
            if attention_installed {
                return suppress(
                    repository,
                    address,
                    intent,
                    "unrelated_human_attention_already_required",
                );
            }
            MissingCiRecoveryState {
                head_sha: current_head.to_string(),
                first_observed_at: intent.first_observed_at,
            }
        }
    };

    // Install the public attention barrier and a durable operation marker in
    // one conditional update. If comment publication or the process fails next,
    // a fresh daemon can distinguish this operation from unrelated attention.
    let mut parking_body = pull_request.body.clone();
    let mut parking_version = pull_request.version;
    if metadata.missing_ci_recovery.is_none() || !attention_installed {
        metadata.missing_ci_recovery = Some(recovery.clone());
        parking_body = match replace_metadata_block(&parking_body, &metadata) {
            Ok(body) => body,
            Err(error) => {
                return suppress(
                    repository,
                    address,
                    intent,
                    &format!("workflow_metadata_update_invalid: {error}"),
                );
            }
        };
        let updated = match forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    body: Some(parking_body.clone()),
                    add_labels: vec![NEEDS_HUMAN_LABEL.to_string()],
                    expected_version: Some(parking_version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
        {
            Ok(updated) => updated,
            Err(error) => {
                return retry(
                    repository,
                    address,
                    intent,
                    format!("parking_barrier_write_failed: {error}"),
                );
            }
        };
        parking_body = updated.body;
        parking_version = updated.version;
    }

    if !audit_exists {
        let body = missing_ci_comment_body(
            &classified.metadata,
            current_head,
            recovery.first_observed_at,
            &marker,
        );
        if let Err(error) = forge
            .add_pull_request_comment(&pull_request.id, CreateComment { body })
            .await
        {
            return retry(
                repository,
                address,
                intent,
                format!("audit_comment_write_failed_after_barrier: {error}"),
            );
        }
    }

    // The attention label and head-keyed audit are now the durable final state;
    // remove only the operation marker so normal attention filtering resumes.
    metadata.missing_ci_recovery = None;
    let completed_body = match replace_metadata_block(&parking_body, &metadata) {
        Ok(body) => body,
        Err(error) => {
            return retry(
                repository,
                address,
                intent,
                format!("parking_marker_clear_render_failed: {error}"),
            );
        }
    };
    if let Err(error) = forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                body: Some(completed_body),
                expected_version: Some(parking_version),
                ..UpdatePullRequest::default()
            },
        )
        .await
    {
        return retry(
            repository,
            address,
            intent,
            format!("parking_marker_clear_failed: {error}"),
        );
    }

    tracing::warn!(
        target: "temper::engine",
        service = "engine",
        repo = %format!("{}/{}", repository.owner, repository.name),
        repository_id = %repository.id,
        pull_request = address.number.get(),
        expected_head_sha = %intent.expected_head_sha,
        first_observed_at = %recovery.first_observed_at,
        recovery_outcome = "parked",
        "missing-CI recovery parked pull request for human attention"
    );
    MissingCiRecoveryOutcome::Parked
}

fn validate_repaired_head(metadata: &WorkflowMetadata) -> Result<(), &'static str> {
    if metadata
        .repaired_head
        .as_deref()
        .is_some_and(|head| head.is_empty() || head.trim() != head)
    {
        Err("workflow_metadata_repaired_head_invalid")
    } else {
        Ok(())
    }
}

fn sha_identifies_head(job_sha: &str, head_sha: &str) -> bool {
    let job_sha = job_sha.trim();
    if job_sha.is_empty() {
        return false;
    }
    if job_sha.eq_ignore_ascii_case(head_sha) {
        return true;
    }

    let (shorter, longer) = if job_sha.len() < head_sha.len() {
        (job_sha, head_sha)
    } else {
        (head_sha, job_sha)
    };
    shorter.len() >= 7
        && longer
            .get(..shorter.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(shorter))
}

fn has_live_or_ambiguous_ownership(
    metadata: &WorkflowMetadata,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let assignment = metadata
        .assignment
        .as_ref()
        .map(|assignment| {
            let job_id = required_nonempty(assignment.job_id.as_deref(), "assignment.job_id")?;
            let role = assignment
                .role
                .as_ref()
                .map(|role| role.as_str())
                .filter(|role| !role.trim().is_empty())
                .ok_or_else(|| "ambiguous_ownership: assignment.role is missing".to_string())?;
            let worker =
                required_nonempty(assignment.worker_id.as_deref(), "assignment.worker_id")?;
            let expires_at = assignment.expires_at.ok_or_else(|| {
                "ambiguous_ownership: assignment.expires_at is missing".to_string()
            })?;
            Ok::<_, String>((job_id, role, worker, expires_at))
        })
        .transpose()?;
    let lease = metadata
        .lease
        .as_ref()
        .map(|lease| {
            let role = lease.role.as_str();
            if role.trim().is_empty() || lease.worker.trim().is_empty() {
                return Err("ambiguous_ownership: lease identity is empty".to_string());
            }
            Ok::<_, String>((role, lease.worker.as_str(), lease.expires_at))
        })
        .transpose()?;

    if let (
        Some((_, assignment_role, assignment_worker, assignment_expiry)),
        Some((lease_role, lease_worker, lease_expiry)),
    ) = (assignment, lease)
    {
        if assignment_role != lease_role
            || assignment_worker != lease_worker
            || assignment_expiry != lease_expiry
        {
            return Err("ambiguous_ownership: assignment and lease disagree".to_string());
        }
        return Ok(now < assignment_expiry);
    }

    Ok(
        assignment.is_some_and(|(_, _, _, expires_at)| now < expires_at)
            || lease.is_some_and(|(_, _, expires_at)| now < expires_at),
    )
}

fn required_nonempty<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, String> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("ambiguous_ownership: {field} is missing"))
}

fn missing_ci_comment_marker(head_sha: &str) -> String {
    format!("<!-- temper:comment-key={MISSING_CI_COMMENT_KEY_PREFIX}{head_sha} -->")
}

fn missing_ci_comment_body(
    metadata: &WorkflowMetadata,
    current_head: &str,
    first_observed_at: DateTime<Utc>,
    marker: &str,
) -> String {
    let repaired_head = match metadata.repaired_head.as_deref() {
        Some(repaired) if repaired == current_head => format!(
            "Workflow metadata identifies this same SHA as the matching `repaired_head`: `{repaired}`."
        ),
        Some(repaired) => format!(
            "Workflow metadata records `repaired_head` `{repaired}`, which does not match the current head."
        ),
        None => "Workflow metadata does not record a `repaired_head`.".to_string(),
    };
    format!(
        "Temper parked this pull request because no CI run or status for the current head was found during final validation.\n\nCurrent head: `{current_head}`\nMissing-CI observation began: `{first_observed_at}`\n\n{repaired_head}\n\nOperator action: retrigger CI for this exact head. After a current-head run or status appears, clear `needs-human` so workflow automation may resume.\n\n{marker}"
    )
}

fn retry(
    repository: &Repository,
    address: ArtifactAddress,
    intent: &MissingCiRecoveryIntent,
    reason: String,
) -> MissingCiRecoveryOutcome {
    tracing::warn!(
        target: "temper::engine",
        service = "engine",
        repo = %format!("{}/{}", repository.owner, repository.name),
        repository_id = %repository.id,
        pull_request = address.number.get(),
        expected_head_sha = %intent.expected_head_sha,
        first_observed_at = %intent.first_observed_at,
        recovery_outcome = "retryable",
        retry_reason = %reason,
        "missing-CI recovery remains incomplete and will be retried"
    );
    MissingCiRecoveryOutcome::Retryable { reason }
}

fn suppress(
    repository: &Repository,
    address: ArtifactAddress,
    intent: &MissingCiRecoveryIntent,
    reason: &str,
) -> MissingCiRecoveryOutcome {
    tracing::info!(
        target: "temper::engine",
        service = "engine",
        repo = %format!("{}/{}", repository.owner, repository.name),
        repository_id = %repository.id,
        pull_request = address.number.get(),
        expected_head_sha = %intent.expected_head_sha,
        first_observed_at = %intent.first_observed_at,
        recovery_outcome = "suppressed",
        suppression_reason = reason,
        "stale missing-CI recovery intent suppressed"
    );
    MissingCiRecoveryOutcome::Suppressed
}

#[cfg(test)]
#[path = "missing_ci_recovery_tests.rs"]
mod tests;
