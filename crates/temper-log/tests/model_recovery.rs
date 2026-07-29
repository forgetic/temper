// SPDX-License-Identifier: MPL-2.0

use std::io;
use std::sync::{Arc, Mutex};

use temper_log::WorkItemRef;
use temper_log::emit::{
    ModelFailureParked, ModelProviderDeferred, ModelProviderWake, ModelRecoveryCleared,
    ModelRecoveryDecision, ModelSessionRotated, ModelTurnRetrying, emit_model_failure_parked,
    emit_model_provider_deferred, emit_model_provider_wake, emit_model_recovery_cleared,
    emit_model_session_rotated, emit_model_turn_retrying,
};
use temper_protocol_activity::{ModelFailureCategoryV1, ModelFailureV1};
use tracing::subscriber::with_default;
use tracing_subscriber::fmt::MakeWriter;

#[test]
fn model_recovery_catalog_projects_the_same_safe_typed_fields() {
    let buffer = SharedBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let failure = ModelFailureV1 {
        provider: "fixture-provider".into(),
        model: "fixture-model".into(),
        category: ModelFailureCategoryV1::Provider,
        disposition: temper_protocol_activity::ModelFailureDispositionV1::Retryable,
        boundary: temper_protocol_activity::ModelFailureBoundaryV1::Http,
        event_kind: temper_protocol_activity::ModelFailureEventKindV1::HttpResponse,
        status_present: true,
        code_present: true,
        retryable: true,
        http_status: Some(503),
        provider_request_id: Some("request-750".into()),
        provider_error_code: Some("unavailable".into()),
        message: "Provider is unavailable.".into(),
        detail_redacted: false,
    };

    let item = WorkItemRef::issue("ai/temper", 750);
    with_default(subscriber, || {
        emit_model_turn_retrying(ModelTurnRetrying {
            scope: "main",
            scope_id: "scope-1",
            call_id: "model-call-1",
            attempt: 1,
            next_attempt: 2,
            delay_ms: 500,
            duration_ms: 120,
            model_failure: &failure,
        });
        emit_model_session_rotated(ModelSessionRotated {
            worker_id: "worker-1",
            job_id: "job-750",
            decision: decision(
                &failure,
                "attempt-rotation",
                "rotate_session",
                "session-prior",
                None,
                Some("session-fresh"),
            ),
        });
        emit_model_provider_deferred(ModelProviderDeferred {
            item: &item,
            worker_id: "worker-1",
            job_id: "job-750",
            workstream_id: "workstream-750",
            decision: decision(
                &failure,
                "attempt-deferred",
                "provider_deferred",
                "session-fresh",
                Some("session-prior"),
                None,
            ),
        });
        emit_model_provider_wake(ModelProviderWake {
            item: &item,
            workstream_id: "workstream-750",
            failure_epoch: 2,
            failure_count: 3,
            elapsed_ms: 45_000,
            deferral_count: 1,
            generation: 2,
            action: "provider_health_wake",
            event_id: "provider-healthy-1",
            disposition: "retryable",
            provider: "fixture-provider",
            model: "fixture-model",
            category: "provider",
            boundary: "http",
            event_kind: "http_response",
            status_present: true,
            code_present: true,
            http_status: Some(503),
            provider_request_id: Some("request-750"),
            provider_error_code: Some("unavailable"),
        });
        emit_model_recovery_cleared(ModelRecoveryCleared {
            item: &item,
            workstream_id: "workstream-750",
            failure_epoch: 2,
            failure_count: 3,
            elapsed_ms: 46_000,
            generation: 2,
        });
        emit_model_failure_parked(ModelFailureParked {
            item: &item,
            worker_id: "worker-1",
            job_id: "job-750",
            decision: decision(
                &failure,
                "attempt-park",
                "park_for_human",
                "session-fresh",
                Some("session-prior"),
                None,
            ),
        });
    });

    let records: Vec<serde_json::Value> = String::from_utf8(buffer.bytes())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 6);
    let retry = &records[0]["fields"];
    let rotation = &records[1]["fields"];
    let deferred = &records[2]["fields"];
    let wake = &records[3]["fields"];
    let cleared = &records[4]["fields"];
    let parked = &records[5]["fields"];
    for fields in [rotation, deferred, parked] {
        assert_eq!(fields["provider"], "fixture-provider");
        assert_eq!(fields["model"], "fixture-model");
        assert_eq!(fields["category"], "provider");
        assert_eq!(fields["retryable"], true);
        assert_eq!(fields["http_status"], 503);
        assert_eq!(fields["provider_request_id"], "request-750");
        assert_eq!(fields["provider_error_code"], "unavailable");
        assert_eq!(
            fields["evidence_location"],
            ".temper-agent-session/state.json"
        );
        assert_eq!(fields["disposition"], "retryable");
        assert_eq!(fields["boundary"], "http");
        assert_eq!(fields["event_kind"], "http_response");
        assert_eq!(fields["status_present"], true);
        assert_eq!(fields["code_present"], true);
        assert_eq!(fields["session_number"], 2);
        assert_eq!(fields["session_failure_count"], 1);
        assert_eq!(fields["cumulative_failure_count"], 3);
        assert_eq!(fields["generation"], 2);
        assert!(fields["model_failure_message"].is_null());
    }
    assert_eq!(retry["event"], "model.turn.retrying");
    assert_eq!(retry["attempt"], 1);
    assert_eq!(retry["next_attempt"], 2);
    assert_eq!(retry["disposition"], "retryable");
    assert_eq!(retry["provider_request_id"], "request-750");
    assert_eq!(rotation["event"], "model.session.rotated");
    assert_eq!(rotation["action"], "rotate_session");
    assert_eq!(rotation["current_session_id"], "session-prior");
    assert_eq!(rotation["new_session_id"], "session-fresh");
    assert_eq!(deferred["event"], "model.provider.deferred");
    assert_eq!(deferred["action"], "provider_deferred");
    assert_eq!(deferred["workstream_id"], "workstream-750");
    assert_eq!(wake["event"], "model.provider.wake");
    assert_eq!(wake["action"], "provider_health_wake");
    assert_eq!(wake["cumulative_failure_count"], 3);
    assert_eq!(wake["deferral_count"], 1);
    assert_eq!(wake["generation"], 2);
    assert_eq!(wake["final_disposition"], "retryable");
    assert_eq!(wake["boundary"], "http");
    assert_eq!(wake["provider_request_id"], "request-750");
    assert_eq!(cleared["event"], "model.recovery.cleared");
    assert_eq!(cleared["action"], "success_clear");
    assert_eq!(cleared["final_disposition"], "succeeded");
    assert_eq!(cleared["cumulative_failure_count"], 3);
    assert_eq!(parked["event"], "model.failure.parked");
    assert_eq!(parked["action"], "park_for_human");
    assert_eq!(parked["current_session_id"], "session-fresh");
    assert_eq!(parked["prior_session_id"], "session-prior");
    assert_eq!(parked["artifact.ref"], "ai/temper#750");
    assert!(
        !serde_json::to_string(&records)
            .unwrap()
            .contains("Provider is unavailable.")
    );
}

fn decision<'a>(
    failure: &'a ModelFailureV1,
    attempt_id: &'a str,
    action: &'a str,
    current_session_id: &'a str,
    prior_session_id: Option<&'a str>,
    new_session_id: Option<&'a str>,
) -> ModelRecoveryDecision<'a> {
    ModelRecoveryDecision {
        attempt_id,
        failure_epoch: 2,
        failure_count: 3,
        session_number: 2,
        session_failure_count: 1,
        elapsed_ms: 45_000,
        action,
        deferral_count: 1,
        generation: 2,
        current_session_id,
        prior_session_id,
        new_session_id,
        evidence_location: ".temper-agent-session/state.json",
        model_failure: failure,
    }
}

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl io::Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuffer {
    type Writer = SharedBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
