//! Typed model-failure mapping and terminal-stream regressions.

use super::{
    MAX_STREAM_RETRIES, ModelCallObservability, ModelOperationContext, ModelTaskOutcome,
    SYSTEM_STREAM_RETRY_RUNTIME, stream_to_completion,
};
use crate::machine::{AgentEvent, ModelCallStatus};
use crate::shell::task_group::CancellationToken;
use crate::shell::{EventSink, ModelIdentity, SystemEventClock};
use crate::{ModelFailureCategory, ModelFailureDiagnostic};
use skein::lab::{LabConfig, LabRuntime};
use skein::types::Budget;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tongs::error::Error;
use tongs::model::{AssistantMessage, StopReason, StreamEvent, Usage};
use tongs::provider::{Context, EventStream, Provider, StreamOptions};
use tongs::{FailureCategory, ProviderFailureDiagnostic};

#[derive(Default)]
struct EventRecorder(Mutex<Vec<AgentEvent>>);

impl EventSink for EventRecorder {
    fn emit(&self, event: AgentEvent) {
        self.0.lock().expect("events").push(event);
    }
}

fn run_in_lab<T, F>(future: F) -> (T, u64)
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let mut runtime = LabRuntime::new(LabConfig::new(23).with_auto_advance().max_steps(200_000));
    let region = runtime.state.create_root_region(Budget::INFINITE);
    let result = Arc::new(Mutex::new(None));
    let task_result = Arc::clone(&result);
    let (task_id, _handle) = runtime
        .state
        .create_task(region, Budget::INFINITE, async move {
            *task_result.lock().expect("lab result") = Some(future.await);
        })
        .expect("create lab task");
    runtime.scheduler.lock().schedule(task_id, 0);
    let report = runtime.run_with_auto_advance();
    let value = result
        .lock()
        .expect("lab result")
        .take()
        .expect("lab task completed");
    (value, report.virtual_elapsed_nanos)
}

#[test]
fn transport_and_overload_errors_are_typed_and_retryable() {
    let identity = ModelIdentity::new("provider", "model");
    let transport = ModelFailureDiagnostic::from_tongs_error(
        &identity,
        &Error::Http("connection reset by peer".into()),
    );
    assert_eq!(transport.category(), ModelFailureCategory::Transport);
    assert!(transport.retryable());
    assert!(transport.detail_redacted());
    assert!(!transport.message().contains("connection reset"));

    for status in [429, 503, 529] {
        let diagnostic = ModelFailureDiagnostic::from_tongs_error(
            &identity,
            &Error::Api {
                status,
                message: "unstructured provider text".into(),
            },
        );
        assert!(diagnostic.retryable());
        assert_eq!(diagnostic.http_status(), Some(status));
        if status == 429 {
            assert_eq!(diagnostic.category(), ModelFailureCategory::RateLimit);
        } else {
            assert_eq!(diagnostic.category(), ModelFailureCategory::Provider);
        }
    }
}

#[derive(Clone)]
struct ScriptedProvider {
    events: Vec<tongs::Result<StreamEvent>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn api(&self) -> &str {
        "scripted"
    }

    async fn stream(
        &self,
        _context: &Context<'_>,
        _options: &StreamOptions,
    ) -> tongs::Result<EventStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(EventStream::from_events(self.events.clone()))
    }
}

struct UnknownThenSuccessProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for UnknownThenSuccessProvider {
    fn api(&self) -> &str {
        "unknown-then-success"
    }

    async fn stream(
        &self,
        _context: &Context<'_>,
        _options: &StreamOptions,
    ) -> tongs::Result<EventStream> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
        let event = if attempt == 0 {
            StreamEvent::Error {
                reason: StopReason::Error,
                error: assistant_with_stop(StopReason::Error),
                diagnostic: ProviderFailureDiagnostic::redacted(
                    false,
                    None,
                    Some("req_unknown_stream"),
                ),
            }
        } else {
            StreamEvent::Done {
                reason: StopReason::Stop,
                message: assistant_with_stop(StopReason::Stop),
            }
        };
        Ok(EventStream::from_events(vec![Ok(event)]))
    }
}

fn assistant_with_stop(stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: "test-api".to_string(),
        provider: "test-provider".to_string(),
        model: "test-model".to_string(),
        usage: Usage::default(),
        stop_reason,
        error_message: None,
        timestamp: 0,
    }
}

fn run_scripted(
    script: Vec<tongs::Result<StreamEvent>>,
) -> (ModelTaskOutcome, Vec<AgentEvent>, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        events: script,
        calls: Arc::clone(&calls),
    };
    let events = Arc::new(EventRecorder::default());
    let observed = Arc::clone(&events);
    let (outcome, _) = run_in_lab(async move {
        let cancellation = CancellationToken::default();
        stream_to_completion(
            &provider,
            None,
            &[],
            &[],
            &StreamOptions::default(),
            ModelOperationContext {
                connect_timeout: Duration::from_secs(10),
                idle_timeout: Duration::from_secs(10),
                retry: crate::ModelRetryLimits::default(),
                retry_runtime: &SYSTEM_STREAM_RETRY_RUNTIME,
                cancellation: &cancellation,
            },
            ModelCallObservability {
                turn: 0,
                model: &ModelIdentity::new("test-provider", "test-model"),
                clock: &SystemEventClock,
                events: observed.as_ref(),
                invocation_catalog: None,
            },
        )
        .await
    });
    let recorded = events.0.lock().expect("events").clone();
    (outcome, recorded, calls.load(Ordering::SeqCst))
}

#[test]
fn interrupted_transport_is_failed_and_keeps_the_retry_budget() {
    let (outcome, events, calls) = run_scripted(vec![Err(Error::Http(
        "connection reset with SECRET_SENTINEL".to_string(),
    ))]);

    assert_eq!(calls, MAX_STREAM_RETRIES + 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ModelCallRetrying { .. }))
            .count(),
        MAX_STREAM_RETRIES
    );
    assert!(matches!(
        outcome,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::Transport
                && diagnostic.retryable()
                && diagnostic.detail_redacted()
                && !diagnostic.message().contains("SECRET_SENTINEL")
    ));
}

#[test]
fn unclassified_stream_failure_retries_same_call_and_later_succeeds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = UnknownThenSuccessProvider {
        calls: Arc::clone(&calls),
    };
    let events = Arc::new(EventRecorder::default());
    let observed = Arc::clone(&events);
    let (outcome, _) = run_in_lab(async move {
        let cancellation = CancellationToken::default();
        stream_to_completion(
            &provider,
            None,
            &[],
            &[],
            &StreamOptions::default(),
            ModelOperationContext {
                connect_timeout: Duration::from_secs(10),
                idle_timeout: Duration::from_secs(10),
                retry: crate::ModelRetryLimits::default(),
                retry_runtime: &SYSTEM_STREAM_RETRY_RUNTIME,
                cancellation: &cancellation,
            },
            ModelCallObservability {
                turn: 4,
                model: &ModelIdentity::new("test-provider", "test-model"),
                clock: &SystemEventClock,
                events: observed.as_ref(),
                invocation_catalog: None,
            },
        )
        .await
    });

    assert!(matches!(outcome, ModelTaskOutcome::Responded(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let recorded = events.0.lock().expect("events");
    let starts = recorded
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ModelCallStarted {
                call_id, attempt, ..
            } => Some((call_id.as_str(), *attempt)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts, [("turn-4", 0), ("turn-4", 1)]);
    assert!(recorded.iter().any(|event| matches!(
        event,
        AgentEvent::ModelCallRetrying { reason, .. }
            if reason.disposition() == crate::ModelFailureDisposition::Unknown
                && reason.boundary() == crate::ModelFailureBoundary::Sse
                && reason.event_kind() == crate::ModelFailureEventKind::StreamError
                && reason.provider_request_id() == Some("req_unknown_stream")
    )));
}

#[test]
fn terminal_stream_error_is_a_failed_attempt_with_typed_diagnostic() {
    let provider_diagnostic = ProviderFailureDiagnostic::new(
        FailureCategory::Authentication,
        false,
        Some(401),
        Some("req_auth"),
        Some("invalid_api_key"),
        "Authentication failed.",
    );
    let (outcome, events, calls) = run_scripted(vec![Ok(StreamEvent::Error {
        reason: StopReason::Error,
        error: assistant_with_stop(StopReason::Error),
        diagnostic: provider_diagnostic,
    })]);

    assert_eq!(calls, 1);
    assert!(matches!(
        outcome,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::Authentication
                && diagnostic.http_status() == Some(401)
                && diagnostic.provider_request_id() == Some("req_auth")
    ));
    assert!(matches!(
        events.as_slice(),
        [
            AgentEvent::ModelCallStarted { .. },
            AgentEvent::ModelCallFinished {
                status: ModelCallStatus::Failed,
                stop_reason: Some(StopReason::Error),
                failure: Some(_),
                ..
            }
        ]
    ));
}

#[test]
fn defensive_done_with_error_is_failed_never_succeeded() {
    let (outcome, events, calls) = run_scripted(vec![Ok(StreamEvent::Done {
        reason: StopReason::Error,
        message: assistant_with_stop(StopReason::Error),
    })]);

    assert_eq!(calls, 1);
    assert!(matches!(
        outcome,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::Response
                && !diagnostic.retryable()
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ModelCallFinished {
            status: ModelCallStatus::Failed,
            stop_reason: Some(StopReason::Error),
            ..
        }
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::ModelCallFinished {
            status: ModelCallStatus::Succeeded,
            stop_reason: Some(StopReason::Error),
            ..
        }
    )));
}

#[test]
fn terminal_rate_limit_keeps_retry_count_and_typed_reason() {
    let provider_diagnostic = ProviderFailureDiagnostic::new(
        FailureCategory::RateLimit,
        true,
        Some(429),
        Some("req_rate"),
        Some("rate_limit"),
        "Please retry later.",
    );
    let (outcome, events, calls) = run_scripted(vec![Ok(StreamEvent::Error {
        reason: StopReason::Error,
        error: assistant_with_stop(StopReason::Error),
        diagnostic: provider_diagnostic,
    })]);

    assert_eq!(calls, MAX_STREAM_RETRIES + 1);
    let retry_reasons = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ModelCallRetrying { reason, .. } => Some(reason),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retry_reasons.len(), MAX_STREAM_RETRIES);
    assert!(retry_reasons.iter().all(|reason| {
        reason.category() == ModelFailureCategory::RateLimit
            && reason.http_status() == Some(429)
            && reason.provider_request_id() == Some("req_rate")
    }));
    assert!(matches!(
        outcome,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::RateLimit
                && diagnostic.http_status() == Some(429)
                && diagnostic.retryable()
    ));
}

#[test]
fn malformed_response_error_is_failed_and_sanitized() {
    let provider_diagnostic = ProviderFailureDiagnostic::new(
        FailureCategory::Response,
        false,
        None,
        None,
        Some("malformed_sse"),
        "Malformed provider response.",
    );
    let (outcome, events, calls) = run_scripted(vec![Err(Error::Provider(provider_diagnostic))]);

    assert_eq!(calls, 1);
    assert!(matches!(
        outcome,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::Response
                && diagnostic.provider_error_code() == Some("malformed_sse")
    ));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::ModelCallFinished {
            status: ModelCallStatus::Failed,
            failure: Some(_),
            ..
        })
    ));
}

#[test]
fn authentication_context_and_malformed_errors_are_typed_and_not_retried() {
    let identity = ModelIdentity::new("provider", "model");
    for (error, category) in [
        (
            Error::Auth("expired secret".into()),
            ModelFailureCategory::Authentication,
        ),
        (
            Error::Decode("malformed body".into()),
            ModelFailureCategory::Response,
        ),
    ] {
        let diagnostic = ModelFailureDiagnostic::from_tongs_error(&identity, &error);
        assert_eq!(diagnostic.category(), category);
        assert!(!diagnostic.retryable());
        assert!(diagnostic.detail_redacted());
    }

    let context = ProviderFailureDiagnostic::new(
        FailureCategory::Context,
        false,
        Some(400),
        Some("req_context"),
        Some("context_length_exceeded"),
        "Context window exceeded.",
    );
    let diagnostic = ModelFailureDiagnostic::from_tongs_error(&identity, &Error::Provider(context));
    assert_eq!(diagnostic.category(), ModelFailureCategory::Context);
    assert!(!diagnostic.retryable());
    assert_eq!(
        diagnostic.provider_error_code(),
        Some("context_length_exceeded")
    );
}

#[test]
fn client_and_deterministic_errors_are_not_retryable() {
    let identity = ModelIdentity::new("provider", "model");
    for error in [
        Error::Api {
            status: 400,
            message: "max_tokens too large".into(),
        },
        Error::Api {
            status: 401,
            message: "unauthorized".into(),
        },
        Error::Api {
            status: 404,
            message: "model not available".into(),
        },
        Error::Auth("expired".into()),
        Error::Decode("bad json".into()),
        Error::Aborted,
    ] {
        assert!(!ModelFailureDiagnostic::from_tongs_error(&identity, &error).retryable());
    }
}
