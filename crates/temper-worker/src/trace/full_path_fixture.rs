// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;
use std::time::Duration;

use temper_agent::activity::{AgentActivityConfig, ScopeFactory};
use temper_agent::usage::UsageTotals;
use temper_agent_core::{
    AgentEvent, AgentStop, ModelCallStatus, ModelIdentity, StreamDelta, ToolCallStatus,
    ToolResultMetadata,
};
use temper_protocol_activity::{
    AgentActivityBatch, AgentActivityCapturePolicyV1, AgentActivityEventV1, CaptureModeV1,
    PromptSnapshotV1, PromptToolDefinitionV1, StopReasonV1, W3cTraceContext,
};
use temper_protocol_agent::{
    AgentSessionState, WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem,
};
use tongs::model::{ContentBlock, StopReason, TextContent, Usage};
use tongs::provider::ToolDef;

use super::{ActivityEndpoint, TraceCollector};

const ACTIVITY_READ_POLL: Duration = Duration::from_millis(25);
const MODEL_CALL_IDLE_GAP: Duration = Duration::from_millis(100);

pub(super) const ARGUMENT_SENTINEL: &str = "ARGUMENT-BYTES-350-MUST-BE-BOUNDED";
pub(super) const MESSAGE_SENTINEL: &str = "ASSISTANT-BYTES-350-MUST-BE-BOUNDED";
pub(super) const DELTA_SENTINEL: &str = "DELTA-BYTES-350-MUST-NOT-CROSS-TRANSCRIPT";
pub(super) const MAIN_SYSTEM_PROMPT: &str = "exact full-path system prompt café";
pub(super) const MAIN_USER_PREFIX: &str = "exact full-path initial user context 🙂 ";
pub(super) const PROMPT_TOOL_DESCRIPTION: &str =
    "model-visible full-path tool description PROMPT-TOOL-BODY-364";
pub(super) const LARGE_PROMPT_REPETITIONS: usize = 1_024;

pub(super) fn full_path_policy() -> AgentActivityCapturePolicyV1 {
    AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Transcript,
        ..Default::default()
    }
}

fn prompt_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_fixture".to_string(),
            description: PROMPT_TOOL_DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "submit_fixture".to_string(),
            description: "second model-visible tool".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"summary": {"type": ["string", "null"]}}
            }),
        },
    ]
}

fn snapshot(system_prompt: &str, initial_user_message: String) -> PromptSnapshotV1 {
    PromptSnapshotV1 {
        system_prompt: Some(system_prompt.to_string()),
        initial_user_message,
        tools: prompt_tools()
            .into_iter()
            .map(|tool| PromptToolDefinitionV1 {
                name: tool.name,
                description: tool.description,
                input_schema: tool.parameters,
            })
            .collect(),
    }
}

pub(super) fn expected_main_prompt() -> PromptSnapshotV1 {
    snapshot(
        MAIN_SYSTEM_PROMPT,
        MAIN_USER_PREFIX.repeat(LARGE_PROMPT_REPETITIONS),
    )
}

pub(super) fn expected_child_prompt(display_name: &str) -> PromptSnapshotV1 {
    snapshot(
        &format!("exact {display_name} child system prompt"),
        format!("exact {display_name} child initial user message"),
    )
}

fn emit_prompt(events: &Arc<dyn temper_agent_core::EventSink>, snapshot: PromptSnapshotV1) {
    events.emit(AgentEvent::PromptPrepared {
        system_prompt: snapshot.system_prompt,
        initial_user_message: snapshot.initial_user_message,
        tools: snapshot
            .tools
            .into_iter()
            .map(|tool| ToolDef {
                name: tool.name,
                description: tool.description,
                parameters: tool.input_schema,
            })
            .collect(),
    });
}

pub(super) fn produce_first_party_run(collector: &TraceCollector) -> (String, AgentActivityBatch) {
    let run = collector
        .begin_run("job-full-path-350", &workspace_context())
        .expect("begin trace run")
        .expect("metadata capture enabled");
    let endpoint = ActivityEndpoint::bind_with_read_timeout(run.clone(), ACTIVITY_READ_POLL)
        .expect("bind child activity socket with test read poll");
    let factory = ScopeFactory::new(
        AgentActivityConfig {
            policy: full_path_policy(),
            address: Some(endpoint.address().to_string()),
            ..Default::default()
        },
        Arc::new(UsageTotals::default()),
    );
    let main = factory.main("main", ModelIdentity::new("provider", "model"));
    let events = &main.observability.events;
    emit_prompt(events, expected_main_prompt());
    events.emit(AgentEvent::TurnStart { turn: 0 });
    events.emit(AgentEvent::ModelCallStarted {
        turn: 0,
        call_id: "model-call-350".to_string(),
        attempt: 0,
        provider: "provider".to_string(),
        model: "model".to_string(),
    });
    wait_for_durable_model_start(collector);
    // Keep this factory, its ActivityClient, and their one accepted stream alive
    // across several endpoint read polls. Recreating any of them would turn the
    // capstone into a reconnect test and miss the persistent-stream regression.
    std::thread::sleep(MODEL_CALL_IDLE_GAP);
    events.emit(AgentEvent::ModelCallFinished {
        turn: 0,
        call_id: "model-call-350".to_string(),
        attempt: 0,
        status: ModelCallStatus::Succeeded,
        duration_ms: 30,
        time_to_first_token_ms: Some(7),
        stop_reason: Some(StopReason::ToolUse),
        usage: Usage::default(),
        failure: None,
    });
    events.emit(AgentEvent::TurnUsage {
        turn: 0,
        usage: Usage {
            input: 11,
            output: 5,
            cache_read: 3,
            cache_write: 2,
            ..Usage::default()
        },
    });
    events.emit(AgentEvent::AssistantMessage {
        content: vec![ContentBlock::Text(TextContent {
            text: MESSAGE_SENTINEL.to_string(),
            text_signature: None,
        })],
    });
    events.emit(AgentEvent::StreamDelta(StreamDelta::Text(
        DELTA_SENTINEL.to_string(),
    )));
    events.emit(AgentEvent::ToolStart {
        id: "tool-call-350".to_string(),
        name: "read".to_string(),
        arg_preview: Some(ARGUMENT_SENTINEL.to_string()),
        diagnostic_arguments: None,
        shell_discovery_disposition: None,
    });
    events.emit(AgentEvent::ToolEnd {
        id: "tool-call-350".to_string(),
        name: "read".to_string(),
        status: ToolCallStatus::Succeeded,
        duration_ms: 9,
        result: ToolResultMetadata {
            preview: Some(ARGUMENT_SENTINEL.to_string()),
            bytes: ARGUMENT_SENTINEL.len() as u64,
            truncated: false,
            failure: None,
            codebase_memory_timing: None,
            graph_correlation: None,
            decision_anchor_lineage: None,
        },
    });

    // Keep both child scopes alive together: this exercises the same unique
    // scope/parent behavior used by concurrent investigate/delegate calls.
    let investigate = factory.child(
        main.scope_id.clone(),
        "investigate",
        ModelIdentity::new("provider", "small-model"),
    );
    let delegate = factory.child(
        main.scope_id.clone(),
        "delegate",
        ModelIdentity::new("provider", "small-model"),
    );
    emit_prompt(
        &investigate.observability.events,
        expected_child_prompt("investigate"),
    );
    emit_prompt(
        &delegate.observability.events,
        expected_child_prompt("delegate"),
    );
    investigate.observability.events.emit(AgentEvent::AgentEnd {
        reason: AgentStop::Completed,
    });
    delegate.observability.events.emit(AgentEvent::AgentEnd {
        reason: AgentStop::Completed,
    });
    events.emit(AgentEvent::AgentEnd {
        reason: AgentStop::Completed,
    });
    drop(delegate);
    drop(investigate);
    drop(main);
    drop(factory);
    wait_for_socket_collection(collector);
    endpoint.stop();

    run.finish_success(Some(StopReasonV1::EndTurn))
        .expect("host success boundary");
    let run_id = run.run_id().to_string();
    drop(run);
    let recovered = collector.recover().expect("recover producer spool");
    let batch = recovered[0]
        .pending_batch(100)
        .expect("producer generated a forwarding batch");
    (run_id, batch)
}

fn wait_for_durable_model_start(collector: &TraceCollector) {
    for _ in 0..500 {
        if collector.recover().ok().is_some_and(|runs| {
            runs.first().is_some_and(|run| {
                run.events.iter().any(|event| {
                    matches!(
                        &event.event,
                        AgentActivityEventV1::ModelCallStarted(started)
                            if started.call_id == "model-call-350"
                    )
                })
            })
        }) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("model.call.started must be durable before the injected idle gap");
}

fn wait_for_socket_collection(collector: &TraceCollector) {
    for _ in 0..500 {
        if collector
            .recover()
            .ok()
            .and_then(|runs| runs.first().map(|run| run.events.len() == 18))
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("child socket must durably collect every pre- and post-idle frame");
}

pub(super) fn workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        trace_context: Some(W3cTraceContext {
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string(),
            tracestate: Some("temper=full-path".to_string()),
        }),
        repos: vec![WorkspaceRepository {
            id: "forgejo:ai/temper".to_string(),
            owner: "ai".to_string(),
            name: "temper".to_string(),
            default_branch: "main".to_string(),
            dir: "temper".to_string(),
            access: "writable".to_string(),
            base_branch: "feature/302-agent-session-traces".to_string(),
            branch_hint: Some("agent/pr-for-code-350".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(350) }".to_string(),
            context: serde_json::json!({
                "artifact": {"type": "issue", "number": 350}
            })
            .to_string(),
        },
        artifact_context: None,
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-350".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: Default::default(),
        pull_request_freshness: None,
        agent_session: Some(AgentSessionState::new("session-350")),
    }
}
