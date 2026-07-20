//! Tests for `shell::streaming`.

use super::{
    MAX_STREAM_RETRIES, ModelCallObservability, ModelOperationContext, ModelTaskOutcome,
    stream_to_completion,
};
use crate::ModelFailureCategory;
use crate::machine::{AgentEvent, ModelCallStatus};
use crate::shell::task_group::CancellationToken;
use crate::shell::{EventClock, EventSink, ModelIdentity, SystemEventClock};
use futures::StreamExt;
use skein::lab::{LabConfig, LabRuntime};
use skein::types::Budget;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tongs::model::{AssistantMessage, StopReason, StreamEvent, Usage};
use tongs::provider::{Context, EventStream, Provider, StreamOptions};

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
    let mut runtime = LabRuntime::new(LabConfig::new(19).with_auto_advance().max_steps(200_000));
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

#[derive(Clone, Copy)]
enum StallPoint {
    Connect,
    FirstEvent,
    IdleEvent,
}

struct StalledProvider {
    point: StallPoint,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for StalledProvider {
    fn api(&self) -> &str {
        "stalled"
    }

    async fn stream(
        &self,
        _context: &Context<'_>,
        _options: &StreamOptions,
    ) -> tongs::Result<EventStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.point {
            StallPoint::Connect => futures::future::pending::<tongs::Result<EventStream>>().await,
            StallPoint::FirstEvent => Ok(EventStream::new(futures::stream::pending())),
            StallPoint::IdleEvent => Ok(EventStream::new(
                futures::stream::once(async { Ok(StreamEvent::Start) })
                    .chain(futures::stream::pending()),
            )),
        }
    }
}

fn run_stalled_model(
    point: StallPoint,
    connect_timeout: Duration,
    idle_timeout: Duration,
) -> (ModelTaskOutcome, Vec<AgentEvent>, usize, u64) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = StalledProvider {
        point,
        calls: Arc::clone(&calls),
    };
    let events = Arc::new(EventRecorder::default());
    let observed = Arc::clone(&events);
    let (outcome, elapsed) = run_in_lab(async move {
        let cancellation = CancellationToken::default();
        stream_to_completion(
            &provider,
            None,
            &[],
            &[],
            &StreamOptions::default(),
            ModelOperationContext {
                connect_timeout,
                idle_timeout,
                cancellation: &cancellation,
            },
            ModelCallObservability {
                turn: 0,
                model: &ModelIdentity::new("test", "stalled"),
                clock: &SystemEventClock,
                events: observed.as_ref(),
            },
        )
        .await
    });
    let recorded = events.0.lock().expect("events").clone();
    (outcome, recorded, calls.load(Ordering::SeqCst), elapsed)
}

#[test]
fn stalled_provider_connect_retries_on_virtual_time() {
    let (outcome, events, calls, elapsed) = run_stalled_model(
        StallPoint::Connect,
        Duration::from_secs(2),
        Duration::from_secs(11),
    );
    assert!(matches!(
        outcome,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::Timeout
                && diagnostic.message() == "Model connect deadline elapsed."
                && diagnostic.retryable()
    ));
    assert_eq!(calls, MAX_STREAM_RETRIES + 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ModelCallRetrying { .. }))
            .count(),
        MAX_STREAM_RETRIES
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::ModelCallFinished {
                    status: ModelCallStatus::Failed,
                    ..
                }
            ))
            .count(),
        MAX_STREAM_RETRIES + 1
    );
    assert!(elapsed >= Duration::from_secs(14).as_nanos() as u64);
}

#[test]
fn first_and_idle_stream_events_use_distinct_resolved_limits() {
    let (first, _, first_calls, _) = run_stalled_model(
        StallPoint::FirstEvent,
        Duration::from_secs(3),
        Duration::from_secs(17),
    );
    assert!(matches!(
        first,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::Timeout
                && diagnostic.message() == "Model connect deadline elapsed."
    ));
    assert_eq!(first_calls, MAX_STREAM_RETRIES + 1);

    let (idle, _, idle_calls, _) = run_stalled_model(
        StallPoint::IdleEvent,
        Duration::from_secs(3),
        Duration::from_secs(5),
    );
    assert!(matches!(
        idle,
        ModelTaskOutcome::Failed(ref diagnostic)
            if diagnostic.category() == ModelFailureCategory::Timeout
                && diagnostic.message() == "Model stream idle deadline elapsed."
    ));
    assert_eq!(idle_calls, MAX_STREAM_RETRIES + 1);
}

#[test]
fn external_model_cancellation_emits_terminal_boundary_without_time_advance() {
    struct CancellingProvider(CancellationToken);

    #[async_trait::async_trait]
    impl Provider for CancellingProvider {
        fn api(&self) -> &str {
            "cancel"
        }

        async fn stream(
            &self,
            _context: &Context<'_>,
            _options: &StreamOptions,
        ) -> tongs::Result<EventStream> {
            self.0.cancel();
            futures::future::pending::<tongs::Result<EventStream>>().await
        }
    }

    let cancellation = CancellationToken::default();
    let provider = CancellingProvider(cancellation.clone());
    let events = Arc::new(EventRecorder::default());
    let observed = Arc::clone(&events);
    let (outcome, elapsed) = run_in_lab(async move {
        stream_to_completion(
            &provider,
            None,
            &[],
            &[],
            &StreamOptions::default(),
            ModelOperationContext {
                connect_timeout: Duration::from_secs(120),
                idle_timeout: Duration::from_secs(120),
                cancellation: &cancellation,
            },
            ModelCallObservability {
                turn: 4,
                model: &ModelIdentity::new("test", "cancel"),
                clock: &SystemEventClock,
                events: observed.as_ref(),
            },
        )
        .await
    });
    assert!(matches!(outcome, ModelTaskOutcome::Cancelled));
    assert_eq!(elapsed, 0);
    let events = events.0.lock().expect("events");
    assert!(matches!(
        events.as_slice(),
        [
            AgentEvent::ModelCallStarted { turn: 4, .. },
            AgentEvent::ModelCallFinished {
                turn: 4,
                status: ModelCallStatus::Cancelled,
                duration_ms: 0,
                ..
            }
        ]
    ));
}

#[test]
fn model_attempt_timing_and_usage_use_the_injected_clock() {
    struct FakeClock(Mutex<VecDeque<u64>>);
    impl EventClock for FakeClock {
        fn now_millis(&self) -> u64 {
            self.0
                .lock()
                .expect("clock")
                .pop_front()
                .expect("clock value")
        }
    }

    struct FakeProvider;
    #[async_trait::async_trait]
    impl Provider for FakeProvider {
        fn api(&self) -> &str {
            "fake-api"
        }

        async fn stream(
            &self,
            _context: &Context<'_>,
            _options: &StreamOptions,
        ) -> tongs::Result<EventStream> {
            let message = AssistantMessage {
                content: Vec::new(),
                api: "fake-api".to_string(),
                provider: "fake-provider".to_string(),
                model: "fake-model".to_string(),
                usage: Usage {
                    input: 11,
                    output: 7,
                    cache_read: 5,
                    cache_write: 3,
                    ..Usage::default()
                },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            Ok(EventStream::from_events(vec![
                Ok(StreamEvent::Start),
                Ok(StreamEvent::TextDelta {
                    content_index: 0,
                    delta: "token".to_string(),
                }),
                Ok(StreamEvent::Done {
                    reason: StopReason::Stop,
                    message,
                }),
            ]))
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<AgentEvent>>);
    impl EventSink for Recorder {
        fn emit(&self, event: AgentEvent) {
            self.0.lock().expect("events").push(event);
        }
    }

    let recorder = Arc::new(Recorder::default());
    let observed = Arc::clone(&recorder);
    temper_agent_io::block_on(async move {
        let clock = FakeClock(Mutex::new(VecDeque::from([100, 125, 180])));
        let cancellation = CancellationToken::default();
        let completion = stream_to_completion(
            &FakeProvider,
            None,
            &[],
            &[],
            &StreamOptions::default(),
            ModelOperationContext {
                connect_timeout: Duration::from_secs(2),
                idle_timeout: Duration::from_secs(2),
                cancellation: &cancellation,
            },
            ModelCallObservability {
                turn: 2,
                model: &ModelIdentity::new("fake-provider", "fake-model"),
                clock: &clock,
                events: observed.as_ref(),
            },
        )
        .await;
        assert!(matches!(completion, ModelTaskOutcome::Responded(_)));
    });

    let events = recorder.0.lock().expect("events");
    assert!(matches!(
        events[0],
        AgentEvent::ModelCallStarted {
            turn: 2,
            attempt: 0,
            ..
        }
    ));
    let AgentEvent::ModelCallFinished {
        status,
        duration_ms,
        time_to_first_token_ms,
        stop_reason,
        usage,
        ..
    } = &events[2]
    else {
        panic!("expected terminal attempt event");
    };
    assert_eq!(*status, ModelCallStatus::Succeeded);
    assert_eq!(*duration_ms, 80);
    assert_eq!(*time_to_first_token_ms, Some(25));
    assert_eq!(*stop_reason, Some(StopReason::Stop));
    assert_eq!(usage.cache_read, 5);
    assert_eq!(usage.cache_write, 3);
}
