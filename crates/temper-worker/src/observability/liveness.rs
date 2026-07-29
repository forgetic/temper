//! Structured privacy-safe worker lifecycle, result, and capacity events.

use temper_protocol_activity::ModelFailureV1;
use temper_protocol_worker::{JobResult, SessionRecoveryActionV1, SessionRecoveryEvidenceV1};

use crate::worker_machine::ActiveOperation;

/// Privacy-safe operation fields used by worker liveness events. The type has
/// no place for arguments, prompts, results, or credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedOperation {
    pub kind: &'static str,
    pub name: String,
    pub operation_id: String,
    pub elapsed_ms: u64,
}

impl ObservedOperation {
    pub fn from_active(operation: &ActiveOperation, now_ns: u64) -> Self {
        Self {
            kind: match operation.kind {
                crate::worker_machine::OperationKind::Model => "model",
                crate::worker_machine::OperationKind::Tool => "tool",
            },
            name: operation.name.clone(),
            operation_id: operation.id.clone(),
            elapsed_ms: now_ns.saturating_sub(operation.started_at.as_nanos()) / 1_000_000,
        }
    }
}

/// Typed worker event request emitted by the sans-I/O machine and rendered by
/// the shell. Fields are closed and privacy-safe: operational events remain
/// content-free, while model-recovery events carry only normalized bounded
/// diagnostics and validated durable identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerEvent {
    JobProgress {
        worker_id: String,
        job_id: String,
        attempt_id: String,
        phase: &'static str,
        run_elapsed_ms: u64,
        last_progress_elapsed_ms: u64,
        no_progress_elapsed_ms: u64,
        active_parallel_operation_count: u32,
        operation: Option<ObservedOperation>,
    },
    JobTimeout {
        worker_id: String,
        job_id: String,
        attempt_id: String,
        phase: &'static str,
        reason: &'static str,
        limit_ms: u64,
        run_elapsed_ms: u64,
        last_progress_elapsed_ms: u64,
        no_progress_elapsed_ms: u64,
        active_parallel_operation_count: u32,
        operation: Option<ObservedOperation>,
    },
    CancellationRequested {
        worker_id: String,
        job_id: String,
        attempt_id: String,
        reason: &'static str,
        limit_ms: u64,
    },
    CancellationCompleted {
        worker_id: String,
        job_id: String,
        attempt_id: String,
        outcome: String,
        descendant_cleanup: String,
        forced: bool,
    },
    SessionRotated {
        worker_id: String,
        job_id: String,
        model_failure: ModelFailureV1,
        session_recovery: SessionRecoveryEvidenceV1,
    },
    ResultRecorded {
        worker_id: String,
        job_id: String,
        attempt_id: String,
        outbox_state: &'static str,
        delivery_state: &'static str,
        success: bool,
    },
    ResultDelivery {
        worker_id: String,
        job_id: String,
        attempt_id: String,
        outbox_state: &'static str,
        delivery_state: &'static str,
        claim_convergence: &'static str,
        warning: bool,
    },
    CapacityReleased {
        worker_id: String,
        job_id: String,
        attempt_id: String,
        permit_released: bool,
        free_capacity: u32,
    },
}

impl WorkerEvent {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::JobProgress { .. } => "worker.job.progress",
            Self::JobTimeout { .. } => "worker.job.timeout",
            Self::CancellationRequested { .. } => "worker.job.cancellation_requested",
            Self::CancellationCompleted { .. } => "worker.job.cancellation_completed",
            Self::SessionRotated { .. } => "model.session.rotated",
            Self::ResultRecorded { .. } => "worker.result.recorded",
            Self::ResultDelivery { .. } => "worker.result.delivery",
            Self::CapacityReleased { .. } => "worker.capacity.released",
        }
    }

    /// Builds the catalog event only from typed evidence in a durable result.
    /// Optional activity capture and child stderr are deliberately not inputs.
    pub fn session_rotated(result: &JobResult) -> Option<Self> {
        let failure = result.failure.as_ref()?;
        let recovery = failure.session_recovery.as_ref()?;
        if recovery.action != SessionRecoveryActionV1::RotateSession
            || recovery
                .validate_for_attempt(result.attempt_id.as_deref())
                .is_err()
        {
            return None;
        }
        let mut model_failure = failure.model_failure.clone()?;
        model_failure.normalize();
        Some(Self::SessionRotated {
            worker_id: result.worker_id.clone(),
            job_id: result.job_id.clone(),
            model_failure,
            session_recovery: recovery.clone(),
        })
    }

    pub fn emit(&self) {
        match self {
            Self::JobProgress {
                worker_id,
                job_id,
                attempt_id,
                phase,
                run_elapsed_ms,
                last_progress_elapsed_ms,
                no_progress_elapsed_ms,
                active_parallel_operation_count,
                operation,
            } => {
                let (kind, name, id, elapsed) = operation_fields(operation.as_ref());
                tracing::debug!(
                    target: "temper::worker",
                    service = "worker",
                    event = "worker.job.progress",
                    worker_id,
                    job_id,
                    attempt_id,
                    phase,
                    operation_kind = kind,
                    operation_name = name,
                    operation_id = id,
                    operation_elapsed_ms = elapsed,
                    run_elapsed_ms,
                    last_progress_elapsed_ms,
                    no_progress_elapsed_ms,
                    active_parallel_operation_count,
                    "worker accepted agent lifecycle progress"
                );
            }
            Self::JobTimeout {
                worker_id,
                job_id,
                attempt_id,
                phase,
                reason,
                limit_ms,
                run_elapsed_ms,
                last_progress_elapsed_ms,
                no_progress_elapsed_ms,
                active_parallel_operation_count,
                operation,
            } => {
                let (kind, name, id, elapsed) = operation_fields(operation.as_ref());
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "worker.job.timeout",
                    worker_id,
                    job_id,
                    attempt_id,
                    phase,
                    timeout_reason = reason,
                    timeout_limit_ms = limit_ms,
                    operation_kind = kind,
                    operation_name = name,
                    operation_id = id,
                    operation_elapsed_ms = elapsed,
                    run_elapsed_ms,
                    last_progress_elapsed_ms,
                    no_progress_elapsed_ms,
                    active_parallel_operation_count,
                    "worker job exceeded its liveness deadline"
                );
            }
            Self::CancellationRequested {
                worker_id,
                job_id,
                attempt_id,
                reason,
                limit_ms,
            } => tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "worker.job.cancellation_requested",
                worker_id,
                job_id,
                attempt_id,
                timeout_reason = reason,
                timeout_limit_ms = limit_ms,
                "worker requested cancellation for a timed-out job"
            ),
            Self::CancellationCompleted {
                worker_id,
                job_id,
                attempt_id,
                outcome,
                descendant_cleanup,
                forced,
            } => tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "worker.job.cancellation_completed",
                worker_id,
                job_id,
                attempt_id,
                cancellation_outcome = outcome,
                descendant_cleanup,
                forced,
                "worker cancellation quiesced all attempt-owned resources"
            ),
            Self::SessionRotated {
                worker_id,
                job_id,
                model_failure,
                session_recovery,
            } => {
                let status = model_failure.http_status.map(u64::from);
                tracing::info!(
                    target: "temper::worker",
                    service = "worker",
                    event = "model.session.rotated",
                    worker_id,
                    job_id,
                    attempt_id = session_recovery.attempt_id.as_str(),
                    failure_epoch = session_recovery.failure_epoch,
                    failure_count = session_recovery.failure_count,
                    cumulative_failure_count = session_recovery.failure_count,
                    session_number = session_recovery.session_number,
                    session_failure_count = session_recovery.session_failure_count,
                    elapsed_ms = session_recovery.epoch_elapsed_ms,
                    action = session_recovery.action.as_str(),
                    deferral_count = session_recovery.deferral_count,
                    generation = session_recovery.deferral_generation,
                    current_session_id = session_recovery.current_session_id.as_str(),
                    prior_session_id = session_recovery.prior_session_id.as_deref(),
                    new_session_id = session_recovery.new_session_id.as_deref(),
                    evidence_location = session_recovery.evidence_location.as_str(),
                    provider = model_failure.provider.as_str(),
                    model = model_failure.model.as_str(),
                    category = model_failure.category.as_str(),
                    disposition = model_failure.disposition.as_str(),
                    final_disposition = model_failure.disposition.as_str(),
                    boundary = model_failure.boundary.as_str(),
                    event_kind = model_failure.event_kind.as_str(),
                    status_present = model_failure.status_present,
                    code_present = model_failure.code_present,
                    retryable = model_failure.retryable,
                    http_status = status,
                    provider_request_id = model_failure.provider_request_id.as_deref(),
                    provider_error_code = model_failure.provider_error_code.as_deref(),
                    detail_redacted = model_failure.detail_redacted,
                    "worker:  durable model failure rotated the agent session"
                );
            }
            Self::ResultRecorded {
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                success: true,
            } => tracing::debug!(
                target: "temper::worker",
                service = "worker",
                event = "worker.result.recorded",
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                "worker durably recorded a terminal result"
            ),
            Self::ResultRecorded {
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                success: false,
            } => tracing::error!(
                target: "temper::worker",
                service = "worker",
                event = "worker.result.recorded",
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                "worker could not durably record a terminal result"
            ),
            Self::ResultDelivery {
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                claim_convergence,
                warning: false,
            } => tracing::debug!(
                target: "temper::worker",
                service = "worker",
                event = "worker.result.delivery",
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                claim_convergence,
                "worker result delivery advanced"
            ),
            Self::ResultDelivery {
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                claim_convergence,
                warning: true,
            } => tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "worker.result.delivery",
                worker_id,
                job_id,
                attempt_id,
                outbox_state,
                delivery_state,
                claim_convergence,
                "worker result delivery requires retry or operator attention"
            ),
            Self::CapacityReleased {
                worker_id,
                job_id,
                attempt_id,
                permit_released,
                free_capacity,
            } => tracing::debug!(
                target: "temper::worker",
                service = "worker",
                event = "worker.capacity.released",
                worker_id,
                job_id,
                attempt_id,
                permit_released,
                free_capacity,
                "worker released job capacity after durable terminal recording"
            ),
        }
    }
}

fn operation_fields(operation: Option<&ObservedOperation>) -> (&str, &str, &str, u64) {
    operation.map_or(("none", "none", "none", 0), |operation| {
        (
            operation.kind,
            operation.name.as_str(),
            operation.operation_id.as_str(),
            operation.elapsed_ms,
        )
    })
}
