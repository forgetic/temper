// SPDX-License-Identifier: MPL-2.0

//! Exact-attempt ownership fences and heartbeat-completion transitions.

use std::collections::BTreeSet;

use temper_engine_io::http::{HttpResponder, HttpResponseData};
use temper_protocol_worker::{
    AttemptCancellation, CancelAttempts, JobHeartbeat, JobResult, Release, ReleaseDisposition,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use crate::InFlightJob;
use crate::applier::{RecoveredHeartbeatOutcome, RecoveredOwnershipLossReason};

use super::machine::{DaemonMachine, DaemonRequest};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct AttemptKey {
    pub(super) worker_id: String,
    pub(super) job_id: String,
    pub(super) attempt_id: Option<String>,
}

impl AttemptKey {
    pub(super) fn from_report(worker_id: &str, report: &JobHeartbeat) -> Self {
        Self {
            worker_id: worker_id.to_string(),
            job_id: report.job_id.clone(),
            attempt_id: report.attempt_id.clone(),
        }
    }

    pub(super) fn from_result(result: &JobResult) -> Self {
        Self {
            worker_id: result.worker_id.clone(),
            job_id: result.job_id.clone(),
            attempt_id: result.attempt_id.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct FencedAttempt {
    pub(super) reason: String,
    pub(super) disposition: ReleaseDisposition,
}

pub(super) struct RecoveredHeartbeatCheck {
    pub(super) key: AttemptKey,
    pub(super) job: InFlightJob,
    pub(super) context: crate::applier::ClaimContext,
}

impl DaemonMachine {
    pub(super) fn begin_ownership_check(&mut self, key: &AttemptKey) {
        let pending = self
            .pending_ownership_checks
            .entry(key.clone())
            .or_default();
        *pending = pending.saturating_add(1);
    }

    fn finish_ownership_check(&mut self, key: &AttemptKey) {
        let Some(pending) = self.pending_ownership_checks.get_mut(key) else {
            return;
        };
        if *pending > 1 {
            *pending -= 1;
        } else {
            self.pending_ownership_checks.remove(key);
        }
    }

    pub(super) fn attempt_has_pending_ownership_check(&self, key: &AttemptKey) -> bool {
        self.pending_ownership_checks.contains_key(key)
    }

    pub(super) fn attempt_is_fenced(&self, key: &AttemptKey) -> bool {
        self.fenced_attempts.contains_key(key)
    }

    pub(super) fn attempt_can_read_context(&self, key: &AttemptKey) -> bool {
        !self.attempt_is_fenced(key)
            && !self.attempt_has_pending_ownership_check(key)
            && self
                .core
                .is_current_attempt(&key.worker_id, &key.job_id, key.attempt_id.as_deref())
    }

    pub(super) fn fence_rejected_report(&mut self, key: AttemptKey) {
        let disposition = match self.core.current_assignment_identity(&key.job_id) {
            Some((worker_id, attempt_id))
                if worker_id != key.worker_id || attempt_id != key.attempt_id =>
            {
                ReleaseDisposition::Superseded
            }
            _ => ReleaseDisposition::Reclaimed,
        };
        self.fence_attempt(
            key,
            "daemon does not own this exact assignment attempt".to_string(),
            disposition,
        );
    }

    pub(super) fn fence_attempt(
        &mut self,
        key: AttemptKey,
        reason: String,
        disposition: ReleaseDisposition,
    ) {
        self.fenced_attempts.entry(key).or_insert(FencedAttempt {
            reason: bounded_cancellation_reason(reason),
            disposition,
        });
    }

    pub(super) fn fenced_release(&self, result: &JobResult) -> Option<WorkerProtocolMessage> {
        let key = AttemptKey::from_result(result);
        let fenced = self.fenced_attempts.get(&key)?;
        Some(WorkerProtocolMessage::Release(Release {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: result.worker_id.clone(),
            job_id: result.job_id.clone(),
            attempt_id: result.attempt_id.clone(),
            disposition: fenced.disposition,
            message: Some(fenced.reason.clone()),
        }))
    }

    fn heartbeat_response(
        &self,
        worker_id: &str,
        reports: &[AttemptKey],
        fallback: HttpResponseData,
    ) -> HttpResponseData {
        let mut seen = BTreeSet::new();
        let mut cancellations = Vec::new();
        for key in reports {
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(fenced) = self.fenced_attempts.get(key) else {
                continue;
            };
            let Some(attempt_id) = key.attempt_id.as_deref() else {
                // Legacy unfenced peers cannot consume the additive exact-attempt
                // cancellation protocol. Their tombstone still fences all daemon
                // effects and stale result delivery.
                continue;
            };
            if let Ok(cancellation) = AttemptCancellation::ownership_lost(
                worker_id,
                &key.job_id,
                attempt_id,
                &fenced.reason,
            ) {
                cancellations.push(cancellation);
            }
        }
        if cancellations.is_empty() {
            return fallback;
        }
        match CancelAttempts::new(worker_id, cancellations) {
            Ok(directive) => super::protocol::protocol_response(Some(
                WorkerProtocolMessage::CancelAttempts(directive),
            )),
            Err(_) => fallback,
        }
    }

    pub(super) fn finish_recovered_heartbeats(
        &mut self,
        worker_id: String,
        reports: Vec<AttemptKey>,
        outcomes: Vec<(AttemptKey, RecoveredHeartbeatOutcome)>,
        responder: HttpResponder,
        response: HttpResponseData,
    ) -> Vec<DaemonRequest> {
        for (key, outcome) in outcomes {
            self.finish_ownership_check(&key);
            if self.attempt_is_fenced(&key) {
                continue;
            }
            if let RecoveredHeartbeatOutcome::OwnershipLost { reason } = outcome {
                let disposition =
                    if matches!(reason, RecoveredOwnershipLossReason::AssignmentReplaced) {
                        ReleaseDisposition::Superseded
                    } else {
                        ReleaseDisposition::Reclaimed
                    };
                self.fence_attempt(key, reason.to_string(), disposition);
            }
        }
        vec![DaemonRequest::Respond {
            responder,
            response: self.heartbeat_response(&worker_id, &reports, response),
        }]
    }

    pub(super) fn immediate_heartbeat_response(
        &self,
        worker_id: &str,
        reports: &[AttemptKey],
        responder: HttpResponder,
        response: HttpResponseData,
    ) -> Vec<DaemonRequest> {
        vec![DaemonRequest::Respond {
            responder,
            response: self.heartbeat_response(worker_id, reports, response),
        }]
    }
}

fn bounded_cancellation_reason(reason: String) -> String {
    const LIMIT: usize = temper_protocol_worker::MAX_ATTEMPT_CANCELLATION_REASON_BYTES;
    let reason = if reason.trim().is_empty() {
        "recovered assignment ownership was lost".to_string()
    } else {
        reason
    };
    if reason.len() <= LIMIT {
        return reason;
    }
    let mut end = LIMIT;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}
