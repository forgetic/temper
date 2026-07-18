// SPDX-License-Identifier: MPL-2.0

//! Daemon dispatch closure and durable-assignment release transitions.

use temper_engine_io::http::{HttpResponder, HttpResponseData};
use temper_protocol_worker::Assign;

use super::machine::{DaemonMachine, DaemonRequest};
use crate::InFlightJob;

impl DaemonMachine {
    pub(super) fn begin_shutdown(
        &mut self,
        reply: temper_engine_io::OneshotSender<()>,
    ) -> Vec<DaemonRequest> {
        self.shutting_down = true;
        let waiters = std::mem::take(&mut self.waiters);
        let requests = waiters
            .into_values()
            .map(|waiter| DaemonRequest::Respond {
                responder: waiter.responder,
                response: HttpResponseData::status_only(204),
            })
            .collect();
        reply.send(());
        requests
    }

    pub(super) fn release_assignments_for_shutdown(
        &mut self,
        reply: temper_engine_io::OneshotSender<()>,
    ) -> Vec<DaemonRequest> {
        let mut assignments = Vec::new();
        for job in self.core.in_flight_jobs() {
            if let Some(context) = self.assignment_contexts.remove(&job.job_id) {
                self.core.coordinator_mut().complete(&job.job_id).ok();
                assignments.push((job, context));
            }
        }
        vec![DaemonRequest::RunShutdownRelease { assignments, reply }]
    }

    pub(super) fn rollback_shutdown_claim(
        &mut self,
        assign: Assign,
        responder: HttpResponder,
        context: crate::applier::ClaimContext,
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
            DaemonRequest::RunClaimRollback { job, context },
        ]
    }
}
