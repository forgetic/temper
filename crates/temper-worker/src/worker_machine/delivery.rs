//! Durable result delivery, acknowledgement, and replay policy.

use std::time::Duration;

use temper_protocol_worker::{ErrorCode, ReleaseDisposition, WorkerProtocolMessage};

use crate::result_outbox::ResultOutboxEntry;

use super::{WorkerMachine, WorkerRequest};

impl WorkerMachine {
    fn delivery_event(
        entry: &ResultOutboxEntry,
        delivery_state: &'static str,
        claim_convergence: &'static str,
        warning: bool,
    ) -> WorkerRequest {
        WorkerRequest::Observe(crate::observability::WorkerEvent::ResultDelivery {
            worker_id: entry.assignment.worker_id.clone(),
            job_id: entry.assignment.job_id.clone(),
            attempt_id: entry.assignment.attempt_id.clone(),
            outbox_state: "durable",
            delivery_state,
            claim_convergence,
            warning,
        })
    }

    pub(super) fn send_entry(entry: &ResultOutboxEntry) -> WorkerRequest {
        WorkerRequest::SendResult {
            entry_id: entry.entry_id.clone(),
            message: WorkerProtocolMessage::Result(entry.result.clone()),
        }
    }

    pub(super) fn retry_entry(&mut self, entry_id: &str, reason: String) -> Vec<WorkerRequest> {
        if !self.outbox.contains_key(entry_id) {
            return Vec::new();
        }
        let attempt = self
            .replay_attempts
            .entry(entry_id.to_string())
            .or_insert(0);
        *attempt = attempt.saturating_add(1);
        let exponent = attempt.saturating_sub(1).min(8);
        let delay = Duration::from_secs(2_u64.saturating_pow(exponent).min(300));
        let entry = self
            .outbox
            .get(entry_id)
            .expect("checked outbox entry exists")
            .clone();
        vec![
            Self::delivery_event(&entry, "retrying", "pending", true),
            WorkerRequest::Log(format!(
                "worker: retaining durable result entry_id={entry_id} retry={} backoff_ms={} reason={reason}",
                *attempt,
                delay.as_millis()
            )),
            WorkerRequest::ArmResultReplayTimer {
                entry_id: entry_id.to_string(),
                delay,
            },
        ]
    }

    pub(super) fn result_delivery(
        &mut self,
        entry_id: String,
        outcome: Result<Option<WorkerProtocolMessage>, String>,
    ) -> Vec<WorkerRequest> {
        let Some(entry) = self.outbox.get(&entry_id).cloned() else {
            return Vec::new();
        };
        match outcome {
            Ok(Some(WorkerProtocolMessage::Release(release)))
                if entry.matches_release(&release) =>
            {
                if matches!(
                    release.disposition,
                    ReleaseDisposition::Superseded | ReleaseDisposition::Reclaimed
                ) {
                    return vec![
                        Self::delivery_event(&entry, "acknowledged_stale", "stale", true),
                        WorkerRequest::Warn(format!(
                            "worker: durable result became stale entry_id={} job_id={} attempt_id={} disposition={:?}",
                            entry.entry_id,
                            entry.assignment.job_id,
                            entry.assignment.attempt_id,
                            release.disposition
                        )),
                        WorkerRequest::AcknowledgeResult { entry, release },
                    ];
                }
                vec![
                    Self::delivery_event(&entry, "acknowledged", "converged", false),
                    WorkerRequest::AcknowledgeResult { entry, release },
                ]
            }
            Ok(Some(WorkerProtocolMessage::Error(error)))
                if matches!(
                    error.code,
                    ErrorCode::Unauthorized
                        | ErrorCode::MalformedMessage
                        | ErrorCode::ProtocolVersionMismatch
                        | ErrorCode::RegistrationRejected
                ) =>
            {
                vec![
                    Self::delivery_event(&entry, "rejected", "unreconciled", true),
                    WorkerRequest::Warn(format!(
                        "worker: permanently rejecting durable result entry_id={} job_id={} attempt_id={} code={:?}",
                        entry.entry_id,
                        entry.assignment.job_id,
                        entry.assignment.attempt_id,
                        error.code
                    )),
                    WorkerRequest::RejectResult {
                        entry,
                        reason: format!("daemon permanently rejected result: {:?}", error.code),
                    },
                ]
            }
            Ok(Some(other)) => self.retry_entry(
                &entry_id,
                format!("unexpected daemon acknowledgement: {other:?}"),
            ),
            Ok(None) => self.retry_entry(
                &entry_id,
                "daemon returned no release acknowledgement".to_string(),
            ),
            Err(error) if permanent_transport_rejection(&error) => vec![
                Self::delivery_event(&entry, "rejected", "unreconciled", true),
                WorkerRequest::Warn(format!(
                    "worker: permanently rejecting durable result entry_id={} job_id={} attempt_id={} reason={error}",
                    entry.entry_id, entry.assignment.job_id, entry.assignment.attempt_id,
                )),
                WorkerRequest::RejectResult {
                    entry,
                    reason: error,
                },
            ],
            Err(error) => self.retry_entry(&entry_id, error),
        }
    }
}

fn permanent_transport_rejection(error: &str) -> bool {
    [
        "HTTP 400", "HTTP 401", "HTTP 403", "HTTP 404", "HTTP 409", "HTTP 422",
    ]
    .iter()
    .any(|status| error.contains(status))
}
