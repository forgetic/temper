use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use temper_agent_core::{
    AgentEvent, ModelCallStatus, ModelFailureDiagnostic, ModelIdentity, StreamDelta, ToolCallStatus,
};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityChildRecordV1,
    AgentActivityEventV1, AgentActivityFrameV1, AgentScopeKindV1, AgentScopeV1,
    AgentTerminalReasonV1, CaptureModeV1, CapturedContentV1, MODEL_CALL_RETRY_FAILURE_MESSAGE,
    ScopeStatusV1, StopReasonV1, TurnStartedV1,
};
use tongs::model::{ContentBlock, StopReason};
use tongs::{FailureCategory, ProviderFailureDiagnostic};

use super::transport::ActivityClient;
use super::*;

mod terminal;

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
    fn emit(&self, record: &AgentActivityChildRecordV1) {
        self.0.lock().expect("frames").push(record.frame.clone());
    }
}

struct PanickingProjection;

impl ActivityProjection for PanickingProjection {
    fn emit(&self, _record: &AgentActivityChildRecordV1) {
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
    let AgentActivityEventV1::ScopeFinished(finished) = &frames[7].event else {
        panic!("scope finish");
    };
    assert_eq!(finished.status, ScopeStatusV1::Succeeded);
    assert_eq!(
        finished.terminal_reason,
        Some(AgentTerminalReasonV1::Completed)
    );
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
fn metadata_keeps_tool_start_identity_without_any_argument_bytes() {
    const ARGUMENT: &str = "ARGUMENT-BYTES-350-keep-out-of-metadata";

    for mode in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..10)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        run.observability.events.emit(AgentEvent::ToolStart {
            id: "call-350".to_string(),
            name: "read".to_string(),
            arg_preview: Some(ARGUMENT.to_string()),
        });

        let frames = recorder.0.lock().expect("frames");
        let started = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ToolStarted(started) => Some(started),
                _ => None,
            })
            .expect("tool.started boundary");
        assert_eq!(started.call_id, "call-350");
        assert_eq!(started.name, "read");
        if mode == CaptureModeV1::Metadata {
            assert_eq!(started.arguments, None);
            let wire = serde_json::to_vec(&*frames).expect("serialize metadata frames");
            assert!(
                !wire
                    .windows(ARGUMENT.len())
                    .any(|bytes| bytes == ARGUMENT.as_bytes())
            );
            assert!(!String::from_utf8(wire).unwrap().contains("arguments"));
        } else {
            let CapturedContentV1::Inline(arguments) =
                started.arguments.as_ref().expect("captured arguments")
            else {
                panic!("tool arguments should remain bounded inline content");
            };
            assert_eq!(arguments.text, ARGUMENT);
            assert!(!arguments.truncated);
        }
    }
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
fn provider_retry_diagnostics_are_fixed_in_every_capture_mode() {
    const SENTINELS: [&str; 4] = [
        "CREDENTIAL-RETRY-SENTINEL-355",
        "HEADER-RETRY-SENTINEL-355",
        "ENVIRONMENT-RETRY-SENTINEL-355",
        "PROVIDER-RESPONSE-RETRY-SENTINEL-355",
    ];
    let upstream = ProviderFailureDiagnostic::new(
        FailureCategory::Provider,
        true,
        None,
        None,
        None,
        &SENTINELS.join(" "),
    );
    let diagnostics =
        ModelFailureDiagnostic::from_provider(&ModelIdentity::new("provider", "model"), &upstream);

    for mode in [
        CaptureModeV1::Off,
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                capture_thinking: mode == CaptureModeV1::Diagnostic,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..10)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        run.observability
            .events
            .emit(AgentEvent::ModelCallRetrying {
                turn: 3,
                call_id: "model-call-355".to_string(),
                next_attempt: 4,
                delay_ms: 750,
                reason: diagnostics.clone(),
            });

        let frames = recorder.0.lock().expect("frames");
        let retry = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ModelCallRetrying(retry) => Some(retry),
                _ => None,
            })
            .expect("retry boundary");
        assert_eq!(retry.call_id, "model-call-355");
        assert_eq!(retry.next_attempt, 4);
        assert_eq!(retry.delay_ms, 750);
        assert_eq!(
            retry.failure.code,
            temper_protocol_activity::FailureCodeV1::Provider
        );
        assert!(retry.failure.retryable);
        assert_eq!(retry.failure.message, MODEL_CALL_RETRY_FAILURE_MESSAGE);
        let wire = serde_json::to_string(&*frames).expect("serialize retry frames");
        for sentinel in SENTINELS {
            assert!(!wire.contains(sentinel), "{mode:?} leaked {sentinel}");
        }
    }
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
        failure: Some(ModelFailureDiagnostic::redacted_unknown(
            "provider", "model", true,
        )),
    });
    sink.emit(AgentEvent::ModelCallRetrying {
        turn: 0,
        call_id: "turn-0".to_string(),
        next_attempt: 1,
        delay_ms: 500,
        reason: ModelFailureDiagnostic::redacted_unknown("provider", "model", true),
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
            terminal_reason: Some(AgentTerminalReasonV1::Completed),
        }),
    };

    client.emit(&AgentActivityChildRecordV1 {
        frame: frame.clone(),
        blobs: Vec::new(),
    });
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
        client.emit(&AgentActivityChildRecordV1 {
            frame: frame.clone(),
            blobs: Vec::new(),
        });
    }
}
