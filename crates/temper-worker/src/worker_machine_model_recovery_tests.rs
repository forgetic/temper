// SPDX-License-Identifier: MPL-2.0

//! Durable model-recovery observability at the worker machine/outbox boundary.

use temper_protocol_activity::{ModelFailureCategoryV1, ModelFailureV1};
use temper_protocol_worker::{
    FailureClass, ResultStatus, SessionRecoveryActionV1, SessionRecoveryEvidenceV1,
    WorkerProtocolMessage,
};
use temper_worker_io::{EngineTime, Machine};

use super::tests::{assign, params, run};
use super::{AttemptCompletion, JobCleanup, WorkerCompletion, WorkerMachine, WorkerRequest};
use crate::executor::{JobOutcome, job_result_for_attempt};
use crate::result_outbox::ResultOutboxEntry;

#[test]
fn rotation_event_is_requested_only_after_the_result_is_durable() {
    let mut machine = WorkerMachine::new(params());
    machine.on_start(EngineTime::ZERO);
    run(
        &mut machine,
        vec![
            WorkerCompletion::Registered(Ok(())),
            WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign("job-1"))))),
        ],
    );
    let result = job_result_for_attempt(
        "worker-1",
        "job-1",
        Some("attempt-job-1".to_string()),
        JobOutcome::Failure {
            class: FailureClass::Transient,
            message: "model session rotated".to_string(),
            model_failure: Some(ModelFailureV1 {
                provider: "fixture-provider".to_string(),
                model: "fixture-model".to_string(),
                category: ModelFailureCategoryV1::Provider,
                disposition: temper_protocol_activity::ModelFailureDispositionV1::Retryable,
                boundary: temper_protocol_activity::ModelFailureBoundaryV1::Http,
                event_kind: temper_protocol_activity::ModelFailureEventKindV1::HttpResponse,
                status_present: true,
                code_present: true,
                retryable: false,
                http_status: Some(503),
                provider_request_id: Some("request-750".to_string()),
                provider_error_code: Some("unavailable".to_string()),
                message: "Provider is unavailable.".to_string(),
                detail_redacted: false,
            }),
            session_recovery: Some(SessionRecoveryEvidenceV1 {
                attempt_id: "attempt-job-1".to_string(),
                failure_epoch: 1,
                failure_count: 1,
                session_number: 0,
                session_failure_count: 0,
                epoch_started_unix_ms: None,
                epoch_elapsed_ms: 0,
                disposition: None,
                immediate_retry_exhausted: false,
                configured_session_failure_limit: 0,
                configured_fresh_session_limit: 0,
                configured_deferral_limit: 0,
                deferral_count: 0,
                deferral_generation: 0,
                not_before_unix_ms: None,
                slo_deadline_unix_ms: None,
                action: SessionRecoveryActionV1::RotateSession,
                current_session_id: "session-prior".to_string(),
                prior_session_id: None,
                new_session_id: Some("session-fresh".to_string()),
                evidence_location: ".temper-agent-session/state.json".to_string(),
            }),
        },
    );
    let before_durable = run(
        &mut machine,
        vec![WorkerCompletion::AttemptQuiesced {
            job_id: "job-1".to_string(),
            attempt_id: "attempt-job-1".to_string(),
            generation: 1,
            completion: AttemptCompletion {
                result: Some(result.clone()),
                cleanup: JobCleanup::no_process(None),
            },
        }],
    );
    assert!(before_durable.iter().all(|request| !matches!(
        request,
        WorkerRequest::Observe(crate::observability::WorkerEvent::SessionRotated { .. })
    )));

    let entry = ResultOutboxEntry::from_result(result).unwrap();
    let after_durable = run(
        &mut machine,
        vec![WorkerCompletion::ResultRecorded {
            job_id: "job-1".to_string(),
            attempt_id: "attempt-job-1".to_string(),
            generation: 1,
            outcome: Ok(entry),
        }],
    );
    assert!(after_durable.iter().any(|request| matches!(
        request,
        WorkerRequest::Observe(crate::observability::WorkerEvent::SessionRotated {
            job_id,
            session_recovery,
            ..
        }) if job_id == "job-1" && session_recovery.action == SessionRecoveryActionV1::RotateSession
    )));
    assert!(after_durable.iter().any(|request| matches!(
        request,
        WorkerRequest::SendResult {
            message: WorkerProtocolMessage::Result(result),
            ..
        } if result.status == ResultStatus::Failure
    )));
}
