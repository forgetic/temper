// SPDX-License-Identifier: MPL-2.0

//! Assignment-transition helpers kept outside the daemon state-machine table.

use temper_engine_io::http::HttpResponder;
use temper_protocol_worker::{
    Assign, ErrorCode, ProtocolError, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use crate::InFlightJob;
use crate::applier::ClaimOutcome;

use super::machine::{ClaimAdmissionGuard, DaemonMachine, DaemonRequest};

pub(super) fn in_flight_job_from_assignment(assign: &Assign) -> InFlightJob {
    InFlightJob {
        job_id: assign.job_id.clone(),
        attempt_id: assign.attempt_id.clone(),
        role: assign.role.clone(),
        repo: assign.repo.clone(),
        artifact: assign.artifact.clone(),
        job_payload: assign.job_payload.clone(),
    }
}

pub(super) fn claim_failure_response(
    responder: HttpResponder,
    job_id: String,
    reason: String,
) -> DaemonRequest {
    DaemonRequest::Respond {
        responder,
        response: super::protocol::protocol_response(Some(WorkerProtocolMessage::Error(
            ProtocolError {
                protocol_version: WORKER_PROTOCOL_VERSION,
                code: ErrorCode::PollTimeout,
                message: format!("durable assignment claim failed: {reason}"),
                retry_after_ms: Some(100),
                job_id: Some(job_id),
            },
        ))),
    }
}

pub(super) fn new_daemon_boot_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_BOOT: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_BOOT.fetch_add(1, Ordering::Relaxed);
    let epoch_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("daemon-boot-{epoch_nanos:x}-{sequence:x}")
}

impl DaemonMachine {
    pub(super) fn handle_claim_finished(
        &mut self,
        admission: ClaimAdmissionGuard,
        assign: Assign,
        worker_id: String,
        responder: HttpResponder,
        outcome: ClaimOutcome,
    ) -> Vec<DaemonRequest> {
        let context = crate::applier::ClaimContext {
            worker_id: worker_id.clone(),
            daemon_boot_id: self.daemon_boot_id.clone(),
        };
        if !responder.is_open() {
            self.core.rollback_assignment(&assign.job_id);
            return vec![DaemonRequest::RunClaimRollback {
                job: in_flight_job_from_assignment(&assign),
                context,
                admission: Some(admission),
            }];
        }
        match outcome {
            ClaimOutcome::Claimed if self.shutdown_admission.is_fenced() => {
                self.rollback_shutdown_claim(assign, responder, context, admission)
            }
            ClaimOutcome::Claimed => {
                if self.core.commit_assignment(&assign.job_id).is_ok() {
                    self.assignment_contexts
                        .insert(assign.job_id.clone(), context.clone());
                    self.finish_claim(admission);
                    vec![
                        DaemonRequest::Log(super::protocol::assignment_log_line(
                            &assign, &worker_id,
                        )),
                        DaemonRequest::RespondAssignment {
                            job: in_flight_job_from_assignment(&assign),
                            context,
                            responder,
                            response: super::protocol::protocol_response(Some(
                                WorkerProtocolMessage::Assign(assign),
                            )),
                        },
                    ]
                } else {
                    self.core.rollback_assignment(&assign.job_id);
                    self.finish_claim(admission);
                    vec![claim_failure_response(
                        responder,
                        assign.job_id,
                        "assignment reservation became stale".to_string(),
                    )]
                }
            }
            ClaimOutcome::Stale { reason } => {
                self.core.discard_assignment_reservation(&assign.job_id);
                self.finish_claim(admission);
                vec![claim_failure_response(responder, assign.job_id, reason)]
            }
            ClaimOutcome::Contended { reason } | ClaimOutcome::Retryable { reason } => {
                self.core.rollback_assignment(&assign.job_id);
                self.finish_claim(admission);
                vec![claim_failure_response(responder, assign.job_id, reason)]
            }
        }
    }
}
