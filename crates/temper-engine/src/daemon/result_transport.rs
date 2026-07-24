// SPDX-License-Identifier: MPL-2.0

//! Exact-attempt result transport admission and replay handling.

use temper_engine_io::http::{HttpResponder, HttpResponseData};
use temper_protocol_worker::{
    ErrorCode, JobResult, ProtocolError, ReleaseDisposition, WORKER_PROTOCOL_VERSION, WorkerAuth,
    WorkerProtocolMessage,
};

use super::machine::{AttemptKey, DaemonMachine, DaemonRequest};
use super::protocol::{
    protocol_response, result_disposition, result_disposition_log_value, result_received_log_line,
};
use super::shutdown::AssignmentAttemptIdentity;

impl DaemonMachine {
    pub(super) fn handle_result(
        &mut self,
        mut result: JobResult,
        auth: Option<WorkerAuth>,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        let authentication = self.core.authenticate_result(&result, auth.as_ref());
        match authentication {
            Err(_) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(401),
                }];
            }
            Ok(_) if self.shutdown_admission.is_fenced() => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: HttpResponseData::status_only(503),
                }];
            }
            Ok(Some(response)) => {
                return vec![DaemonRequest::Respond {
                    responder,
                    response: protocol_response(Some(response)),
                }];
            }
            Ok(None) => {}
        }

        // The authenticated worker is still a protocol trust boundary. Keep
        // model diagnostics canonical and drop malformed or attempt-mismatched
        // session evidence before logging, replay comparison, or durability.
        result.normalize_failure_evidence();

        let key = AttemptKey::from_result(&result);

        // Result application performs its own fresh lease validation. Do not
        // race it with an unresolved heartbeat ownership check.
        if self.attempt_has_pending_ownership_check(&key) {
            return vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(503),
            }];
        }

        // Tombstones are payload-agnostic and monotonic. A terminal result from
        // the exact still-current fenced attempt proves worker quiescence and
        // releases capacity, but it never reaches the result applier.
        if let Some(release) = self.fenced_release(&result) {
            if self
                .core
                .is_current_attempt(&key.worker_id, &key.job_id, key.attempt_id.as_deref())
            {
                let disposition = match &release {
                    WorkerProtocolMessage::Release(release) => release.disposition,
                    _ => unreachable!("fenced release helper always returns release"),
                };
                let _ = self.core.complete_result(&result, disposition);
                self.assignment_contexts.remove(&key.job_id);
                self.retry_attempts.remove(&key.job_id);
                self.retry_backoff_until.remove(&key.job_id);
            }
            return vec![DaemonRequest::Respond {
                responder,
                response: protocol_response(Some(release)),
            }];
        }

        // A lost acknowledgement may replay after the exact apply completed.
        // Retain the applied payload so only a byte-for-byte equivalent result
        // receives the prior release and no Forge work runs twice.
        if let Some((applied, response)) = self.completed_results.get(&key) {
            let response = if applied == &result {
                protocol_response(Some(response.clone()))
            } else {
                protocol_response(Some(WorkerProtocolMessage::Error(ProtocolError {
                    protocol_version: WORKER_PROTOCOL_VERSION,
                    code: ErrorCode::MalformedMessage,
                    message: "different result reused a completed assignment attempt".to_string(),
                    retry_after_ms: None,
                    job_id: Some(result.job_id.clone()),
                })))
            };
            return vec![DaemonRequest::Respond {
                responder,
                response,
            }];
        }
        if let Some(pending) = self.pending_results.get(&key) {
            let response = if pending == &result {
                HttpResponseData::status_only(503)
            } else {
                HttpResponseData::status_only(422)
            };
            return vec![DaemonRequest::Respond {
                responder,
                response,
            }];
        }

        let job = match self.core.result_job(&result) {
            Err(response) => {
                let response = match &response {
                    WorkerProtocolMessage::Release(release)
                        if matches!(
                            release.disposition,
                            ReleaseDisposition::Superseded | ReleaseDisposition::Reclaimed
                        ) =>
                    {
                        let disposition = release.disposition;
                        let reason = release
                            .message
                            .clone()
                            .unwrap_or_else(|| match disposition {
                                ReleaseDisposition::Superseded => {
                                    "assignment attempt was superseded".to_string()
                                }
                                ReleaseDisposition::Reclaimed => {
                                    "assignment attempt was reclaimed".to_string()
                                }
                                ReleaseDisposition::Accepted => unreachable!(),
                            });
                        self.fence_attempt(key.clone(), reason, disposition);
                        self.fenced_release(&result).unwrap_or(response)
                    }
                    _ => response,
                };
                return vec![DaemonRequest::Respond {
                    responder,
                    response: protocol_response(Some(response)),
                }];
            }
            Ok(job) => job,
        };

        // A recovered claim keeps its prior daemon boot identity until exact
        // release. Retry every delivery through `apply_recovered`, not merely
        // the first one: a transient failure may happen before the lease
        // decorator has reattached the claim to its process-local state.
        let recovered_context = self
            .assignment_contexts
            .get(&job.job_id)
            .filter(|context| context.daemon_boot_id != self.daemon_boot_id)
            .cloned();
        let disposition = result_disposition(&result);
        let mut requests = vec![DaemonRequest::Log(result_received_log_line(
            &result,
            result_disposition_log_value(disposition),
        ))];
        if self.applying.is_empty() {
            let decisions = self.wake_coordinator.begin_apply();
            requests.extend(self.wake_decision_requests(decisions));
        }
        self.applying.insert(job.job_id.clone());
        self.pending_results.insert(key.clone(), result.clone());
        let admission = self.admit_result_application(AssignmentAttemptIdentity::from(&key));
        requests.push(DaemonRequest::RunApplyAndRespond {
            admission,
            job,
            result,
            recovered_context,
            responder,
        });
        requests
    }
}
