//! Always-on, content-free lifecycle normalization and transport.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::net::{TcpStream, ToSocketAddrs};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[cfg(test)]
use temper_agent_core::ModelFailureDiagnostic;
use temper_agent_core::{
    AgentEvent, AgentStop, EventSink, ModelCallStatus, StreamDelta, ToolCallStatus,
};
use temper_protocol_agent::{
    AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentLifecycleAgentStatusV1, AgentLifecycleEventV1,
    AgentLifecycleFrameV1, AgentLifecycleHelloV1, AgentLifecycleModelStatusV1,
    AgentLifecycleScopeV1, AgentLifecycleToolStatusV1, MAX_AGENT_LIFECYCLE_FRAME_BYTES,
};

use super::{ActivityClock, NormalizingEventSink};

mod cancellation;
pub use cancellation::AgentCancellationLatch;
use cancellation::spawn_command_reader;

const LIFECYCLE_QUEUE_CAPACITY: usize = 256;
const LIFECYCLE_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const LIFECYCLE_WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const LIFECYCLE_TERMINAL_FLUSH_TIMEOUT: Duration = Duration::from_millis(500);
const PROGRESS_WINDOW_MS: u64 = 5_000;

/// Direct in-process progress callback. The worker reporter adds sequence and
/// attempt identity; the callback sees only the same content-free typed event
/// carried by the process transport.
pub type AgentLifecycleReporter =
    Arc<dyn Fn(AgentLifecycleScopeV1, AgentLifecycleEventV1) + Send + Sync>;

pub(super) trait LifecycleProjection: Send + Sync {
    fn emit(&self, scope: AgentLifecycleScopeV1, event: AgentLifecycleEventV1);
}

struct CallbackProjection {
    callback: AgentLifecycleReporter,
}

impl LifecycleProjection for CallbackProjection {
    fn emit(&self, scope: AgentLifecycleScopeV1, event: AgentLifecycleEventV1) {
        (self.callback)(scope, event);
    }
}

struct WriterMessage {
    bytes: Vec<u8>,
    delivered: Option<mpsc::SyncSender<()>>,
}

struct ClientState {
    next_seq: u64,
}

/// A dedicated writer. It never shares the activity queue, capture policy,
/// quota, spool, or storage path. Backpressure is bounded by the fixed channel;
/// frames are never intentionally shed or rewritten as trace gaps.
struct LifecycleClient {
    sender: mpsc::SyncSender<WriterMessage>,
    state: Mutex<ClientState>,
}

impl LifecycleClient {
    fn new(address: &str, cancellation: AgentCancellationLatch) -> Self {
        let (sender, receiver) = mpsc::sync_channel(LIFECYCLE_QUEUE_CAPACITY);
        let address = address.to_string();
        let _ = std::thread::Builder::new()
            .name("temper-agent-lifecycle".to_string())
            .spawn(move || lifecycle_writer(&address, receiver, cancellation));
        Self {
            sender,
            state: Mutex::new(ClientState { next_seq: 1 }),
        }
    }
}

impl LifecycleProjection for LifecycleClient {
    fn emit(&self, scope: AgentLifecycleScopeV1, event: AgentLifecycleEventV1) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let frame = AgentLifecycleFrameV1 {
            version: AGENT_LIFECYCLE_PROTOCOL_VERSION,
            seq: state.next_seq,
            scope,
            event,
        };
        if frame.validate().is_err() {
            return;
        }
        let Ok(mut bytes) = serde_json::to_vec(&frame) else {
            return;
        };
        if bytes.len().saturating_add(1) > MAX_AGENT_LIFECYCLE_FRAME_BYTES {
            return;
        }
        state.next_seq = state.next_seq.saturating_add(1);
        bytes.push(b'\n');
        let terminal_boundary = matches!(frame.event, AgentLifecycleEventV1::AgentFinished { .. });
        let (delivered, completion) = if terminal_boundary {
            let (sender, receiver) = mpsc::sync_channel(0);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        // Required lifecycle boundaries use bounded backpressure. A failed
        // writer disconnects the queue immediately, preserving the run result.
        if self
            .sender
            .send(WriterMessage { bytes, delivered })
            .is_err()
        {
            return;
        }
        drop(state);
        if let Some(completion) = completion {
            let _ = completion.recv_timeout(LIFECYCLE_TERMINAL_FLUSH_TIMEOUT);
        }
    }
}

fn lifecycle_writer(
    address: &str,
    receiver: mpsc::Receiver<WriterMessage>,
    cancellation: AgentCancellationLatch,
) {
    let mut addresses = match address.to_socket_addrs() {
        Ok(addresses) => addresses,
        Err(_) => return,
    };
    let Some(address) = addresses.next() else {
        return;
    };
    let stream = match TcpStream::connect_timeout(&address, LIFECYCLE_CONNECT_TIMEOUT) {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let _ = stream.set_write_timeout(Some(LIFECYCLE_WRITE_TIMEOUT));
    let command_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let writer = Arc::new(Mutex::new(stream));
    let Ok(mut hello) = serde_json::to_vec(&AgentLifecycleHelloV1::default()) else {
        return;
    };
    hello.push(b'\n');
    if writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .write_all(&hello)
        .is_err()
    {
        return;
    }
    let command_writer = Arc::clone(&writer);
    spawn_command_reader(command_stream, command_writer, cancellation);
    while let Ok(message) = receiver.recv() {
        let write_result = writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write_all(&message.bytes);
        if write_result.is_err() {
            break;
        }
        if let Some(delivered) = message.delivered {
            let _ = delivered.send(());
        }
    }
}

pub(super) fn projection(
    address: Option<&str>,
    reporter: Option<AgentLifecycleReporter>,
    cancellation: AgentCancellationLatch,
) -> Option<Arc<dyn LifecycleProjection>> {
    // A direct reporter is authoritative for in-process execution. The socket
    // carrier is used only by split-process execution, preventing duplicates if
    // a test explicitly supplies both.
    if let Some(reporter) = reporter {
        return Some(Arc::new(CallbackProjection { callback: reporter }));
    }
    address.map(|address| {
        Arc::new(LifecycleClient::new(address, cancellation)) as Arc<dyn LifecycleProjection>
    })
}

struct LifecycleState {
    current_call_id: Option<String>,
    last_progress_ms: BTreeMap<String, u64>,
}

/// Maps core machine events onto the closed correctness vocabulary.
pub(super) struct LifecycleEventSink {
    scope: AgentLifecycleScopeV1,
    clock: Arc<dyn ActivityClock>,
    projection: Arc<dyn LifecycleProjection>,
    state: Mutex<LifecycleState>,
}

impl LifecycleEventSink {
    pub(super) fn new(
        scope: AgentLifecycleScopeV1,
        clock: Arc<dyn ActivityClock>,
        projection: Arc<dyn LifecycleProjection>,
    ) -> Self {
        Self {
            scope,
            clock,
            projection,
            state: Mutex::new(LifecycleState {
                current_call_id: None,
                last_progress_ms: BTreeMap::new(),
            }),
        }
    }

    fn project(&self, event: AgentLifecycleEventV1) {
        self.projection.emit(self.scope.clone(), event);
    }

    fn normalize(&self, event: AgentEvent) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match event {
            AgentEvent::ModelCallStarted {
                call_id, attempt, ..
            } => {
                state.current_call_id = Some(call_id.clone());
                self.project(AgentLifecycleEventV1::ModelStarted { call_id, attempt });
            }
            AgentEvent::ModelCallFinished {
                call_id,
                attempt,
                status,
                ..
            } => {
                self.project(AgentLifecycleEventV1::ModelFinished {
                    call_id: call_id.clone(),
                    attempt,
                    status: model_status(status),
                });
                if state.current_call_id.as_deref() == Some(call_id.as_str()) {
                    state.current_call_id = None;
                }
                // Keep the throttle watermark: a retry retains the same
                // call_id, so progress remains bounded across attempts.
            }
            AgentEvent::ModelCallRetrying {
                call_id,
                next_attempt,
                ..
            } => {
                state.current_call_id = Some(call_id.clone());
                self.project(AgentLifecycleEventV1::ModelRetrying {
                    call_id,
                    next_attempt,
                });
            }
            AgentEvent::StreamDelta(StreamDelta::Text(text)) if !text.is_empty() => {
                self.model_progress(&mut state)
            }
            AgentEvent::StreamDelta(StreamDelta::ToolCall { .. }) => {
                self.model_progress(&mut state)
            }
            // Thinking-only and empty text deltas do not advance liveness.
            AgentEvent::StreamDelta(StreamDelta::Thinking(_))
            | AgentEvent::StreamDelta(StreamDelta::Text(_)) => {}
            AgentEvent::ToolStart { id, name, .. } => {
                self.project(AgentLifecycleEventV1::ToolStarted { call_id: id, name })
            }
            AgentEvent::ToolEnd {
                id, name, status, ..
            } => self.project(AgentLifecycleEventV1::ToolFinished {
                call_id: id,
                name,
                status: tool_status(status),
            }),
            AgentEvent::Steered { .. } => self.project(AgentLifecycleEventV1::SteeringApplied),
            AgentEvent::AgentEnd { reason } => {
                self.project(AgentLifecycleEventV1::AgentFinished {
                    status: agent_status(reason),
                });
            }
            AgentEvent::PromptPrepared { .. }
            | AgentEvent::TurnStart { .. }
            | AgentEvent::AssistantMessage { .. }
            | AgentEvent::TurnUsage { .. } => {}
        }
    }

    fn model_progress(&self, state: &mut LifecycleState) {
        let Some(call_id) = state.current_call_id.clone() else {
            return;
        };
        let now = self.clock.now().elapsed_ms;
        let allowed = state
            .last_progress_ms
            .get(&call_id)
            .is_none_or(|previous| now.saturating_sub(*previous) >= PROGRESS_WINDOW_MS);
        if allowed {
            state.last_progress_ms.insert(call_id.clone(), now);
            self.project(AgentLifecycleEventV1::ModelProgress { call_id });
        }
    }
}

impl EventSink for LifecycleEventSink {
    fn emit(&self, event: AgentEvent) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.normalize(event)));
    }
}

/// Keeps correctness lifecycle production beside, rather than inside, optional
/// activity normalization. Each sink has an independent failure boundary.
pub(super) struct CompositeEventSink {
    pub(super) activity: Arc<NormalizingEventSink>,
    pub(super) lifecycle: Option<Arc<LifecycleEventSink>>,
}

impl EventSink for CompositeEventSink {
    fn emit(&self, event: AgentEvent) {
        self.activity.emit(event.clone());
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.emit(event);
        }
    }
}

fn model_status(status: ModelCallStatus) -> AgentLifecycleModelStatusV1 {
    match status {
        ModelCallStatus::Succeeded => AgentLifecycleModelStatusV1::Succeeded,
        ModelCallStatus::Failed => AgentLifecycleModelStatusV1::Failed,
        ModelCallStatus::Cancelled => AgentLifecycleModelStatusV1::Cancelled,
    }
}

fn tool_status(status: ToolCallStatus) -> AgentLifecycleToolStatusV1 {
    match status {
        ToolCallStatus::Succeeded => AgentLifecycleToolStatusV1::Succeeded,
        ToolCallStatus::Failed => AgentLifecycleToolStatusV1::Failed,
        ToolCallStatus::Cancelled => AgentLifecycleToolStatusV1::Cancelled,
    }
}

fn agent_status(stop: AgentStop) -> AgentLifecycleAgentStatusV1 {
    match stop {
        AgentStop::Completed => AgentLifecycleAgentStatusV1::Succeeded,
        AgentStop::Aborted => AgentLifecycleAgentStatusV1::Cancelled,
        AgentStop::ModelError
        | AgentStop::BudgetExhausted
        | AgentStop::DecisionAnchorRecoveryExhausted => AgentLifecycleAgentStatusV1::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use temper_protocol_activity::{AgentActivityCapturePolicyV1, CaptureModeV1};
    use temper_protocol_agent::AgentLifecycleEventV1;

    use super::*;
    use crate::activity::{ActivityTimestamp, AgentActivityConfig, ScopeFactory};
    use crate::usage::UsageTotals;

    struct FakeClock(Mutex<VecDeque<u64>>);

    impl FakeClock {
        fn new(values: impl IntoIterator<Item = u64>) -> Self {
            Self(Mutex::new(values.into_iter().collect()))
        }
    }

    impl ActivityClock for FakeClock {
        fn now(&self) -> ActivityTimestamp {
            ActivityTimestamp {
                occurred_at: "unused".to_string(),
                elapsed_ms: self.0.lock().unwrap().pop_front().unwrap_or(10_000),
            }
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(AgentLifecycleScopeV1, AgentLifecycleEventV1)>>);

    impl LifecycleProjection for Recorder {
        fn emit(&self, scope: AgentLifecycleScopeV1, event: AgentLifecycleEventV1) {
            self.0.lock().unwrap().push((scope, event));
        }
    }

    #[test]
    fn capture_off_keeps_lifecycle_and_nested_scope_identity() {
        let lifecycle = Arc::new(Mutex::new(Vec::<(
            AgentLifecycleScopeV1,
            AgentLifecycleEventV1,
        )>::new()));
        let lifecycle_for_reporter = Arc::clone(&lifecycle);
        let factory = ScopeFactory::new(
            AgentActivityConfig {
                policy: AgentActivityCapturePolicyV1 {
                    capture: CaptureModeV1::Off,
                    ..Default::default()
                },
                lifecycle_reporter: Some(Arc::new(move |scope, event| {
                    lifecycle_for_reporter.lock().unwrap().push((scope, event));
                })),
                ..Default::default()
            },
            Arc::new(UsageTotals::default()),
        );
        let main = factory.main(
            "main",
            temper_agent_core::ModelIdentity::new("provider", "model"),
        );
        let child = factory.child(
            main.scope_id.clone(),
            "investigate",
            temper_agent_core::ModelIdentity::new("provider", "small"),
        );
        main.observability
            .events
            .emit(AgentEvent::ModelCallStarted {
                turn: 0,
                call_id: "main-call".to_string(),
                attempt: 0,
                provider: "provider".to_string(),
                model: "model".to_string(),
            });
        child.observability.events.emit(AgentEvent::ToolStart {
            id: "child-tool".to_string(),
            name: "read".to_string(),
            arg_preview: None,
        });

        let frames = lifecycle.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0.id, main.scope_id);
        assert_eq!(frames[0].0.parent_id, None);
        assert_eq!(frames[1].0.id, child.scope_id);
        assert_eq!(
            frames[1].0.parent_id.as_deref(),
            Some(main.scope_id.as_str())
        );
    }

    #[test]
    fn maps_every_boundary_and_throttles_only_meaningful_stream_progress() {
        let recorder = Arc::new(Recorder::default());
        let sink = LifecycleEventSink::new(
            AgentLifecycleScopeV1 {
                id: "main".to_string(),
                parent_id: None,
            },
            Arc::new(FakeClock::new([0, 4_999, 5_000])),
            recorder.clone(),
        );
        sink.emit(AgentEvent::ModelCallStarted {
            turn: 0,
            call_id: "model-1".to_string(),
            attempt: 0,
            provider: "not-forwarded".to_string(),
            model: "not-forwarded".to_string(),
        });
        sink.emit(AgentEvent::StreamDelta(StreamDelta::Text("x".to_string())));
        sink.emit(AgentEvent::StreamDelta(StreamDelta::Text("y".to_string())));
        sink.emit(AgentEvent::StreamDelta(StreamDelta::Thinking(
            "thinking does not count".to_string(),
        )));
        sink.emit(AgentEvent::StreamDelta(StreamDelta::ToolCall {
            id: "not-forwarded".to_string(),
            name: "not-forwarded".to_string(),
        }));
        sink.emit(AgentEvent::ModelCallFinished {
            turn: 0,
            call_id: "model-1".to_string(),
            attempt: 0,
            status: ModelCallStatus::Failed,
            duration_ms: 1,
            time_to_first_token_ms: None,
            stop_reason: None,
            usage: Default::default(),
            failure: Some(ModelFailureDiagnostic::redacted_unknown(
                "not-forwarded",
                "not-forwarded",
                true,
            )),
        });
        sink.emit(AgentEvent::ModelCallRetrying {
            turn: 0,
            call_id: "model-1".to_string(),
            next_attempt: 1,
            delay_ms: 999,
            reason: ModelFailureDiagnostic::redacted_unknown(
                "not-forwarded",
                "not-forwarded",
                true,
            ),
        });
        sink.emit(AgentEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "read".to_string(),
            arg_preview: Some("not-forwarded".to_string()),
        });
        sink.emit(AgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "read".to_string(),
            status: ToolCallStatus::Succeeded,
            duration_ms: 12,
            result: temper_agent_core::ToolResultMetadata {
                preview: Some("not-forwarded".to_string()),
                bytes: 99,
                truncated: false,
                failure: None,
                codebase_memory_timing: None,
                graph_correlation: None,
            },
        });
        sink.emit(AgentEvent::Steered { count: 1 });
        sink.emit(AgentEvent::AgentEnd {
            reason: AgentStop::Completed,
        });

        let events = recorder.0.lock().unwrap();
        assert_eq!(
            events.iter().map(|(_, event)| event).collect::<Vec<_>>(),
            vec![
                &AgentLifecycleEventV1::ModelStarted {
                    call_id: "model-1".to_string(),
                    attempt: 0,
                },
                &AgentLifecycleEventV1::ModelProgress {
                    call_id: "model-1".to_string(),
                },
                &AgentLifecycleEventV1::ModelProgress {
                    call_id: "model-1".to_string(),
                },
                &AgentLifecycleEventV1::ModelFinished {
                    call_id: "model-1".to_string(),
                    attempt: 0,
                    status: AgentLifecycleModelStatusV1::Failed,
                },
                &AgentLifecycleEventV1::ModelRetrying {
                    call_id: "model-1".to_string(),
                    next_attempt: 1,
                },
                &AgentLifecycleEventV1::ToolStarted {
                    call_id: "tool-1".to_string(),
                    name: "read".to_string(),
                },
                &AgentLifecycleEventV1::ToolFinished {
                    call_id: "tool-1".to_string(),
                    name: "read".to_string(),
                    status: AgentLifecycleToolStatusV1::Succeeded,
                },
                &AgentLifecycleEventV1::SteeringApplied,
                &AgentLifecycleEventV1::AgentFinished {
                    status: AgentLifecycleAgentStatusV1::Succeeded,
                },
            ]
        );
        let wire = serde_json::to_string(&*events).unwrap();
        for forbidden in ["not-forwarded", "thinking does not count"] {
            assert!(!wire.contains(forbidden));
        }
    }
}
