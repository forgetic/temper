// SPDX-License-Identifier: MPL-2.0

//! Result-application completion transitions for the daemon machine.

use temper_engine_io::http::{HttpResponder, HttpResponseData};
use temper_protocol_worker::{JobResult, ReleaseDisposition, WorkerProtocolMessage};

use super::machine::{AttemptKey, DaemonMachine, DaemonRequest, retry_delay};
use crate::applier::ApplyOutcome;

impl DaemonMachine {
    fn remember_completed_result(
        &mut self,
        key: AttemptKey,
        result: JobResult,
        response: WorkerProtocolMessage,
    ) {
        const COMPLETED_RESULT_LIMIT: usize = 4096;
        if self.completed_results.len() >= COMPLETED_RESULT_LIMIT {
            if let Some(oldest) = self.completed_results.keys().next().cloned() {
                self.completed_results.remove(&oldest);
            }
        }
        self.completed_results.insert(key, (result, response));
    }

    #[cfg(test)]
    pub(super) fn handle_apply_finished(
        &mut self,
        job_id: String,
        outcome: ApplyOutcome,
    ) -> Vec<DaemonRequest> {
        self.applying.remove(&job_id);
        let mut requests = match outcome {
            ApplyOutcome::Retryable { reason } => {
                let attempt = self.retry_attempts.entry(job_id.clone()).or_insert(0);
                *attempt = attempt.saturating_add(1);
                let delay = retry_delay(*attempt);
                self.retry_backoff_until
                    .insert(job_id.clone(), self.now + delay);
                vec![DaemonRequest::Log(format!(
                    "engine: result apply retry scheduled job_id={job_id} attempt={} backoff_ms={} reason={reason}",
                    *attempt,
                    delay.as_millis()
                ))]
            }
            ApplyOutcome::Applied
            | ApplyOutcome::RetryReleased
            | ApplyOutcome::Stale
            | ApplyOutcome::Rejected { .. } => {
                self.retry_attempts.remove(&job_id);
                self.retry_backoff_until.remove(&job_id);
                self.recently_applied
                    .insert(job_id, self.now + self.apply_grace);
                Vec::new()
            }
        };
        if self.applying.is_empty() {
            let decisions = self.wake_coordinator.promote_apply_deferred();
            requests.extend(self.wake_decision_requests(decisions));
        }
        requests
    }

    pub(super) fn handle_apply_and_respond_finished(
        &mut self,
        result: JobResult,
        responder: HttpResponder,
        outcome: ApplyOutcome,
    ) -> Vec<DaemonRequest> {
        let job_id = result.job_id.clone();
        let key = AttemptKey::from_result(&result);
        self.applying.remove(&job_id);
        self.pending_results.remove(&key);

        let mut requests = Vec::new();
        match outcome {
            ApplyOutcome::Applied | ApplyOutcome::RetryReleased => {
                let release = self
                    .core
                    .complete_result(&result, ReleaseDisposition::Accepted);
                self.assignment_contexts.remove(&job_id);
                self.retry_attempts.remove(&job_id);
                self.retry_backoff_until.remove(&job_id);
                self.recently_applied
                    .insert(job_id.clone(), self.now + self.apply_grace);
                self.remember_completed_result(key, result, release.clone());
                requests.push(DaemonRequest::Respond {
                    responder,
                    response: super::protocol::protocol_response(Some(release)),
                });
            }
            ApplyOutcome::Stale => {
                // The applier found no durable authority for the exact attempt
                // that is still current in this daemon. No newer daemon attempt
                // exists, so this is reclaimed rather than superseded. Results
                // rejected against an actually newer attempt are classified in
                // `result_job` before application starts.
                self.fence_attempt(
                    key.clone(),
                    "result application found the assignment attempt reclaimed".to_string(),
                    ReleaseDisposition::Reclaimed,
                );
                let _ = self
                    .core
                    .complete_result(&result, ReleaseDisposition::Reclaimed);
                let release = self
                    .fenced_release(&result)
                    .expect("stale application creates an exact-attempt tombstone");
                self.assignment_contexts.remove(&job_id);
                self.retry_attempts.remove(&job_id);
                self.retry_backoff_until.remove(&job_id);
                self.recently_applied
                    .insert(job_id.clone(), self.now + self.apply_grace);
                requests.push(DaemonRequest::Respond {
                    responder,
                    response: super::protocol::protocol_response(Some(release)),
                });
            }
            ApplyOutcome::Retryable { reason } => {
                let attempt = self.retry_attempts.entry(job_id.clone()).or_insert(0);
                *attempt = attempt.saturating_add(1);
                let delay = retry_delay(*attempt);
                requests.push(DaemonRequest::Log(format!(
                    "engine: result apply remains unacknowledged job_id={job_id} attempt={} backoff_ms={} reason={reason}",
                    *attempt,
                    delay.as_millis()
                )));
                requests.push(DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(503),
                });
            }
            ApplyOutcome::Rejected { class, reason } => {
                // The applier durably parked the deterministic failure; release
                // daemon capacity but reject outbox compaction as accepted.
                let _ = self
                    .core
                    .complete_result(&result, ReleaseDisposition::Accepted);
                self.assignment_contexts.remove(&job_id);
                requests.push(DaemonRequest::Log(format!(
                    "engine: result permanently rejected job_id={job_id} class={class:?} reason={reason}"
                )));
                requests.push(DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(422),
                });
            }
        }
        if self.applying.is_empty() {
            let decisions = self.wake_coordinator.promote_apply_deferred();
            requests.extend(self.wake_decision_requests(decisions));
        }
        requests
    }
}
