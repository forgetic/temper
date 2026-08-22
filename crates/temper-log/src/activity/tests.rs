use std::sync::Arc;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityEventV1 as Event,
    AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeKindV1, AgentScopeV1,
    AgentTerminalReasonV1, CaptureModeV1, CapturedContentV1, DroppedEventKindV1, InlineContentV1,
    ModelCallFinishedV1, ModelCallStartedV1, ModelCallStatusV1, PromptCaptureDispositionV1,
    PromptPreparedV1, PromptSnapshotV1, PromptToolDefinitionV1, RunFinishedV1, RunStartedV1,
    RunStatusV1, ScopeFinishedV1, ScopeStartedV1, ScopeStatusV1, StopReasonV1,
    ToolFailureCategoryV1, ToolFailureDiagnosticV1, ToolFailureReasonV1, ToolFinishedV1,
    ToolStartedV1, ToolStatusV1, TraceGapV1, TurnFinishedV1, TurnStartedV1, UsageV1,
    W3cTraceContext,
};

use super::*;

mod model_failure;
mod tool_failure;

const PROMPT_SYSTEM_SENTINEL: &str = "LOG-PROMPT-SYSTEM-SENTINEL-364";
const PROMPT_USER_SENTINEL: &str = "LOG-PROMPT-USER-SENTINEL-364";
const PROMPT_TOOL_SENTINEL: &str = "LOG-PROMPT-TOOL-SENTINEL-364";
const PROMPT_SCHEMA_SENTINEL: &str = "LOG-PROMPT-SCHEMA-SENTINEL-364";

fn assignment() -> AgentAssignmentIdentityV1 {
    AgentAssignmentIdentityV1 {
        trace_context: Some(W3cTraceContext {
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
            tracestate: Some("vendor=opaque".into()),
        }),
        job_id: "job-313".into(),
        repository: "ai/temper".into(),
        artifact_ref: "ai/temper#313".into(),
        role: "engineer".into(),
        action: "open_pr".into(),
        correlation_key: "pr-for-code-313".into(),
    }
}

fn scope(id: &str, kind: AgentScopeKindV1, parent_id: Option<&str>) -> AgentScopeV1 {
    AgentScopeV1 {
        id: id.into(),
        kind,
        parent_id: parent_id.map(str::to_string),
    }
}

fn event(
    seq: u64,
    elapsed_ms: u64,
    scope: AgentScopeV1,
    turn: Option<u32>,
    event: Event,
) -> AgentRunEventV1 {
    AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: "run-313".into(),
        seq,
        occurred_at: format!("2026-07-13T14:28:{seq:02}.000Z"),
        elapsed_ms,
        assignment: assignment(),
        agent_session_id: Some("session-313".into()),
        scope,
        turn,
        event,
    }
}

fn prompt_prepared() -> Event {
    let snapshot = PromptSnapshotV1 {
        system_prompt: Some(PROMPT_SYSTEM_SENTINEL.to_string()),
        initial_user_message: PROMPT_USER_SENTINEL.to_string(),
        tools: vec![PromptToolDefinitionV1 {
            name: "private_prompt_tool".to_string(),
            description: PROMPT_TOOL_SENTINEL.to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"secret": {"const": PROMPT_SCHEMA_SENTINEL}}
            }),
        }],
    };
    let canonical = snapshot.to_canonical_json_bytes().expect("prompt JSON");
    let tools = snapshot
        .tools_to_canonical_json_bytes()
        .expect("tool manifest JSON");
    Event::PromptPrepared(PromptPreparedV1 {
        system_prompt_present: true,
        system_prompt_bytes: PROMPT_SYSTEM_SENTINEL.len() as u64,
        initial_user_message_bytes: PROMPT_USER_SENTINEL.len() as u64,
        tool_manifest_bytes: tools.len() as u64,
        tool_count: 1,
        original_snapshot_bytes: canonical.len() as u64,
        captured_bytes: canonical.len() as u64,
        disposition: PromptCaptureDispositionV1::Captured,
        content: Some(CapturedContentV1::Inline(InlineContentV1 {
            text: String::from_utf8(canonical).expect("prompt UTF-8"),
            truncated: false,
        })),
    })
}

fn canonical_run() -> Vec<AgentRunEventV1> {
    let main = scope("main-1", AgentScopeKindV1::Main, None);
    let mut events = vec![
        event(
            1,
            0,
            main.clone(),
            None,
            Event::RunStarted(RunStartedV1 {
                capture: CaptureModeV1::Metadata,
            }),
        ),
        event(
            2,
            5,
            main.clone(),
            None,
            Event::ScopeStarted(ScopeStartedV1 {
                display_name: Some("main".into()),
            }),
        ),
        event(
            3,
            10,
            main.clone(),
            Some(0),
            Event::TurnStarted(TurnStartedV1 {}),
        ),
        event(
            4,
            20,
            main.clone(),
            Some(0),
            Event::ModelCallStarted(ModelCallStartedV1 {
                call_id: "model-1".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet".into(),
                attempt: 0,
            }),
        ),
        event(
            5,
            120,
            main.clone(),
            Some(0),
            Event::ModelCallFinished(ModelCallFinishedV1 {
                call_id: "model-1".into(),
                attempt: 0,
                status: ModelCallStatusV1::Succeeded,
                duration_ms: 100,
                time_to_first_token_ms: Some(25),
                stop_reason: Some(StopReasonV1::ToolUse),
                failure: None,
            }),
        ),
        event(
            6,
            125,
            main.clone(),
            Some(0),
            Event::Usage(UsageV1 {
                input_tokens: 20,
                output_tokens: 5,
                cache_read_tokens: 3,
                cache_write_tokens: 1,
            }),
        ),
        event(
            7,
            130,
            main.clone(),
            Some(0),
            Event::ToolStarted(ToolStartedV1 {
                call_id: "tool-1".into(),
                name: "bash".into(),
                arguments: Some(CapturedContentV1::Inline(InlineContentV1 {
                    text: "Authorization: Bearer secret".into(),
                    truncated: false,
                })),
                shell_discovery_disposition: None,
            }),
        ),
        event(
            8,
            180,
            main.clone(),
            Some(0),
            Event::ToolFinished(ToolFinishedV1 {
                call_id: "tool-1".into(),
                name: "bash".into(),
                status: ToolStatusV1::Succeeded,
                duration_ms: 50,
                result: Some(CapturedContentV1::Inline(InlineContentV1 {
                    text: "PRIVATE_TOKEN=secret".into(),
                    truncated: false,
                })),
                failure: None,
                codebase_memory_timing: None,
                graph_correlation: None,
                decision_anchor_lineage: None,
            }),
        ),
        event(
            9,
            190,
            main.clone(),
            Some(0),
            Event::TurnFinished(TurnFinishedV1 {
                duration_ms: 180,
                stop_reason: StopReasonV1::EndTurn,
            }),
        ),
        event(
            10,
            195,
            main.clone(),
            None,
            Event::ScopeFinished(ScopeFinishedV1 {
                status: ScopeStatusV1::Succeeded,
                duration_ms: 190,
                terminal_reason: Some(AgentTerminalReasonV1::Completed),
            }),
        ),
        event(
            11,
            198,
            main.clone(),
            None,
            Event::TraceGap(TraceGapV1 {
                dropped_events: 2,
                dropped_bytes: 40,
                kinds: vec![DroppedEventKindV1::TextDelta],
            }),
        ),
        event(
            12,
            200,
            main.clone(),
            None,
            Event::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Succeeded,
                duration_ms: 200,
                stop_reason: Some(StopReasonV1::EndTurn),
            }),
        ),
    ];
    // Insert source-equivalent prompt content after scope start and before the
    // first turn. Span projection must treat this event as invisible.
    for event in events.iter_mut().skip(2) {
        event.seq += 1;
        event.occurred_at = format!("2026-07-13T14:28:{:02}.000Z", event.seq);
    }
    events.insert(2, event(3, 7, main, Some(0), prompt_prepared()));
    events
}

#[test]
fn canonical_boundaries_form_a_nested_privacy_safe_span_tree() {
    let exporter = Arc::new(InMemoryActivitySpanExporter::default());
    let mut projector = CanonicalActivityProjector::new(exporter.clone());
    projector.project_all(&canonical_run());

    let spans = exporter.finished_spans();
    assert_eq!(
        spans.iter().map(|span| span.start.kind).collect::<Vec<_>>(),
        [
            ActivitySpanKind::ModelCall,
            ActivitySpanKind::Tool,
            ActivitySpanKind::Turn,
            ActivitySpanKind::Scope,
            ActivitySpanKind::Run,
        ]
    );
    let model = &spans[0];
    assert_eq!(
        model.start.parent_span_id.as_deref(),
        Some("run-313:scope:main-1:turn:0")
    );
    assert_eq!(model.attributes.provider.as_deref(), Some("anthropic"));
    assert_eq!(model.attributes.time_to_first_token_ms, Some(25));
    assert_eq!(model.attributes.usage.input_tokens, 20);
    assert_eq!(model.attributes.usage.cache_read_tokens, 3);
    let turn = &spans[2];
    assert_eq!(turn.attributes.usage.input_tokens, 20);
    let scope = &spans[3];
    assert_eq!(
        scope.attributes.terminal_reason,
        Some(AgentTerminalReasonV1::Completed)
    );
    let run = &spans[4];
    assert_eq!(run.start.remote_parent, assignment().trace_context);
    assert_eq!(run.attributes.retry_count, 0);
    assert_eq!(run.attributes.dropped_events, 2);
    assert_eq!(run.attributes.dropped_kinds, ["text_delta"]);
    assert_eq!(run.status, ActivitySpanStatus::Ok);

    let rendered = format!("{spans:?}");
    for forbidden in [
        "Bearer secret",
        "PRIVATE_TOKEN",
        "Authorization:",
        PROMPT_SYSTEM_SENTINEL,
        PROMPT_USER_SENTINEL,
        PROMPT_TOOL_SENTINEL,
        PROMPT_SCHEMA_SENTINEL,
    ] {
        assert!(
            !rendered.contains(forbidden),
            "span projection leaked {forbidden}"
        );
    }
}

#[test]
fn every_terminal_reason_is_projected_onto_the_finished_scope_span() {
    let cases = [
        (
            ScopeStatusV1::Succeeded,
            AgentTerminalReasonV1::Completed,
            ActivitySpanStatus::Ok,
        ),
        (
            ScopeStatusV1::Failed,
            AgentTerminalReasonV1::ModelError,
            ActivitySpanStatus::Error,
        ),
        (
            ScopeStatusV1::Cancelled,
            AgentTerminalReasonV1::Aborted,
            ActivitySpanStatus::Cancelled,
        ),
        (
            ScopeStatusV1::Failed,
            AgentTerminalReasonV1::BudgetExhausted,
            ActivitySpanStatus::Error,
        ),
    ];

    for (scope_status, terminal_reason, span_status) in cases {
        let exporter = Arc::new(InMemoryActivitySpanExporter::default());
        let mut projector = CanonicalActivityProjector::new(exporter.clone());
        let main = scope("main-1", AgentScopeKindV1::Main, None);
        projector.project_all(&[
            event(
                1,
                0,
                main.clone(),
                None,
                Event::ScopeStarted(ScopeStartedV1 {
                    display_name: Some("main".into()),
                }),
            ),
            event(
                2,
                10,
                main,
                None,
                Event::ScopeFinished(ScopeFinishedV1 {
                    status: scope_status,
                    duration_ms: 10,
                    terminal_reason: Some(terminal_reason),
                }),
            ),
        ]);

        let spans = exporter.finished_spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].status, span_status);
        assert_eq!(spans[0].attributes.terminal_reason, Some(terminal_reason));
    }
}

#[test]
fn parallel_sub_agent_scopes_keep_unique_ids_and_parentage() {
    let exporter = Arc::new(InMemoryActivitySpanExporter::default());
    let mut projector = CanonicalActivityProjector::new(exporter.clone());
    let main = scope("main-1", AgentScopeKindV1::Main, None);
    let child_a = scope("investigate-a", AgentScopeKindV1::SubAgent, Some("main-1"));
    let child_b = scope("investigate-b", AgentScopeKindV1::SubAgent, Some("main-1"));
    let events = vec![
        event(
            1,
            0,
            main.clone(),
            None,
            Event::RunStarted(RunStartedV1 {
                capture: CaptureModeV1::Metadata,
            }),
        ),
        event(
            2,
            1,
            main.clone(),
            None,
            Event::ScopeStarted(ScopeStartedV1 {
                display_name: Some("main".into()),
            }),
        ),
        event(
            3,
            2,
            child_a.clone(),
            None,
            Event::ScopeStarted(ScopeStartedV1 {
                display_name: Some("investigate".into()),
            }),
        ),
        event(
            4,
            3,
            child_b.clone(),
            None,
            Event::ScopeStarted(ScopeStartedV1 {
                display_name: Some("investigate".into()),
            }),
        ),
        event(
            5,
            8,
            child_b,
            None,
            Event::ScopeFinished(ScopeFinishedV1 {
                status: ScopeStatusV1::Succeeded,
                duration_ms: 5,
                terminal_reason: Some(AgentTerminalReasonV1::Completed),
            }),
        ),
        event(
            6,
            9,
            child_a,
            None,
            Event::ScopeFinished(ScopeFinishedV1 {
                status: ScopeStatusV1::Succeeded,
                duration_ms: 7,
                terminal_reason: Some(AgentTerminalReasonV1::Completed),
            }),
        ),
        event(
            7,
            10,
            main.clone(),
            None,
            Event::ScopeFinished(ScopeFinishedV1 {
                status: ScopeStatusV1::Succeeded,
                duration_ms: 9,
                terminal_reason: Some(AgentTerminalReasonV1::Completed),
            }),
        ),
        event(
            8,
            11,
            main,
            None,
            Event::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Succeeded,
                duration_ms: 11,
                stop_reason: Some(StopReasonV1::EndTurn),
            }),
        ),
    ];
    projector.project_all(&events);

    let children = exporter
        .finished_spans()
        .into_iter()
        .filter(|span| span.start.kind == ActivitySpanKind::Scope)
        .filter(|span| span.start.attributes.parent_scope_id.is_some())
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert_ne!(children[0].start.span_id, children[1].start.span_id);
    assert!(
        children
            .iter()
            .all(|span| { span.start.parent_span_id.as_deref() == Some("run-313:scope:main-1") })
    );
}

#[test]
fn replay_is_idempotent_and_separate_runs_keep_distinct_roots() {
    let exporter = Arc::new(InMemoryActivitySpanExporter::default());
    let mut projector = CanonicalActivityProjector::new(exporter.clone());
    let events = canonical_run();
    projector.project_all(&events);
    projector.project_all(&events);
    assert_eq!(exporter.finished_spans().len(), 5);

    let mut second = events;
    for event in &mut second {
        event.run_id = "run-314".into();
        event.assignment.correlation_key = "pr-for-code-313".into();
    }
    projector.project_all(&second);
    let roots = exporter
        .finished_spans()
        .into_iter()
        .filter(|span| span.start.kind == ActivitySpanKind::Run)
        .map(|span| span.start.span_id)
        .collect::<Vec<_>>();
    assert_eq!(roots, ["run-313:run", "run-314:run"]);
}

struct PanickingExporter;
impl ActivitySpanExporter for PanickingExporter {
    fn span_started(&self, _span: &ActivitySpanStart) {
        panic!("start exporter failed");
    }

    fn span_finished(&self, _span: ProjectedActivitySpan) {
        panic!("finish exporter failed");
    }
}

#[test]
fn exporter_failures_never_escape_projection() {
    let mut projector = CanonicalActivityProjector::new(Arc::new(PanickingExporter));
    projector.project_all(&canonical_run());
}

#[test]
fn default_capture_policy_remains_metadata_only() {
    let policy = AgentActivityCapturePolicyV1::default();
    assert_eq!(policy.capture, CaptureModeV1::Metadata);
    assert!(!policy.capture_thinking);
}

#[cfg(feature = "otel")]
#[test]
fn tracing_bridge_exports_nested_w3c_parented_privacy_safe_spans() {
    use opentelemetry::trace::{Status, TracerProvider as _};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing_subscriber::prelude::*;

    let otel_exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(otel_exporter.clone())
        .build();
    let tracer = provider.tracer("temper-activity-test");
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_location(false),
    );

    tracing::subscriber::with_default(subscriber, || {
        let mut projector =
            CanonicalActivityProjector::new(Arc::new(TracingActivitySpanExporter::default()));
        projector.project_all(&canonical_run());
    });

    provider.force_flush().unwrap();
    let spans = otel_exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 5);
    let by_name = |name: &str| {
        spans
            .iter()
            .find(|span| span.name == name)
            .unwrap_or_else(|| panic!("missing {name} span"))
    };
    let run = by_name("agent.run");
    let scope = by_name("agent.scope");
    let turn = by_name("agent.turn");
    let model = by_name("llm.call");
    let tool = by_name("tool.call");

    assert_eq!(
        run.span_context.trace_id().to_string(),
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert_eq!(run.parent_span_id.to_string(), "00f067aa0ba902b7");
    assert!(run.parent_span_is_remote);
    assert_eq!(scope.parent_span_id, run.span_context.span_id());
    assert_eq!(turn.parent_span_id, scope.span_context.span_id());
    assert_eq!(model.parent_span_id, turn.span_context.span_id());
    assert_eq!(tool.parent_span_id, turn.span_context.span_id());
    assert_eq!(run.status, Status::Ok);
    assert_eq!(model.status, Status::Ok);

    let model_attributes = format!("{:?}", model.attributes);
    assert!(model_attributes.contains("event.started_at"));
    assert!(model_attributes.contains("duration_ms"));
    assert!(model_attributes.contains("gen_ai.provider.name"));
    assert!(model_attributes.contains("usage.input_tokens"));
    let scope_attributes = format!("{:?}", scope.attributes);
    assert!(scope_attributes.contains("agent.terminal_reason"));
    assert!(scope_attributes.contains("completed"));
    let rendered = format!("{spans:?}");
    for forbidden in [
        "Bearer secret",
        "PRIVATE_TOKEN",
        "Authorization:",
        PROMPT_SYSTEM_SENTINEL,
        PROMPT_USER_SENTINEL,
        PROMPT_TOOL_SENTINEL,
        PROMPT_SCHEMA_SENTINEL,
    ] {
        assert!(
            !rendered.contains(forbidden),
            "OTel span leaked {forbidden}"
        );
    }
}
