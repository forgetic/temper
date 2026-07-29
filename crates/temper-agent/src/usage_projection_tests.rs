use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityChildRecordV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentScopeKindV1, AgentScopeV1, AgentTerminalReasonV1, FailureCodeV1,
    FailureInfoV1, ModelCallFinishedV1, ModelCallRetryingV1, ModelCallStatusV1,
    ModelFailureCategoryV1, ModelFailureV1, ScopeFinishedV1, ScopeStatusV1,
};
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

use super::{ActivityProjection, TracingProjection, UsageTotals};

#[derive(Default)]
struct Visitor(BTreeMap<String, String>);

impl Visit for Visitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let mut visitor = Visitor::default();
        event.record(&mut visitor);
        self.0.lock().unwrap().push(visitor.0);
    }
}

fn record(event: AgentActivityEventV1) -> AgentActivityChildRecordV1 {
    AgentActivityChildRecordV1 {
        frame: AgentActivityFrameV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            occurred_at: "2026-07-18T12:00:00.000Z".into(),
            elapsed_ms: 10,
            scope: AgentScopeV1 {
                id: "main-532".into(),
                kind: AgentScopeKindV1::Main,
                parent_id: None,
            },
            turn: Some(1),
            event,
        },
        blobs: Vec::new(),
    }
}

fn diagnostic(category: ModelFailureCategoryV1) -> ModelFailureV1 {
    let (status, request_id, code, message, redacted) = match category {
        ModelFailureCategoryV1::Timeout => (None, None, None, "Model request timed out.", false),
        ModelFailureCategoryV1::RateLimit => (
            Some(429),
            Some("req_rate_532"),
            Some("rate_limit"),
            "Rate limit exceeded; retry later.",
            false,
        ),
        ModelFailureCategoryV1::Response => (
            Some(502),
            Some("req_response_532"),
            Some("malformed_stream"),
            "Provider returned a malformed stream.",
            false,
        ),
        ModelFailureCategoryV1::RedactedUnknown => (
            None,
            None,
            None,
            temper_protocol_activity::REDACTED_MODEL_FAILURE_MESSAGE,
            true,
        ),
        _ => unreachable!("test covers operator-facing regression categories"),
    };
    let mut failure = ModelFailureV1 {
        provider: "openai".into(),
        model: "gpt-test".into(),
        category,
        disposition: temper_protocol_activity::ModelFailureDispositionV1::Unknown,
        boundary: if status.is_some() {
            temper_protocol_activity::ModelFailureBoundaryV1::Http
        } else {
            temper_protocol_activity::ModelFailureBoundaryV1::Local
        },
        event_kind: if status.is_some() {
            temper_protocol_activity::ModelFailureEventKindV1::HttpResponse
        } else {
            temper_protocol_activity::ModelFailureEventKindV1::LocalError
        },
        status_present: status.is_some(),
        code_present: code.is_some(),
        retryable: false,
        http_status: status,
        provider_request_id: request_id.map(str::to_string),
        provider_error_code: code.map(str::to_string),
        message: message.into(),
        detail_redacted: redacted,
    };
    failure.normalize();
    failure
}

fn finished(call_id: &str, failure: ModelFailureV1) -> AgentActivityEventV1 {
    AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
        call_id: call_id.into(),
        attempt: 0,
        status: ModelCallStatusV1::Failed,
        duration_ms: 25,
        time_to_first_token_ms: None,
        stop_reason: None,
        failure: Some(failure),
    })
}

#[test]
fn retrying_and_terminal_logs_use_finished_call_diagnostics() {
    let capture = CaptureLayer::default();
    let events = capture.0.clone();
    let subscriber = registry().with(capture);

    with_default(subscriber, || {
        let projection = TracingProjection::new(Arc::new(UsageTotals::default()));
        let rate_limit = diagnostic(ModelFailureCategoryV1::RateLimit);
        projection.emit(&record(finished("rate", rate_limit)));
        projection.emit(&record(AgentActivityEventV1::ModelCallRetrying(
            ModelCallRetryingV1 {
                call_id: "rate".into(),
                next_attempt: 1,
                delay_ms: 500,
                disposition: temper_protocol_activity::ModelFailureDispositionV1::Retryable,
                failure: FailureInfoV1 {
                    code: FailureCodeV1::Provider,
                    message: temper_protocol_activity::MODEL_CALL_RETRY_FAILURE_MESSAGE.into(),
                    retryable: true,
                },
            },
        )));

        for (call_id, category) in [
            ("timeout", ModelFailureCategoryV1::Timeout),
            ("response", ModelFailureCategoryV1::Response),
            ("redacted", ModelFailureCategoryV1::RedactedUnknown),
        ] {
            projection.emit(&record(finished(call_id, diagnostic(category))));
        }
        projection.emit(&record(AgentActivityEventV1::ScopeFinished(
            ScopeFinishedV1 {
                status: ScopeStatusV1::Failed,
                duration_ms: 100,
                terminal_reason: Some(AgentTerminalReasonV1::ModelError),
            },
        )));
    });

    let failures = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.get("event").map(String::as_str) == Some("model.call_failed"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 3);
    for category in ["timeout", "response", "redacted_unknown"] {
        assert!(failures.iter().any(|event| {
            event.get("model.failure.category").map(String::as_str) == Some(category)
        }));
    }
    let retry = events
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.get("event").map(String::as_str) == Some("model.turn.retrying"))
        .cloned()
        .expect("retrying model turn event");
    assert_eq!(
        retry.get("provider_request_id").map(String::as_str),
        Some("req_rate_532")
    );
    assert_eq!(
        retry.get("disposition").map(String::as_str),
        Some("retryable")
    );
    assert_eq!(retry.get("boundary").map(String::as_str), Some("http"));
    assert_eq!(
        retry.get("status_present").map(String::as_str),
        Some("true")
    );
    assert!(!retry.contains_key("model_failure_message"));

    let rendered = format!("{failures:?}");
    assert!(!rendered.contains(temper_protocol_activity::MODEL_CALL_RETRY_FAILURE_MESSAGE));
    assert!(!rendered.contains("SECRET-SENTINEL-532"));
}
