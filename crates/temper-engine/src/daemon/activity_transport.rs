// SPDX-License-Identifier: MPL-2.0

//! Authentication and journal dispatch for worker activity batches.

use temper_engine_io::http::{HttpResponder, HttpResponseData};
use temper_protocol_worker::{WORKER_PROTOCOL_VERSION, WorkerActivityBatch, WorkerAuth};

use super::machine::{DaemonMachine, DaemonRequest};

impl DaemonMachine {
    pub(super) fn handle_activity_batch(
        &self,
        mut request: WorkerActivityBatch,
        auth: Option<WorkerAuth>,
        trusted_transport: bool,
        responder: HttpResponder,
    ) -> Vec<DaemonRequest> {
        // The transport DTO is untrusted even after worker authentication.
        // Normalize diagnostics before protocol validation so unsafe detail can
        // never reach dispatch, logging, or the journal representation.
        for event in &mut request.batch.events {
            event.event.sanitize_untrusted_activity();
        }
        if request.protocol_version != WORKER_PROTOCOL_VERSION
            || request.worker_id.trim().is_empty()
            || request.assignment_id.trim().is_empty()
            || request.batch.validate().is_err()
            || request.capture_policy.validate().is_err()
        {
            return bad_request(responder);
        }
        let Some(first) = request.batch.events.first() else {
            return bad_request(responder);
        };
        if self
            .core
            .authorize_activity_worker(
                &request.worker_id,
                &first.assignment.role,
                &first.assignment.repository,
                auth.as_ref(),
                trusted_transport,
            )
            .is_err()
        {
            return vec![DaemonRequest::Respond {
                responder,
                response: HttpResponseData::status_only(401),
            }];
        }
        if request.assignment_id != first.assignment.job_id {
            return bad_request(responder);
        }
        let binding = crate::AuthenticatedWorkerBinding {
            worker_id: request.worker_id.clone(),
            assignment_id: request.assignment_id.clone(),
            assignment: first.assignment.clone(),
            agent_session_id: first.agent_session_id.clone(),
            capture_policy: request.capture_policy.clone(),
        };
        vec![DaemonRequest::IngestActivity {
            request,
            binding,
            responder,
        }]
    }
}

fn bad_request(responder: HttpResponder) -> Vec<DaemonRequest> {
    vec![DaemonRequest::Respond {
        responder,
        response: HttpResponseData::status_only(400),
    }]
}
