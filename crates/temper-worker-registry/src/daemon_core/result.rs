// SPDX-License-Identifier: MPL-2.0

//! Attempt-fenced result validation and exact assignment completion.

use temper_protocol_worker::{
    ErrorCode, JobResult, Release, ReleaseDisposition, WORKER_PROTOCOL_VERSION, WorkerAuth,
    WorkerProtocolMessage,
};

use super::{DaemonCore, InFlightJob, WorkerAuthError, error_response};

impl DaemonCore {
    /// Validates the protocol envelope and authenticates a result sender without
    /// consulting or consuming assignment state. The daemon machine uses this
    /// before serving pending/completed replay entries as well as before a new
    /// asynchronous application.
    pub fn authenticate_result(
        &self,
        result: &JobResult,
        auth: Option<&WorkerAuth>,
    ) -> Result<Option<WorkerProtocolMessage>, WorkerAuthError> {
        if result.protocol_version != WORKER_PROTOCOL_VERSION {
            return Ok(Some(error_response(
                ErrorCode::ProtocolVersionMismatch,
                "unsupported protocol_version",
                Some(result.job_id.clone()),
            )));
        }
        self.authenticate_registered_worker(&result.worker_id, None, auth)?;
        Ok(None)
    }

    /// Finds or restores the exact assignment fenced by `result` without
    /// consuming it. A stale attempt receives a terminal release that cannot
    /// mutate the current assignment.
    pub fn result_job(&mut self, result: &JobResult) -> Result<InFlightJob, WorkerProtocolMessage> {
        if self.coordinator.assigned_worker(&result.job_id).is_none() {
            if let Some(staged) = self.staged_recovery.get(&result.job_id).cloned() {
                if staged.attempt_id.is_some() && result.attempt_id.is_none() {
                    return Err(error_response(
                        ErrorCode::MalformedMessage,
                        "unfenced result cannot complete a fenced staged assignment",
                        Some(result.job_id.clone()),
                    ));
                }
                // The attempt fence is authoritative across worker
                // reassignment. An older worker replaying its durable result
                // must receive a compactable stale acknowledgement, not a
                // permanent wrong-worker rejection.
                if staged.attempt_id != result.attempt_id {
                    return Err(release_for_result(result, ReleaseDisposition::Superseded));
                }
                if staged.worker_id != result.worker_id {
                    return Err(error_response(
                        ErrorCode::MalformedMessage,
                        "result worker does not own the staged assignment attempt",
                        Some(result.job_id.clone()),
                    ));
                }
                if self
                    .coordinator
                    .restore_assignment(&staged.worker_id, staged.item)
                    .is_err()
                {
                    return Err(error_response(
                        ErrorCode::CapacityExceeded,
                        "staged assignment could not be restored for result application",
                        Some(result.job_id.clone()),
                    ));
                }
                self.staged_recovery.remove(&result.job_id);
                if let Some(attempt_id) = staged.attempt_id {
                    self.assignment_attempts
                        .insert(result.job_id.clone(), attempt_id);
                }
            }
        }
        let Some(worker_id) = self.coordinator.assigned_worker(&result.job_id) else {
            return Err(release_for_result(result, ReleaseDisposition::Reclaimed));
        };
        let current_attempt = self
            .assignment_attempts
            .get(&result.job_id)
            .map(String::as_str);
        if current_attempt.is_some() && result.attempt_id.is_none() {
            return Err(error_response(
                ErrorCode::MalformedMessage,
                "unfenced result cannot complete a fenced assignment",
                Some(result.job_id.clone()),
            ));
        }
        // Compare the daemon-generated fence before the worker identity. A
        // result from an older assignment commonly has both a different
        // worker and a different attempt after live reclamation; it is stale,
        // not malformed, and is safe to compact without touching the current
        // assignment.
        if current_attempt != result.attempt_id.as_deref() {
            return Err(release_for_result(result, ReleaseDisposition::Superseded));
        }
        if worker_id != result.worker_id {
            return Err(error_response(
                ErrorCode::MalformedMessage,
                "result worker does not own the current assignment attempt",
                Some(result.job_id.clone()),
            ));
        }
        self.in_flight_job(&result.job_id).ok_or_else(|| {
            error_response(
                ErrorCode::MalformedMessage,
                "assigned job is missing application context",
                Some(result.job_id.clone()),
            )
        })
    }

    /// Completes only the exact currently fenced assignment.
    pub fn complete_result(
        &mut self,
        result: &JobResult,
        disposition: ReleaseDisposition,
    ) -> WorkerProtocolMessage {
        if let Err(response) = self.result_job(result) {
            return response;
        }
        let _ = self.coordinator.complete(&result.job_id);
        self.job_context.remove(&result.job_id);
        self.assignment_attempts.remove(&result.job_id);
        release_for_result(result, disposition)
    }

    pub(super) fn handle_result(&mut self, result: JobResult) -> WorkerProtocolMessage {
        if let Err(response) = self.result_job(&result) {
            return response;
        }
        self.complete_result(&result, ReleaseDisposition::Accepted)
    }
}

fn release_for_result(
    result: &JobResult,
    disposition: ReleaseDisposition,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Release(Release {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: result.worker_id.clone(),
        job_id: result.job_id.clone(),
        attempt_id: result.attempt_id.clone(),
        disposition,
        message: None,
    })
}
