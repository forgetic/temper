use std::collections::HashMap;
use std::sync::Mutex;

use tracing::Span;

use super::{ActivitySpanExporter, ActivitySpanKind, ActivitySpanStart, ProjectedActivitySpan};

/// Bridges canonical projections into the process-wide `tracing` subscriber.
/// With `temper-log/otel` enabled, the existing tracing-OpenTelemetry layer
/// exports these spans together with ordinary operational spans.
#[derive(Default)]
pub struct TracingActivitySpanExporter {
    active: Mutex<HashMap<String, Span>>,
}

impl ActivitySpanExporter for TracingActivitySpanExporter {
    fn span_started(&self, start: &ActivitySpanStart) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let parent = start
            .parent_span_id
            .as_ref()
            .and_then(|parent_id| active.get(parent_id));
        let span = tracing_span(start, parent);
        apply_remote_parent(&span, start);
        active.insert(start.span_id.clone(), span);
    }

    fn span_finished(&self, finished: ProjectedActivitySpan) {
        let Some(span) = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&finished.start.span_id)
        else {
            return;
        };
        span.record("event.ended_at", finished.ended_at.as_str());
        span.record("duration_ms", finished.duration_ms);
        span.record("agent.status", finished.status.as_str());
        span.record(
            "otel.status_code",
            if finished.status == super::ActivitySpanStatus::Ok {
                "ok"
            } else {
                "error"
            },
        );
        span.record("usage.input_tokens", finished.attributes.usage.input_tokens);
        span.record(
            "usage.output_tokens",
            finished.attributes.usage.output_tokens,
        );
        span.record(
            "usage.cache_read_tokens",
            finished.attributes.usage.cache_read_tokens,
        );
        span.record(
            "usage.cache_write_tokens",
            finished.attributes.usage.cache_write_tokens,
        );
        span.record("retry.count", finished.attributes.retry_count);
        span.record("retry.delay_ms", finished.attributes.retry_delay_ms);
        span.record("trace.dropped_events", finished.attributes.dropped_events);
        span.record("trace.dropped_bytes", finished.attributes.dropped_bytes);
        let dropped_kinds = finished.attributes.dropped_kinds.join(",");
        span.record("trace.dropped_kinds", dropped_kinds.as_str());
        if let Some(first_token) = finished.attributes.time_to_first_token_ms {
            span.record("llm.time_to_first_token_ms", first_token);
        }
        if let Some(stop_reason) = &finished.attributes.stop_reason {
            span.record("agent.stop_reason", stop_reason.as_str());
        }
        if let Some(terminal_reason) = finished.attributes.terminal_reason {
            span.record("agent.terminal_reason", terminal_reason.as_str());
        }
        if let Some(failure) = &finished.attributes.model_failure {
            span.record("model.failure.category", failure.category.as_str());
            span.record("model.failure.disposition", failure.disposition.as_str());
            span.record("model.failure.boundary", failure.boundary.as_str());
            span.record("model.failure.event_kind", failure.event_kind.as_str());
            span.record("model.failure.status_present", failure.status_present);
            span.record("model.failure.code_present", failure.code_present);
            span.record("model.failure.retryable", failure.retryable);
            if let Some(status) = failure.http_status {
                span.record("model.failure.http_status", u64::from(status));
            }
            if let Some(request_id) = &failure.provider_request_id {
                span.record("model.failure.request_id", request_id.as_str());
            }
            if let Some(code) = &failure.provider_error_code {
                span.record("model.failure.provider_code", code.as_str());
            }
            span.record("model.failure.detail_redacted", failure.detail_redacted);
            span.record("model.failure.message", failure.message.as_str());
        }
        drop(span);
    }
}

fn tracing_span(start: &ActivitySpanStart, parent: Option<&Span>) -> Span {
    macro_rules! create_span {
        (parent: $parent:expr, $name:literal) => {
            tracing::info_span!(
                target: "temper::agent.activity",
                parent: $parent,
                $name,
                run.id = %start.run_id,
                span.logical_id = %start.span_id,
                job.id = %start.assignment.job_id,
                repo = %start.assignment.repository,
                artifact.ref = %start.assignment.artifact_ref,
                role = %start.assignment.role,
                action = %start.assignment.action,
                correlation.key = %start.assignment.correlation_key,
                agent.session.id = start.agent_session_id.as_deref().unwrap_or(""),
                scope.id = start.attributes.scope_id.as_deref().unwrap_or(""),
                scope.parent_id = start.attributes.parent_scope_id.as_deref().unwrap_or(""),
                turn = start.attributes.turn.unwrap_or_default(),
                call.id = start.attributes.call_id.as_deref().unwrap_or(""),
                gen_ai.provider.name = start.attributes.provider.as_deref().unwrap_or(""),
                gen_ai.request.model = start.attributes.model.as_deref().unwrap_or(""),
                gen_ai.request.attempt = start.attributes.attempt.unwrap_or_default(),
                tool.name = start.attributes.tool_name.as_deref().unwrap_or(""),
                event.started_at = %start.started_at,
                event.ended_at = tracing::field::Empty,
                duration_ms = tracing::field::Empty,
                agent.status = tracing::field::Empty,
                otel.status_code = tracing::field::Empty,
                agent.stop_reason = tracing::field::Empty,
                agent.terminal_reason = tracing::field::Empty,
                model.failure.category = tracing::field::Empty,
                model.failure.disposition = tracing::field::Empty,
                model.failure.boundary = tracing::field::Empty,
                model.failure.event_kind = tracing::field::Empty,
                model.failure.status_present = tracing::field::Empty,
                model.failure.code_present = tracing::field::Empty,
                model.failure.retryable = tracing::field::Empty,
                model.failure.http_status = tracing::field::Empty,
                model.failure.request_id = tracing::field::Empty,
                model.failure.provider_code = tracing::field::Empty,
                model.failure.detail_redacted = tracing::field::Empty,
                model.failure.message = tracing::field::Empty,
                llm.time_to_first_token_ms = tracing::field::Empty,
                usage.input_tokens = tracing::field::Empty,
                usage.output_tokens = tracing::field::Empty,
                usage.cache_read_tokens = tracing::field::Empty,
                usage.cache_write_tokens = tracing::field::Empty,
                retry.count = tracing::field::Empty,
                retry.delay_ms = tracing::field::Empty,
                trace.dropped_events = tracing::field::Empty,
                trace.dropped_bytes = tracing::field::Empty,
                trace.dropped_kinds = tracing::field::Empty,
            )
        };
        ($name:literal) => {
            tracing::info_span!(
                target: "temper::agent.activity",
                $name,
                run.id = %start.run_id,
                span.logical_id = %start.span_id,
                job.id = %start.assignment.job_id,
                repo = %start.assignment.repository,
                artifact.ref = %start.assignment.artifact_ref,
                role = %start.assignment.role,
                action = %start.assignment.action,
                correlation.key = %start.assignment.correlation_key,
                agent.session.id = start.agent_session_id.as_deref().unwrap_or(""),
                scope.id = start.attributes.scope_id.as_deref().unwrap_or(""),
                scope.parent_id = start.attributes.parent_scope_id.as_deref().unwrap_or(""),
                turn = start.attributes.turn.unwrap_or_default(),
                call.id = start.attributes.call_id.as_deref().unwrap_or(""),
                gen_ai.provider.name = start.attributes.provider.as_deref().unwrap_or(""),
                gen_ai.request.model = start.attributes.model.as_deref().unwrap_or(""),
                gen_ai.request.attempt = start.attributes.attempt.unwrap_or_default(),
                tool.name = start.attributes.tool_name.as_deref().unwrap_or(""),
                event.started_at = %start.started_at,
                event.ended_at = tracing::field::Empty,
                duration_ms = tracing::field::Empty,
                agent.status = tracing::field::Empty,
                otel.status_code = tracing::field::Empty,
                agent.stop_reason = tracing::field::Empty,
                agent.terminal_reason = tracing::field::Empty,
                model.failure.category = tracing::field::Empty,
                model.failure.disposition = tracing::field::Empty,
                model.failure.boundary = tracing::field::Empty,
                model.failure.event_kind = tracing::field::Empty,
                model.failure.status_present = tracing::field::Empty,
                model.failure.code_present = tracing::field::Empty,
                model.failure.retryable = tracing::field::Empty,
                model.failure.http_status = tracing::field::Empty,
                model.failure.request_id = tracing::field::Empty,
                model.failure.provider_code = tracing::field::Empty,
                model.failure.detail_redacted = tracing::field::Empty,
                model.failure.message = tracing::field::Empty,
                llm.time_to_first_token_ms = tracing::field::Empty,
                usage.input_tokens = tracing::field::Empty,
                usage.output_tokens = tracing::field::Empty,
                usage.cache_read_tokens = tracing::field::Empty,
                usage.cache_write_tokens = tracing::field::Empty,
                retry.count = tracing::field::Empty,
                retry.delay_ms = tracing::field::Empty,
                trace.dropped_events = tracing::field::Empty,
                trace.dropped_bytes = tracing::field::Empty,
                trace.dropped_kinds = tracing::field::Empty,
            )
        };
    }
    macro_rules! named_span {
        ($name:literal) => {
            if let Some(parent) = parent {
                create_span!(parent: parent, $name)
            } else {
                create_span!($name)
            }
        };
    }
    match start.kind {
        ActivitySpanKind::Run => named_span!("agent.run"),
        ActivitySpanKind::Scope => named_span!("agent.scope"),
        ActivitySpanKind::Turn => named_span!("agent.turn"),
        ActivitySpanKind::ModelCall => named_span!("llm.call"),
        ActivitySpanKind::Tool => named_span!("tool.call"),
    }
}

#[cfg(feature = "otel")]
fn apply_remote_parent(span: &Span, start: &ActivitySpanStart) {
    use opentelemetry::propagation::{Extractor, TextMapPropagator};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;

    struct Headers<'a>(&'a temper_protocol_activity::W3cTraceContext);
    impl Extractor for Headers<'_> {
        fn get(&self, key: &str) -> Option<&str> {
            match key {
                "traceparent" => Some(self.0.traceparent.as_str()),
                "tracestate" => self.0.tracestate.as_deref(),
                _ => None,
            }
        }

        fn keys(&self) -> Vec<&str> {
            if self.0.tracestate.is_some() {
                vec!["traceparent", "tracestate"]
            } else {
                vec!["traceparent"]
            }
        }
    }

    if let Some(remote_parent) = &start.remote_parent {
        let context = TraceContextPropagator::new().extract(&Headers(remote_parent));
        let _ = span.set_parent(context);
    }
}

#[cfg(not(feature = "otel"))]
fn apply_remote_parent(_span: &Span, _start: &ActivitySpanStart) {}
