// SPDX-License-Identifier: MPL-2.0

//! Human-attention barrier and deduplicated interrupted-CI audit.

use super::*;

const COMMENT_KEY_PREFIX: &str = "interrupted_ci_recovery:";

pub(super) async fn park_interrupted_ci<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    address: ArtifactAddress,
    expected: &InterruptedCiRecoveryState,
) -> InterruptedCiRecoveryOutcome {
    // Final exact revalidation immediately before installing the barrier.
    let fresh = match load_fresh_attempt(forge, repository, address).await {
        Ok(Some(fresh)) => fresh,
        Ok(None) => return InterruptedCiRecoveryOutcome::Suppressed,
        Err(reason) => return InterruptedCiRecoveryOutcome::Retryable { reason },
    };
    let mut metadata = match parse_metadata_block(&fresh.pull_request.body) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return InterruptedCiRecoveryOutcome::Suppressed,
        Err(error) => {
            return InterruptedCiRecoveryOutcome::Retryable {
                reason: format!("workflow_metadata_invalid: {error}"),
            };
        }
    };
    let Some(mut current) = metadata.interrupted_ci_recovery.clone() else {
        return InterruptedCiRecoveryOutcome::Suppressed;
    };
    let Some(identity) = recovery_identity(repository, &fresh) else {
        return InterruptedCiRecoveryOutcome::Suppressed;
    };
    if !fresh.status.is_recovery_required() {
        return clear_superseded_marker(forge, fresh.pull_request, metadata).await;
    }
    if !same_identity(&current, &identity) {
        return reconcile_changed_identity(forge, fresh.pull_request, metadata, identity).await;
    }
    if !same_identity(&current, expected)
        || metadata.assignment.is_some()
        || metadata.lease.is_some()
    {
        return InterruptedCiRecoveryOutcome::Waiting;
    }
    if requires_human_attention(&fresh.pull_request.labels) && !current.parking_barrier_installed {
        // This recovery does not own an attention barrier that appeared while
        // retry/diagnosis was in progress. Relinquish only our marker and leave
        // the external ownership barrier untouched.
        return clear_superseded_marker(forge, fresh.pull_request, metadata).await;
    }

    let marker = audit_marker(&current);
    let comments = match forge
        .list_pull_request_comments(&fresh.pull_request.id)
        .await
    {
        Ok(comments) => comments,
        Err(error) => {
            return InterruptedCiRecoveryOutcome::Retryable {
                reason: format!("audit_comments_read_failed: {error}"),
            };
        }
    };
    let audit_exists = comments
        .iter()
        .any(|comment| comment.body.contains(&marker));
    let mut parked = fresh.pull_request;
    if !current.parking_barrier_installed || !requires_human_attention(&parked.labels) {
        current.parking_barrier_installed = true;
        metadata.interrupted_ci_recovery = Some(current.clone());
        let body = match replace_metadata_block(&parked.body, &metadata) {
            Ok(body) => body,
            Err(error) => {
                return InterruptedCiRecoveryOutcome::Retryable {
                    reason: format!("parking_barrier_render_failed: {error}"),
                };
            }
        };
        if let Err(error) = forge
            .update_pull_request(
                &parked.id,
                UpdatePullRequest {
                    body: Some(body),
                    add_labels: (!requires_human_attention(&parked.labels))
                        .then(|| NEEDS_HUMAN_LABEL.to_string())
                        .into_iter()
                        .collect(),
                    expected_version: Some(parked.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
        {
            return InterruptedCiRecoveryOutcome::Retryable {
                reason: format!("parking_barrier_write_failed: {error}"),
            };
        }
    }

    // The barrier is not authority to publish stale evidence. Re-read the PR
    // and exact latest attempt again immediately before the audit side effect.
    let before_audit = match load_fresh_attempt(forge, repository, address).await {
        Ok(Some(fresh)) => fresh,
        Ok(None) => return InterruptedCiRecoveryOutcome::Suppressed,
        Err(reason) => return InterruptedCiRecoveryOutcome::Retryable { reason },
    };
    let before_audit_metadata = match parse_metadata_block(&before_audit.pull_request.body) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return InterruptedCiRecoveryOutcome::Suppressed,
        Err(error) => {
            return InterruptedCiRecoveryOutcome::Retryable {
                reason: format!("workflow_metadata_invalid_before_audit: {error}"),
            };
        }
    };
    let Some(before_audit_state) = before_audit_metadata.interrupted_ci_recovery.as_ref() else {
        return InterruptedCiRecoveryOutcome::Suppressed;
    };
    let Some(before_audit_identity) = recovery_identity(repository, &before_audit) else {
        return clear_superseded_marker(forge, before_audit.pull_request, before_audit_metadata)
            .await;
    };
    if !before_audit.status.is_recovery_required() {
        return clear_superseded_marker(forge, before_audit.pull_request, before_audit_metadata)
            .await;
    }
    if !same_identity(before_audit_state, &before_audit_identity) {
        return reconcile_changed_identity(
            forge,
            before_audit.pull_request,
            before_audit_metadata,
            before_audit_identity,
        )
        .await;
    }
    if !same_identity(before_audit_state, &current) {
        return InterruptedCiRecoveryOutcome::Waiting;
    }
    parked = before_audit.pull_request;
    metadata = before_audit_metadata;
    current = metadata
        .interrupted_ci_recovery
        .clone()
        .expect("validated immediately above");

    if !audit_exists {
        if let Err(error) = forge
            .add_pull_request_comment(
                &parked.id,
                CreateComment {
                    body: audit_body(&current, &marker),
                },
            )
            .await
        {
            return InterruptedCiRecoveryOutcome::Retryable {
                reason: format!("audit_comment_write_failed_after_barrier: {error}"),
            };
        }
    }

    metadata.interrupted_ci_recovery = None;
    match write_metadata(forge, &parked, &metadata).await {
        Ok(_) => InterruptedCiRecoveryOutcome::Parked,
        Err(reason) => InterruptedCiRecoveryOutcome::Retryable {
            reason: format!("parking_marker_clear_failed: {reason}"),
        },
    }
}

fn audit_marker(state: &InterruptedCiRecoveryState) -> String {
    let identity = serde_json::to_vec(&(
        &state.repository_id,
        &state.pull_request_id,
        &state.head_sha,
        &state.run_id,
        &state.attempt,
        &state.latest_jobs,
    ))
    .unwrap_or_default();
    let hash = identity.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("<!-- temper:comment-key={COMMENT_KEY_PREFIX}{hash:016x} -->")
}

fn audit_body(state: &InterruptedCiRecoveryState, marker: &str) -> String {
    let retry = match state.retry_outcome {
        Some(outcome) => format!("{outcome:?}"),
        None if state.retry_started => "Uncertain".to_string(),
        None => "NotRequested".to_string(),
    };
    let diagnostic = match state.diagnostic.as_ref() {
        Some(diagnostic) => format!(
            "action `{}` on queue `{}` for role `{}`; publication job: {}",
            diagnostic.action,
            diagnostic.queue,
            diagnostic.role,
            diagnostic.job_id.as_deref().unwrap_or("not published")
        ),
        None => "not configured".to_string(),
    };
    let jobs = state
        .evidence
        .iter()
        .map(|evidence| {
            format!(
                "- `{}` (`{}`): conclusion=`{:?}`, provider_conclusion={}, provider_reason={}, run=`{}`, attempt=`{}`, created=`{}`, started={}, completed={}, updated=`{}`, url={}",
                evidence.job_name,
                evidence.job_id,
                evidence.conclusion,
                evidence.provider_conclusion.as_deref().unwrap_or("(unavailable)"),
                evidence.provider_reason.as_deref().unwrap_or("(unavailable)"),
                evidence.run_id.as_deref().unwrap_or("(unavailable)"),
                evidence.attempt.as_deref().unwrap_or("(unavailable)"),
                evidence.created_at,
                evidence.started_at.map(|value| value.to_string()).unwrap_or_else(|| "(unavailable)".to_string()),
                evidence.completed_at.map(|value| value.to_string()).unwrap_or_else(|| "(unavailable)".to_string()),
                evidence.updated_at,
                evidence.url.as_deref().unwrap_or("(unavailable)"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Temper parked this pull request after bounded recovery of a non-repairable interrupted CI attempt. No source-repair worker was dispatched.\n\nRepository: `{}`\nPull request id: `{}`\nCurrent head: `{}`\nRun: `{}`\nAttempt: `{}`\nProvider retry: `{retry}`\nDiagnostic recovery: {diagnostic}\n\nLatest-job evidence:\n{jobs}\n\nOperator action: inspect the linked provider jobs and runner infrastructure, then safely retrigger CI for this exact head or clear `needs-human` only after a newer exact-head attempt is visible.\n\n{marker}",
        state.repository_id, state.pull_request_id, state.head_sha, state.run_id, state.attempt,
    )
}
