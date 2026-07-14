use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use temper_agent_core::{AgentEvent, ModelCallStatus, ModelIdentity, StreamDelta, ToolCallStatus};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentScopeKindV1, AgentScopeV1, CaptureModeV1, StopReasonV1,
    TurnStartedV1,
};
use tongs::model::{ContentBlock, StopReason};

use super::transport::ActivityClient;
use super::*;

struct FakeClock {
    values: Mutex<VecDeque<ActivityTimestamp>>,
}

impl FakeClock {
    fn new(elapsed: impl IntoIterator<Item = u64>) -> Self {
        Self {
            values: Mutex::new(
                elapsed
                    .into_iter()
                    .map(|elapsed_ms| ActivityTimestamp {
                        occurred_at: "2026-01-02T03:04:05.000Z".to_string(),
                        elapsed_ms,
                    })
                    .collect(),
            ),
        }
    }
}

impl ActivityClock for FakeClock {
    fn now(&self) -> ActivityTimestamp {
        self.values
            .lock()
            .expect("clock")
            .pop_front()
            .unwrap_or(ActivityTimestamp {
                occurred_at: "2026-01-02T03:04:05.000Z".to_string(),
                elapsed_ms: 999,
            })
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentActivityFrameV1>>);

impl ActivityProjection for Recorder {
    fn emit(&self, frame: &AgentActivityFrameV1) {
        self.0.lock().expect("frames").push(frame.clone());
    }
}

struct PanickingProjection;

impl ActivityProjection for PanickingProjection {
    fn emit(&self, _frame: &AgentActivityFrameV1) {
        panic!("projection failed");
    }
}

fn message(value: &str) -> AgentEvent {
    AgentEvent::AssistantMessage {
        content: vec![ContentBlock::Text(tongs::model::TextContent {
            text: value.to_string(),
            text_signature: None,
        })],
    }
}

#[test]
fn normalized_order_timing_usage_and_stop_reason_are_deterministic() {
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        Arc::new(FakeClock::new(0..30)),
        vec![recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("anthropic", "claude"));
    let sink = run.observability.events;
    sink.emit(AgentEvent::TurnStart { turn: 0 });
    sink.emit(AgentEvent::ModelCallStarted {
        turn: 0,
        call_id: "turn-0".to_string(),
        attempt: 0,
        provider: "anthropic".to_string(),
        model: "claude".to_string(),
    });
    sink.emit(AgentEvent::ModelCallFinished {
        turn: 0,
        call_id: "turn-0".to_string(),
        attempt: 0,
        status: ModelCallStatus::Succeeded,
        duration_ms: 80,
        time_to_first_token_ms: Some(25),
        stop_reason: Some(StopReason::Stop),
        usage: tongs::model::Usage {
            input: 10,
            output: 3,
            cache_read: 4,
            cache_write: 2,
            ..Default::default()
        },
        failure: None,
    });
    sink.emit(AgentEvent::TurnUsage {
        turn: 0,
        usage: tongs::model::Usage {
            input: 10,
            output: 3,
            cache_read: 4,
            cache_write: 2,
            ..Default::default()
        },
    });
    sink.emit(message("done"));
    sink.emit(AgentEvent::AgentEnd {
        reason: temper_agent_core::AgentStop::Completed,
    });

    let frames = recorder.0.lock().expect("frames");
    let kinds = frames
        .iter()
        .map(|frame| frame.event.event_type())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            "scope.started",
            "turn.started",
            "model.call.started",
            "model.call.finished",
            "usage",
            "assistant.message",
            "turn.finished",
            "scope.finished",
        ]
    );
    let AgentActivityEventV1::ModelCallFinished(finished) = &frames[3].event else {
        panic!("model finish");
    };
    assert_eq!(finished.duration_ms, 80);
    assert_eq!(finished.time_to_first_token_ms, Some(25));
    assert_eq!(finished.stop_reason, Some(StopReasonV1::EndTurn));
    let AgentActivityEventV1::Usage(usage) = &frames[4].event else {
        panic!("usage");
    };
    assert_eq!(usage.cache_read_tokens, 4);
    assert_eq!(usage.cache_write_tokens, 2);
}

#[test]
fn concurrent_children_get_unique_ids_and_the_same_correct_parent() {
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1::default(),
        Arc::new(FakeClock::new(0..20)),
        vec![recorder.clone()],
    );
    let main = factory.main("main", ModelIdentity::new("p", "m"));
    let first = factory.child(
        main.scope_id.clone(),
        "investigate",
        ModelIdentity::new("p", "small"),
    );
    let second = factory.child(
        main.scope_id.clone(),
        "investigate",
        ModelIdentity::new("p", "small"),
    );
    assert_ne!(first.scope_id, second.scope_id);
    for child in [first, second] {
        child
            .observability
            .events
            .emit(AgentEvent::TurnStart { turn: 0 });
    }
    let frames = recorder.0.lock().expect("frames");
    let children = frames
        .iter()
        .filter(|frame| frame.scope.kind == AgentScopeKindV1::SubAgent)
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 4);
    assert!(
        children
            .iter()
            .all(|frame| frame.scope.parent_id.as_deref() == Some(main.scope_id.as_str()))
    );
}

#[test]
fn metadata_excludes_content_and_all_modes_redact_and_bound() {
    let secret = format!("password=hunter2 {}", "x".repeat(40_000));
    for mode in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                max_inline_bytes: 128,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..20)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("p", "m"));
        let sink = run.observability.events;
        sink.emit(message(&secret));
        sink.emit(AgentEvent::StreamDelta(StreamDelta::Text(secret.clone())));
        sink.emit(AgentEvent::StreamDelta(StreamDelta::Thinking(
            secret.clone(),
        )));
        sink.emit(AgentEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            arg_preview: Some(secret.clone()),
        });
        sink.emit(AgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            status: ToolCallStatus::Succeeded,
            duration_ms: 3,
            result: temper_agent_core::ToolResultMetadata {
                preview: Some("stdout password=hunter2".to_string()),
                bytes: 23,
                truncated: false,
            },
        });
        sink.emit(AgentEvent::ToolStart {
            id: "tool-2".to_string(),
            name: "read".to_string(),
            arg_preview: Some("safe/path.rs".to_string()),
        });
        sink.emit(AgentEvent::ToolEnd {
            id: "tool-2".to_string(),
            name: "read".to_string(),
            status: ToolCallStatus::Succeeded,
            duration_ms: 4,
            result: temper_agent_core::ToolResultMetadata {
                preview: Some(secret.clone()),
                bytes: secret.len() as u64,
                truncated: true,
            },
        });
        let json =
            serde_json::to_string(&*recorder.0.lock().expect("frames")).expect("serialize frames");
        assert!(!json.contains("hunter2"));
        assert!(!json.contains("stdout"));
        if mode == CaptureModeV1::Metadata {
            assert!(!json.contains("assistant.message"));
            assert!(!json.contains("output.text.delta"));
            assert!(!json.contains("output.thinking.delta"));
            assert!(!json.contains("\"result\""));
        } else {
            assert!(json.len() < 5_000);
        }
        if mode == CaptureModeV1::Transcript {
            assert!(!json.contains("output.text.delta"));
            assert!(!json.contains("output.thinking.delta"));
        }
    }
}

#[test]
fn workspace_result_and_thinking_requirements_are_enforced() {
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Diagnostic,
            capture_thinking: false,
            ..Default::default()
        },
        Arc::new(FakeClock::new(0..20)),
        vec![recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("p", "m"));
    run.observability.events.emit(message(
        r#"{"title":"secret handoff","summary":"WorkspaceResult"}"#,
    ));
    run.observability
        .events
        .emit(AgentEvent::StreamDelta(StreamDelta::Thinking(
            "private chain".to_string(),
        )));
    let json = serde_json::to_string(&*recorder.0.lock().expect("frames")).unwrap();
    assert!(!json.contains("secret handoff"));
    assert!(!json.contains("private chain"));
}

#[test]
fn projection_panics_are_contained() {
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1::default(),
        Arc::new(FakeClock::new(0..20)),
        vec![Arc::new(PanickingProjection), recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("p", "m"));
    run.observability
        .events
        .emit(AgentEvent::TurnStart { turn: 0 });
    assert_eq!(recorder.0.lock().expect("frames").len(), 2);
}

#[test]
fn retries_keep_attempt_boundaries_inside_one_ordered_turn() {
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        Arc::new(FakeClock::new(0..40)),
        vec![recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("provider", "model"));
    let sink = run.observability.events;
    sink.emit(AgentEvent::TurnStart { turn: 0 });
    sink.emit(AgentEvent::ModelCallStarted {
        turn: 0,
        call_id: "turn-0".to_string(),
        attempt: 0,
        provider: "provider".to_string(),
        model: "model".to_string(),
    });
    sink.emit(AgentEvent::ModelCallFinished {
        turn: 0,
        call_id: "turn-0".to_string(),
        attempt: 0,
        status: ModelCallStatus::Failed,
        duration_ms: 20,
        time_to_first_token_ms: None,
        stop_reason: None,
        usage: Default::default(),
        failure: Some("temporary overload".to_string()),
    });
    sink.emit(AgentEvent::ModelCallRetrying {
        turn: 0,
        call_id: "turn-0".to_string(),
        next_attempt: 1,
        delay_ms: 500,
        reason: "temporary overload".to_string(),
    });
    sink.emit(AgentEvent::ModelCallStarted {
        turn: 0,
        call_id: "turn-0".to_string(),
        attempt: 1,
        provider: "provider".to_string(),
        model: "model".to_string(),
    });
    sink.emit(AgentEvent::ModelCallFinished {
        turn: 0,
        call_id: "turn-0".to_string(),
        attempt: 1,
        status: ModelCallStatus::Succeeded,
        duration_ms: 40,
        time_to_first_token_ms: Some(8),
        stop_reason: Some(StopReason::ToolUse),
        usage: Default::default(),
        failure: None,
    });
    sink.emit(message("using a tool"));
    sink.emit(AgentEvent::ToolStart {
        id: "tool-1".to_string(),
        name: "read".to_string(),
        arg_preview: Some("src/lib.rs".to_string()),
    });
    sink.emit(AgentEvent::ToolEnd {
        id: "tool-1".to_string(),
        name: "read".to_string(),
        status: ToolCallStatus::Succeeded,
        duration_ms: 6,
        result: Default::default(),
    });
    sink.emit(AgentEvent::TurnStart { turn: 1 });

    let frames = recorder.0.lock().expect("frames");
    let kinds = frames
        .iter()
        .map(|frame| frame.event.event_type())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            "scope.started",
            "turn.started",
            "model.call.started",
            "model.call.finished",
            "model.call.retrying",
            "model.call.started",
            "model.call.finished",
            "assistant.message",
            "tool.started",
            "tool.finished",
            "turn.finished",
            "turn.started",
        ]
    );
    let AgentActivityEventV1::ModelCallRetrying(retry) = &frames[4].event else {
        panic!("retry event");
    };
    assert_eq!(retry.next_attempt, 1);
    assert_eq!(retry.delay_ms, 500);
}

#[test]
fn terminal_scope_frame_gets_a_bounded_socket_flush() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
    let address = listener.local_addr().expect("listener address").to_string();
    let client = ActivityClient::new(&address);
    let frame = AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-01-02T03:04:05.000Z".to_string(),
        elapsed_ms: 9,
        scope: AgentScopeV1 {
            id: "scope".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: None,
        event: AgentActivityEventV1::ScopeFinished(temper_protocol_activity::ScopeFinishedV1 {
            status: temper_protocol_activity::ScopeStatusV1::Succeeded,
            duration_ms: 9,
        }),
    };

    client.emit(&frame);
    let (stream, _) = listener.accept().expect("accept activity client");
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .expect("read frame");
    let received: AgentActivityFrameV1 = serde_json::from_str(line.trim()).expect("parse frame");
    assert_eq!(received, frame);
}

#[test]
fn invalid_address_and_disconnected_queue_never_panic() {
    let client = ActivityClient::new("not a socket address");
    let frame = AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-01-02T03:04:05.000Z".to_string(),
        elapsed_ms: 1,
        scope: AgentScopeV1 {
            id: "scope".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(0),
        event: AgentActivityEventV1::TurnStarted(TurnStartedV1 {}),
    };
    for _ in 0..1_000 {
        client.emit(&frame);
    }
}
