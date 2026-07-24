// SPDX-License-Identifier: MPL-2.0

//! Bounded, exact-attempt recovery for non-repairable terminal CI.

mod parking;

use self::parking::park_interrupted_ci;

use chrono::{DateTime, Duration, Utc};
use temper_forge::{
    CiJob, CiJobQuery, CiRetryJobSetFingerprint, CiRetryOutcome, CiRetryRequest, CreateComment,
    Forge, HintArtifactKind, ItemListDetails, PullRequest, PullRequestState, Repository,
    UpdatePullRequest,
};
use temper_runner::ArtifactAddress;
use temper_workflow::{
    CiStatus, Classifier, CompiledWorkflow, GateCondition, InterruptedCiDiagnosticState,
    InterruptedCiRecoveryState, NEEDS_HUMAN_LABEL, RoleId, ValidatedWorkflow, WorkflowMetadata,
    matches_queue_cheap, parse_metadata_block, replace_metadata_block, requires_human_attention,
};

const RETRY_CONVERGENCE_GRACE: Duration = Duration::minutes(5);
const UNKNOWN_IDENTITY: &str = "(unavailable)";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InterruptedCiRecoveryOutcome {
    /// The one configured read-only diagnostic may be enqueued.
    DispatchDiagnostic,
    /// Recovery made progress or is awaiting provider/worker convergence.
    Waiting,
    /// Fresh state superseded the recovery identity.
    Suppressed,
    /// Recovery exhausted and installed the human-attention barrier and audit.
    Parked,
    /// A transient Forge/CAS boundary must be retried.
    Retryable { reason: String },
}

#[derive(Clone)]
struct FreshAttempt {
    pull_request: PullRequest,
    latest_jobs: Vec<CiJob>,
    status: CiStatus,
}

/// Revalidates and advances recovery for one exact pull request.
///
/// Every provider mutation is preceded by a fresh PR and exact-head job read.
/// Durable metadata is installed before the retry request and diagnostic
/// publication boundaries, so daemon replacement cannot repeat either action.
pub(crate) async fn recover_interrupted_ci<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    address: ArtifactAddress,
) -> InterruptedCiRecoveryOutcome {
    if address.kind != HintArtifactKind::PullRequest {
        return InterruptedCiRecoveryOutcome::Suppressed;
    }

    let fresh = match load_fresh_attempt(forge, repository, address).await {
        Ok(Some(fresh)) => fresh,
        Ok(None) => return InterruptedCiRecoveryOutcome::Suppressed,
        Err(reason) => return InterruptedCiRecoveryOutcome::Retryable { reason },
    };
    let mut metadata = match parse_metadata_block(&fresh.pull_request.body) {
        Ok(metadata) => metadata.unwrap_or_default(),
        Err(error) => {
            return InterruptedCiRecoveryOutcome::Retryable {
                reason: format!("workflow_metadata_invalid: {error}"),
            };
        }
    };

    if !fresh.status.is_recovery_required() {
        return clear_superseded_marker(forge, fresh.pull_request, metadata).await;
    }
    let Some(current) = recovery_identity(repository, &fresh) else {
        return InterruptedCiRecoveryOutcome::Retryable {
            reason: "latest_current_head_fingerprint_invalid".to_string(),
        };
    };

    if metadata
        .interrupted_ci_recovery
        .as_ref()
        .is_some_and(|existing| !same_identity(existing, &current))
    {
        return reconcile_changed_identity(forge, fresh.pull_request, metadata, current).await;
    }
    if metadata.interrupted_ci_recovery.is_none() {
        if requires_human_attention(&fresh.pull_request.labels) {
            return InterruptedCiRecoveryOutcome::Suppressed;
        }
        if metadata.assignment.is_some() || metadata.lease.is_some() {
            return InterruptedCiRecoveryOutcome::Waiting;
        }
        let mut state = current;
        state.diagnostic = configured_diagnostic(workflow, compiled, &fresh.pull_request);
        if !retry_request_supported_by_identity(&state, &fresh.latest_jobs) {
            state.retry_outcome = Some(CiRetryOutcome::Unsupported);
        }
        metadata.interrupted_ci_recovery = Some(state);
        if let Err(reason) = write_metadata(forge, &fresh.pull_request, &metadata).await {
            return InterruptedCiRecoveryOutcome::Retryable { reason };
        }
        return continue_recovery(forge, repository, workflow, compiled, now, address).await;
    }

    continue_recovery(forge, repository, workflow, compiled, now, address).await
}

async fn continue_recovery<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    address: ArtifactAddress,
) -> InterruptedCiRecoveryOutcome {
    // Re-read after every preceding metadata CAS and immediately before
    // deciding whether another externally visible action is permitted.
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
    let Some(mut state) = metadata.interrupted_ci_recovery.clone() else {
        return InterruptedCiRecoveryOutcome::Suppressed;
    };
    let Some(identity) = recovery_identity(repository, &fresh) else {
        return InterruptedCiRecoveryOutcome::Retryable {
            reason: "latest_current_head_fingerprint_invalid".to_string(),
        };
    };
    if !fresh.status.is_recovery_required() {
        return clear_superseded_marker(forge, fresh.pull_request, metadata).await;
    }
    if !same_identity(&state, &identity) {
        return reconcile_changed_identity(forge, fresh.pull_request, metadata, identity).await;
    }
    if requires_human_attention(&fresh.pull_request.labels) && !state.parking_barrier_installed {
        return clear_superseded_marker(forge, fresh.pull_request, metadata).await;
    }

    if let Some(diagnostic) = state.diagnostic.as_ref() {
        if diagnostic.job_id.is_some() {
            if metadata.assignment.as_ref().is_some_and(|assignment| {
                assignment.job_id.as_deref() == diagnostic.job_id.as_deref()
                    && assignment.queue.as_deref() == Some(diagnostic.queue.as_str())
                    && assignment.role.as_ref() == Some(&diagnostic.role)
                    && assignment.action.as_deref() == Some(diagnostic.action.as_str())
            }) {
                return InterruptedCiRecoveryOutcome::Waiting;
            }
            if metadata.assignment.is_some() || metadata.lease.is_some() {
                return InterruptedCiRecoveryOutcome::Waiting;
            }
            return park_interrupted_ci(forge, repository, address, &state).await;
        }
    }

    if state.retry_outcome.is_none() {
        if state.retry_started {
            let grace_elapsed = state.retry_started_at.is_none_or(|started| {
                now.signed_duration_since(started) >= RETRY_CONVERGENCE_GRACE
            });
            if !grace_elapsed {
                return InterruptedCiRecoveryOutcome::Waiting;
            }
            state.retry_outcome = Some(CiRetryOutcome::Uncertain);
            metadata.interrupted_ci_recovery = Some(state);
            return match write_metadata(forge, &fresh.pull_request, &metadata).await {
                Ok(_) => InterruptedCiRecoveryOutcome::Waiting,
                Err(reason) => InterruptedCiRecoveryOutcome::Retryable { reason },
            };
        }

        state.retry_started = true;
        state.retry_started_at = Some(now);
        metadata.interrupted_ci_recovery = Some(state.clone());
        if let Err(reason) = write_metadata(forge, &fresh.pull_request, &metadata).await {
            return InterruptedCiRecoveryOutcome::Retryable { reason };
        }

        // The marker is durable. Re-read once more immediately before the
        // provider mutation and rebuild the exact request from that snapshot.
        let action_fresh = match load_fresh_attempt(forge, repository, address).await {
            Ok(Some(fresh)) => fresh,
            Ok(None) => return InterruptedCiRecoveryOutcome::Suppressed,
            Err(reason) => return InterruptedCiRecoveryOutcome::Retryable { reason },
        };
        let action_metadata = match parse_metadata_block(&action_fresh.pull_request.body) {
            Ok(Some(metadata)) => metadata,
            _ => return InterruptedCiRecoveryOutcome::Suppressed,
        };
        let Some(action_state) = action_metadata.interrupted_ci_recovery.as_ref() else {
            return InterruptedCiRecoveryOutcome::Suppressed;
        };
        let Some(action_identity) = recovery_identity(repository, &action_fresh) else {
            return InterruptedCiRecoveryOutcome::Suppressed;
        };
        if !action_fresh.status.is_recovery_required() {
            return clear_superseded_marker(forge, action_fresh.pull_request, action_metadata)
                .await;
        }
        if !same_identity(action_state, &action_identity) {
            return reconcile_changed_identity(
                forge,
                action_fresh.pull_request,
                action_metadata,
                action_identity,
            )
            .await;
        }
        let request = match CiRetryRequest::new(
            repository.id.clone(),
            action_fresh.pull_request.id.clone(),
            action_state.head_sha.clone(),
            action_state.run_id.clone(),
            action_state.attempt.clone(),
            &action_fresh.latest_jobs,
        ) {
            Ok(request) => request,
            Err(_) => {
                return record_retry_outcome(
                    forge,
                    action_fresh.pull_request,
                    action_metadata,
                    CiRetryOutcome::Unsupported,
                )
                .await;
            }
        };
        let outcome = match forge.retry_ci_attempt(request).await {
            Ok(outcome) => outcome,
            // The trait operation itself documents transport errors as an
            // uncertain boundary. Never repeat it from a generic error.
            Err(_) => CiRetryOutcome::Uncertain,
        };
        return record_retry_outcome(forge, action_fresh.pull_request, action_metadata, outcome)
            .await;
    }

    match state.retry_outcome {
        Some(CiRetryOutcome::Accepted | CiRetryOutcome::AlreadyObserved) => {
            let grace_elapsed = state.retry_started_at.is_some_and(|started| {
                now.signed_duration_since(started) >= RETRY_CONVERGENCE_GRACE
            });
            if !grace_elapsed {
                return InterruptedCiRecoveryOutcome::Waiting;
            }
        }
        Some(
            CiRetryOutcome::Unsupported | CiRetryOutcome::Rejected(_) | CiRetryOutcome::Uncertain,
        ) => {}
        None => unreachable!("handled above"),
    }

    // Refresh a late-bound diagnostic contract. This supports adding the
    // separately configured action while a durable retry is already pending.
    if state.diagnostic.is_none() {
        state.diagnostic = configured_diagnostic(workflow, compiled, &fresh.pull_request);
        if state.diagnostic.is_some() {
            metadata.interrupted_ci_recovery = Some(state.clone());
            if let Err(reason) = write_metadata(forge, &fresh.pull_request, &metadata).await {
                return InterruptedCiRecoveryOutcome::Retryable { reason };
            }
        }
    }
    if state.diagnostic.is_some() {
        InterruptedCiRecoveryOutcome::DispatchDiagnostic
    } else {
        park_interrupted_ci(forge, repository, address, &state).await
    }
}

async fn record_retry_outcome<F: Forge + ?Sized>(
    forge: &F,
    pull_request: PullRequest,
    mut metadata: WorkflowMetadata,
    outcome: CiRetryOutcome,
) -> InterruptedCiRecoveryOutcome {
    let Some(state) = metadata.interrupted_ci_recovery.as_mut() else {
        return InterruptedCiRecoveryOutcome::Suppressed;
    };
    state.retry_outcome = Some(outcome);
    match write_metadata(forge, &pull_request, &metadata).await {
        Ok(_) => InterruptedCiRecoveryOutcome::Waiting,
        Err(reason) => InterruptedCiRecoveryOutcome::Retryable { reason },
    }
}

fn recovery_identity(
    repository: &Repository,
    fresh: &FreshAttempt,
) -> Option<InterruptedCiRecoveryState> {
    let head_sha = fresh.pull_request.head_sha.as_deref()?.trim();
    if head_sha.is_empty() || fresh.latest_jobs.is_empty() {
        return None;
    }
    let latest_jobs = CiRetryJobSetFingerprint::from_jobs(&fresh.latest_jobs).ok()?;
    let run_id = common_identity(&fresh.latest_jobs, |job| job.run_id.as_deref())
        .unwrap_or(UNKNOWN_IDENTITY)
        .to_string();
    let attempt = common_identity(&fresh.latest_jobs, |job| job.attempt.as_deref())
        .unwrap_or(UNKNOWN_IDENTITY)
        .to_string();
    Some(InterruptedCiRecoveryState {
        repository_id: repository.id.clone(),
        pull_request_id: fresh.pull_request.id.clone(),
        head_sha: head_sha.to_string(),
        run_id,
        attempt,
        latest_jobs,
        evidence: fresh.status.terminal_evidence().to_vec(),
        retry_started: false,
        retry_started_at: None,
        retry_outcome: None,
        diagnostic: None,
        parking_barrier_installed: false,
    })
}

fn common_identity<'a>(
    jobs: &'a [CiJob],
    select: impl Fn(&'a CiJob) -> Option<&'a str>,
) -> Option<&'a str> {
    let mut values = jobs.iter().map(select);
    let first = values.next().flatten()?.trim();
    if first.is_empty()
        || values.any(|value| value.map(str::trim).filter(|value| !value.is_empty()) != Some(first))
    {
        None
    } else {
        Some(first)
    }
}

fn retry_request_supported_by_identity(state: &InterruptedCiRecoveryState, jobs: &[CiJob]) -> bool {
    state.run_id != UNKNOWN_IDENTITY
        && state.attempt != UNKNOWN_IDENTITY
        && jobs.iter().all(|job| {
            job.repo_id == state.repository_id
                && job.pull_request_id.as_ref() == Some(&state.pull_request_id)
                && job.commit_sha == state.head_sha
        })
}

fn same_identity(left: &InterruptedCiRecoveryState, right: &InterruptedCiRecoveryState) -> bool {
    same_attempt_identity(left, right) && left.latest_jobs == right.latest_jobs
}

fn same_attempt_identity(
    left: &InterruptedCiRecoveryState,
    right: &InterruptedCiRecoveryState,
) -> bool {
    left.repository_id == right.repository_id
        && left.pull_request_id == right.pull_request_id
        && left.head_sha == right.head_sha
        && left.run_id == right.run_id
        && left.attempt == right.attempt
}

fn with_previous_progress(
    mut current: InterruptedCiRecoveryState,
    previous: &InterruptedCiRecoveryState,
) -> InterruptedCiRecoveryState {
    current.retry_started = previous.retry_started;
    current.retry_started_at = previous.retry_started_at;
    current.retry_outcome = previous.retry_outcome;
    current.diagnostic = previous.diagnostic.clone();
    current.parking_barrier_installed = previous.parking_barrier_installed;
    current
}

async fn reconcile_changed_identity<F: Forge + ?Sized>(
    forge: &F,
    pull_request: PullRequest,
    mut metadata: WorkflowMetadata,
    current: InterruptedCiRecoveryState,
) -> InterruptedCiRecoveryOutcome {
    let Some(previous) = metadata.interrupted_ci_recovery.as_ref() else {
        return InterruptedCiRecoveryOutcome::Suppressed;
    };
    if !same_attempt_identity(previous, &current) {
        return clear_superseded_marker(forge, pull_request, metadata).await;
    }
    if metadata.assignment.is_some() || metadata.lease.is_some() {
        return InterruptedCiRecoveryOutcome::Waiting;
    }
    metadata.interrupted_ci_recovery = Some(with_previous_progress(current, previous));
    match write_metadata(forge, &pull_request, &metadata).await {
        Ok(_) => InterruptedCiRecoveryOutcome::Waiting,
        Err(reason) => InterruptedCiRecoveryOutcome::Retryable { reason },
    }
}

async fn load_fresh_attempt<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    address: ArtifactAddress,
) -> Result<Option<FreshAttempt>, String> {
    let Some(pull_request) = forge
        .get_pull_request_by_number_with_details(
            &repository.id,
            address.number,
            ItemListDetails::summary(),
        )
        .await
        .map_err(|error| format!("pull_request_read_failed: {error}"))?
    else {
        return Ok(None);
    };
    if pull_request.state != PullRequestState::Open {
        return Ok(None);
    }
    let Some(head_sha) = pull_request
        .head_sha
        .as_deref()
        .map(str::trim)
        .filter(|head| !head.is_empty())
    else {
        return Ok(None);
    };
    let jobs = forge
        .list_ci_jobs(
            &repository.id,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                commit_sha: Some(head_sha.to_string()),
                ..CiJobQuery::default()
            },
        )
        .await
        .map_err(|error| format!("current_head_jobs_read_failed: {error}"))?;
    let latest_jobs = CiStatus::latest_jobs_for_head(&jobs, Some(head_sha));
    let status = CiStatus::from_jobs(&latest_jobs);
    Ok(Some(FreshAttempt {
        pull_request,
        latest_jobs,
        status,
    }))
}

fn configured_diagnostic(
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    pull_request: &PullRequest,
) -> Option<InterruptedCiDiagnosticState> {
    let classified = Classifier::new(workflow)
        .classify_pull_request(pull_request)
        .ok()?;
    let mut candidates = Vec::new();
    for queue in compiled.queues().iter().filter(|queue| {
        matches!(queue.condition, Some(GateCondition::CiRecoveryRequired))
            && matches_queue_cheap(*queue, &classified)
    }) {
        for action in &queue.actions {
            if action.checkout.as_deref() != Some("pull_request_read_only") {
                continue;
            }
            let Some(role) = compiled.role(&action.role) else {
                continue;
            };
            let Some(tool) = role
                .tools
                .iter()
                .find(|tool| tool.transition == action.action)
            else {
                continue;
            };
            if tool.artifact != classified.kind || tool.outcomes.is_empty() {
                continue;
            }
            candidates.push(InterruptedCiDiagnosticState {
                queue: queue.id.as_str().to_string(),
                role: RoleId::new(action.role.as_str()),
                action: action.action.as_str().to_string(),
                job_id: None,
            });
        }
    }
    match candidates.as_slice() {
        [candidate] => Some(candidate.clone()),
        _ => None,
    }
}

async fn clear_superseded_marker<F: Forge + ?Sized>(
    forge: &F,
    pull_request: PullRequest,
    mut metadata: WorkflowMetadata,
) -> InterruptedCiRecoveryOutcome {
    let Some(recovery) = metadata.interrupted_ci_recovery.take() else {
        return InterruptedCiRecoveryOutcome::Suppressed;
    };
    let body = match replace_metadata_block(&pull_request.body, &metadata) {
        Ok(body) => body,
        Err(error) => {
            return InterruptedCiRecoveryOutcome::Retryable {
                reason: format!("workflow_metadata_render_failed: {error}"),
            };
        }
    };
    match forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                body: Some(body),
                remove_labels: recovery
                    .parking_barrier_installed
                    .then(|| NEEDS_HUMAN_LABEL.to_string())
                    .into_iter()
                    .collect(),
                expected_version: Some(pull_request.version),
                ..UpdatePullRequest::default()
            },
        )
        .await
    {
        Ok(_) => InterruptedCiRecoveryOutcome::Suppressed,
        Err(error) => InterruptedCiRecoveryOutcome::Retryable {
            reason: format!("superseded_recovery_marker_clear_failed: {error}"),
        },
    }
}

async fn write_metadata<F: Forge + ?Sized>(
    forge: &F,
    pull_request: &PullRequest,
    metadata: &WorkflowMetadata,
) -> Result<PullRequest, String> {
    let body = replace_metadata_block(&pull_request.body, metadata)
        .map_err(|error| format!("workflow_metadata_render_failed: {error}"))?;
    forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                body: Some(body),
                expected_version: Some(pull_request.version),
                ..UpdatePullRequest::default()
            },
        )
        .await
        .map_err(|error| format!("workflow_metadata_write_failed: {error}"))
}

#[cfg(test)]
#[path = "interrupted_ci_recovery_tests.rs"]
mod tests;
