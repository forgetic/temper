// SPDX-License-Identifier: MPL-2.0

//! Assignment-transition helpers kept outside the daemon state-machine table.

use temper_engine_io::http::HttpResponder;
use temper_protocol_worker::{
    Assign, ErrorCode, ProtocolError, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use crate::InFlightJob;

use super::machine::DaemonRequest;

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
