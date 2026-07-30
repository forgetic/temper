use std::sync::Arc;

use temper_protocol_activity::{
    AgentActivityEventV1 as Event, AgentRunEventV1, AgentScopeKindV1, AgentTerminalReasonV1,
    ModelCallFinishedV1, ModelCallStartedV1, ModelCallStatusV1, ModelFailureCategoryV1,
    ModelFailureV1, ScopeFinishedV1, ScopeStatusV1, StopReasonV1,
};

#[cfg(feature = "otel")]
use crate::activity::TracingActivitySpanExporter;
use crate::activity::{
    ActivitySpanKind, ActivitySpanStatus, CanonicalActivityProjector, InMemoryActivitySpanExporter,
};

use super::{event, scope};

fn failed_model_run(
    failure: Option<ModelFailureV1>,
    status: ModelCallStatusV1,
    stop_reason: Option<StopReasonV1>,
) -> Vec<AgentRunEventV1> {
    let main = scope("main-1", AgentScopeKindV1::Main, None);
    vec![
        event(
            1,
            0,
            main.clone(),
            Some(0),
            Event::ModelCallStarted(ModelCallStartedV1 {
                call_id: "failed-model".into(),
                provider: "openai".into(),
                model: "gpt-test".into(),
                attempt: 0,
            }),
        ),
        event(
            2,
            25,
            main.clone(),
            Some(0),
            Event::ModelCallFinished(ModelCallFinishedV1 {
                call_id: "failed-model".into(),
                attempt: 0,
                status,
                duration_ms: 25,
                time_to_first_token_ms: None,
                stop_reason,
                failure,
            }),
        ),
        event(
            3,
            30,
            main,
            None,
            Event::ScopeFinished(ScopeFinishedV1 {
                status: ScopeStatusV1::Failed,
                duration_ms: 30,
                terminal_reason: Some(AgentTerminalReasonV1::ModelError),
            }),
        ),
    ]
}

fn safe_failure(category: ModelFailureCategoryV1) -> ModelFailureV1 {
    let (http_status, request_id, provider_code, message, detail_redacted) = match category {
        ModelFailureCategoryV1::Timeout => {
            (Some(504), None, None, "Model request timed out.", false)
        }
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
        _ => (None, None, None, "Provider request failed.", false),
    };
    let mut failure = ModelFailureV1 {
        provider: "openai".into(),
        model: "gpt-test".into(),
        category,
        disposition: temper_protocol_activity::ModelFailureDispositionV1::Unknown,
        boundary: if http_status.is_some() {
            temper_protocol_activity::ModelFailureBoundaryV1::Http
        } else {
            temper_protocol_activity::ModelFailureBoundaryV1::Local
        },
        event_kind: if http_status.is_some() {
            temper_protocol_activity::ModelFailureEventKindV1::HttpResponse
        } else {
            temper_protocol_activity::ModelFailureEventKindV1::LocalError
        },
        status_present: http_status.is_some(),
        code_present: provider_code.is_some(),
        retryable: false,
        http_status,
        provider_request_id: request_id.map(str::to_string),
        provider_error_code: provider_code.map(str::to_string),
        message: message.into(),
        detail_redacted,
    };
    failure.normalize();
    failure
}

#[test]
fn failed_model_spans_project_every_safe_diagnostic_category() {
    for category in [
        ModelFailureCategoryV1::Timeout,
        ModelFailureCategoryV1::RateLimit,
        ModelFailureCategoryV1::Response,
        ModelFailureCategoryV1::RedactedUnknown,
    ] {
        let failure = safe_failure(category);
        let exporter = Arc::new(InMemoryActivitySpanExporter::default());
        let mut projector = CanonicalActivityProjector::new(exporter.clone());
        projector.project_all(&failed_model_run(
            Some(failure.clone()),
            ModelCallStatusV1::Failed,
            None,
        ));

        let model = exporter
            .finished_spans()
            .into_iter()
            .find(|span| span.start.kind == ActivitySpanKind::ModelCall)
            .expect("failed model span");
        assert_eq!(model.status, ActivitySpanStatus::Error);
        assert_eq!(model.attributes.model_failure.as_ref(), Some(&failure));
        assert!(!format!("{model:?}").contains("SECRET-SENTINEL-532"));
    }
}

#[test]
fn retained_legacy_error_stop_projects_error_status() {
    let exporter = Arc::new(InMemoryActivitySpanExporter::default());
    let mut projector = CanonicalActivityProjector::new(exporter.clone());
    projector.project_all(&failed_model_run(
        None,
        ModelCallStatusV1::Succeeded,
        Some(StopReasonV1::Error),
    ));

    let model = exporter
        .finished_spans()
        .into_iter()
        .find(|span| span.start.kind == ActivitySpanKind::ModelCall)
        .expect("legacy model span");
    assert_eq!(model.status, ActivitySpanStatus::Error);
    assert_eq!(model.attributes.stop_reason.as_deref(), Some("error"));
}

#[cfg(feature = "otel")]
#[test]
fn tracing_bridge_exports_safe_model_failure_fields_and_error_status() {
    use opentelemetry::trace::{Status, TracerProvider as _};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing_subscriber::prelude::*;

    let otel_exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(otel_exporter.clone())
        .build();
    let tracer = provider.tracer("temper-model-failure-test");
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_location(false),
    );
    let failure = safe_failure(ModelFailureCategoryV1::RateLimit);

    tracing::subscriber::with_default(subscriber, || {
        let mut projector =
            CanonicalActivityProjector::new(Arc::new(TracingActivitySpanExporter::default()));
        projector.project_all(&failed_model_run(
            Some(failure),
            ModelCallStatusV1::Failed,
            None,
        ));
    });

    provider.force_flush().unwrap();
    let spans = otel_exporter.get_finished_spans().unwrap();
    let model = spans
        .iter()
        .find(|span| span.name == "llm.call")
        .expect("failed llm.call span");
    assert!(matches!(model.status, Status::Error { .. }));
    let attributes = format!("{:?}", model.attributes);
    for expected in [
        "model.failure.category",
        "rate_limit",
        "model.failure.disposition",
        "retryable",
        "model.failure.boundary",
        "http",
        "model.failure.event_kind",
        "http_response",
        "model.failure.status_present",
        "model.failure.code_present",
        "model.failure.retryable",
        "model.failure.http_status",
        "429",
        "model.failure.request_id",
        "req_rate_532",
        "model.failure.provider_code",
        "model.failure.detail_redacted",
        "model.failure.message",
        "Rate limit exceeded; retry later.",
    ] {
        assert!(
            attributes.contains(expected),
            "missing {expected}: {attributes}"
        );
    }
    assert!(!attributes.contains("SECRET-SENTINEL-532"));
}
