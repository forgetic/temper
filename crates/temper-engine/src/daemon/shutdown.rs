// SPDX-License-Identifier: MPL-2.0

//! Daemon admission fencing, admitted-operation joining, and durable-assignment
//! release transitions.

use std::collections::BTreeSet;

use temper_engine_io::http::{HttpResponder, HttpResponseData};
use temper_protocol_worker::Assign;

use super::machine::{AttemptKey, DaemonMachine, DaemonRequest};
use crate::InFlightJob;

/// Nominal tokens prevent one admitted operation kind from accidentally
/// releasing another kind's shutdown accounting.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ClaimAdmissionGuard(pub(super) u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ResultApplicationAdmissionGuard(pub(super) u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ContextAdmissionGuard(pub(super) u64);

/// The daemon's monotonic shutdown admission state.
///
/// `Fenced` is set by one serialized machine transition before shutdown begins
/// waiting on any worker or HTTP task. It is never reopened in that daemon
/// process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownAdmission {
    Open,
    Fenced,
}

impl ShutdownAdmission {
    pub(super) fn is_fenced(self) -> bool {
        matches!(self, Self::Fenced)
    }
}

/// Exact identity of one worker assignment attempt.
///
/// `attempt_id` remains optional only for the explicit split-engine legacy
/// compatibility path. Co-resident workers use a concrete attempt id.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AssignmentAttemptIdentity {
    pub worker_id: String,
    pub job_id: String,
    pub attempt_id: Option<String>,
}

impl AssignmentAttemptIdentity {
    pub fn new(
        worker_id: impl Into<String>,
        job_id: impl Into<String>,
        attempt_id: Option<String>,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            job_id: job_id.into(),
            attempt_id,
        }
    }
}

impl From<&AttemptKey> for AssignmentAttemptIdentity {
    fn from(key: &AttemptKey) -> Self {
        Self::new(
            key.worker_id.clone(),
            key.job_id.clone(),
            key.attempt_id.clone(),
        )
    }
}

/// Snapshot returned when the daemon admission fence is installed.
///
/// The sets describe work admitted before the fence. They intentionally remain
/// in the report after the work joins, so a bounded-shutdown caller can name
/// the operations it waited for. A process lost before they join leaves the
/// assignment and worker result behind the existing durable recovery paths.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DaemonShutdownReport {
    pub pending_results: BTreeSet<AssignmentAttemptIdentity>,
    pub pending_applications: BTreeSet<AssignmentAttemptIdentity>,
    pub pending_claims: BTreeSet<AssignmentAttemptIdentity>,
    pub pending_context_operations: BTreeSet<AssignmentAttemptIdentity>,
}

/// Handle for work that crossed daemon admission before the shutdown fence.
///
/// [`report`](Self::report) is immediately available. [`wait_for_join`](Self::wait_for_join)
/// resolves only after every admitted claim (including shutdown rollback),
/// result application, and assignment-scoped context read has completed. It
/// returns `false` if the daemon loop is lost instead of reporting a synthetic
/// local join.
pub struct DaemonShutdownHandle {
    report: DaemonShutdownReport,
    join_notification: temper_engine_io::OneshotReceiver<()>,
}

impl DaemonShutdownHandle {
    pub(super) fn new(
        report: DaemonShutdownReport,
        join_notification: temper_engine_io::OneshotReceiver<()>,
    ) -> Self {
        Self {
            report,
            join_notification,
        }
    }

    pub fn report(&self) -> &DaemonShutdownReport {
        &self.report
    }

    pub async fn wait_for_join(self) -> bool {
        self.join_notification.recv().await.is_some()
    }
}

impl DaemonMachine {
    pub(super) fn begin_shutdown(
        &mut self,
        reply: temper_engine_io::OneshotSender<DaemonShutdownReport>,
        joined: temper_engine_io::OneshotSender<()>,
    ) -> Vec<DaemonRequest> {
        self.shutdown_admission = ShutdownAdmission::Fenced;
        self.wake_coordinator.begin_shutdown();
        let waiters = std::mem::take(&mut self.waiters);
        let requests = waiters
            .into_values()
            .map(|waiter| DaemonRequest::Respond {
                responder: waiter.responder,
                response: HttpResponseData::status_only(204),
            })
            .collect();

        let report = DaemonShutdownReport {
            pending_results: self
                .pending_results
                .keys()
                .map(AssignmentAttemptIdentity::from)
                .collect(),
            pending_applications: self
                .admitted_result_applications
                .values()
                .cloned()
                .collect(),
            pending_claims: self.admitted_claims.values().cloned().collect(),
            pending_context_operations: self.admitted_contexts.values().cloned().collect(),
        };
        self.shutdown_join_waiters.push(joined);
        self.notify_shutdown_joined_if_ready();
        reply.send(report);
        requests
    }

    pub(super) fn admit_claim(
        &mut self,
        identity: AssignmentAttemptIdentity,
    ) -> ClaimAdmissionGuard {
        debug_assert!(!self.shutdown_admission.is_fenced());
        let guard = ClaimAdmissionGuard(self.next_token());
        self.admitted_claims.insert(guard, identity);
        guard
    }

    pub(super) fn finish_claim(&mut self, guard: ClaimAdmissionGuard) {
        self.admitted_claims.remove(&guard);
        self.notify_shutdown_joined_if_ready();
    }

    pub(super) fn admit_result_application(
        &mut self,
        identity: AssignmentAttemptIdentity,
    ) -> ResultApplicationAdmissionGuard {
        debug_assert!(!self.shutdown_admission.is_fenced());
        let guard = ResultApplicationAdmissionGuard(self.next_token());
        self.admitted_result_applications.insert(guard, identity);
        guard
    }

    pub(super) fn finish_result_application(&mut self, guard: ResultApplicationAdmissionGuard) {
        self.admitted_result_applications.remove(&guard);
        self.notify_shutdown_joined_if_ready();
    }

    pub(super) fn admit_context(
        &mut self,
        identity: AssignmentAttemptIdentity,
    ) -> ContextAdmissionGuard {
        debug_assert!(!self.shutdown_admission.is_fenced());
        let guard = ContextAdmissionGuard(self.next_token());
        self.admitted_contexts.insert(guard, identity);
        guard
    }

    pub(super) fn finish_context(&mut self, guard: ContextAdmissionGuard) {
        self.admitted_contexts.remove(&guard);
        self.notify_shutdown_joined_if_ready();
    }

    fn notify_shutdown_joined_if_ready(&mut self) {
        if !self.shutdown_admission.is_fenced()
            || !self.admitted_claims.is_empty()
            || !self.admitted_result_applications.is_empty()
            || !self.admitted_contexts.is_empty()
        {
            return;
        }
        for joined in std::mem::take(&mut self.shutdown_join_waiters) {
            joined.send(());
        }
    }

    /// Split-engine compatibility path: release every assignment still owned
    /// by this daemon boot. Standalone uses the exact-attempt API below after
    /// its co-resident worker proves which attempts joined.
    pub(super) fn release_assignments_for_shutdown(
        &mut self,
        reply: temper_engine_io::OneshotSender<()>,
    ) -> Vec<DaemonRequest> {
        let assignments = self.collect_shutdown_assignments(None);
        vec![DaemonRequest::RunShutdownRelease { assignments, reply }]
    }

    pub(super) fn release_joined_assignments_for_shutdown(
        &mut self,
        joined: BTreeSet<AssignmentAttemptIdentity>,
        reply: temper_engine_io::OneshotSender<()>,
    ) -> Vec<DaemonRequest> {
        let assignments = self.collect_shutdown_assignments(Some(&joined));
        vec![DaemonRequest::RunShutdownRelease { assignments, reply }]
    }

    fn collect_shutdown_assignments(
        &mut self,
        joined: Option<&BTreeSet<AssignmentAttemptIdentity>>,
    ) -> Vec<(InFlightJob, crate::applier::ClaimContext)> {
        let mut assignments = Vec::new();
        for job in self.core.in_flight_jobs() {
            if let Some(joined) = joined {
                let Some((worker_id, attempt_id)) =
                    self.core.current_assignment_identity(&job.job_id)
                else {
                    continue;
                };
                let identity =
                    AssignmentAttemptIdentity::new(worker_id, job.job_id.clone(), attempt_id);
                if !joined.contains(&identity) {
                    continue;
                }
            }
            if let Some(context) = self.assignment_contexts.remove(&job.job_id) {
                self.core.coordinator_mut().complete(&job.job_id).ok();
                assignments.push((job, context));
            }
        }
        assignments
    }

    pub(super) fn rollback_shutdown_claim(
        &mut self,
        assign: Assign,
        responder: HttpResponder,
        context: crate::applier::ClaimContext,
        admission: ClaimAdmissionGuard,
    ) -> Vec<DaemonRequest> {
        self.core.rollback_assignment(&assign.job_id);
        let job = InFlightJob {
            job_id: assign.job_id,
            attempt_id: assign.attempt_id,
            role: assign.role,
            repo: assign.repo,
            artifact: assign.artifact,
            job_payload: assign.job_payload,
        };
        vec![
            DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(204),
            },
            DaemonRequest::RunClaimRollback {
                job,
                context,
                admission: Some(admission),
            },
        ]
    }
}
