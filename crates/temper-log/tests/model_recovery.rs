// SPDX-License-Identifier: MPL-2.0

use std::io;
use std::sync::{Arc, Mutex};

use temper_log::WorkItemRef;
use temper_log::emit::{
    ModelFailureParked, ModelRecoveryDecision, ModelSessionRotated, emit_model_failure_parked,
    emit_model_session_rotated,
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
        retryable: false,
        http_status: Some(503),
        provider_request_id: Some("request-750".into()),
        provider_error_code: Some("unavailable".into()),
        message: "Provider is unavailable.".into(),
        detail_redacted: false,
    };

    with_default(subscriber, || {
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
        let item = WorkItemRef::issue("ai/temper", 750);
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
    assert_eq!(records.len(), 2);
    let rotation = &records[0]["fields"];
    let parked = &records[1]["fields"];
    for fields in [rotation, parked] {
        assert_eq!(fields["provider"], "fixture-provider");
        assert_eq!(fields["model"], "fixture-model");
        assert_eq!(fields["category"], "provider");
        assert_eq!(fields["retryable"], false);
        assert_eq!(fields["http_status"], 503);
        assert_eq!(fields["provider_request_id"], "request-750");
        assert_eq!(fields["provider_error_code"], "unavailable");
        assert_eq!(
            fields["evidence_location"],
            ".temper-agent-session/state.json"
        );
        assert_eq!(fields["model_failure_message"], "Provider is unavailable.");
    }
    assert_eq!(rotation["event"], "model.session.rotated");
    assert_eq!(rotation["action"], "rotate_session");
    assert_eq!(rotation["current_session_id"], "session-prior");
    assert_eq!(rotation["new_session_id"], "session-fresh");
    assert_eq!(parked["event"], "model.failure.parked");
    assert_eq!(parked["action"], "park_for_human");
    assert_eq!(parked["current_session_id"], "session-fresh");
    assert_eq!(parked["prior_session_id"], "session-prior");
    assert_eq!(parked["artifact.ref"], "ai/temper#750");
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
        failure_count: 1,
        action,
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
