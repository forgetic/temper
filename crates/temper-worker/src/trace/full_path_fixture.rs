// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use temper_agent::activity::{AgentActivityConfig, ScopeFactory};
use temper_agent::usage::UsageTotals;
use temper_agent_core::{
    AgentEvent, AgentStop, ModelCallStatus, ModelIdentity, StreamDelta, ToolCallStatus,
    ToolResultMetadata,
};
use temper_protocol_activity::{
    AgentActivityBatch, AgentActivityCapturePolicyV1, StopReasonV1, W3cTraceContext,
};
use temper_protocol_agent::{
    AgentSessionState, WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem,
};
use tongs::model::{ContentBlock, StopReason, TextContent, Usage};

use super::TraceCollector;

pub(super) const ARGUMENT_SENTINEL: &str = "ARGUMENT-BYTES-350-MUST-NOT-CROSS-METADATA";
pub(super) const MESSAGE_SENTINEL: &str = "ASSISTANT-BYTES-350-MUST-NOT-CROSS-METADATA";
pub(super) const DELTA_SENTINEL: &str = "DELTA-BYTES-350-MUST-NOT-CROSS-METADATA";

pub(super) fn produce_first_party_run(collector: &TraceCollector) -> (String, AgentActivityBatch) {
    let run = collector
        .begin_run("job-full-path-350", &workspace_context())
        .expect("begin trace run")
        .expect("metadata capture enabled");
    let endpoint = run.bind_endpoint().expect("bind child activity socket");
    let factory = ScopeFactory::new(
        AgentActivityConfig {
            policy: AgentActivityCapturePolicyV1::default(),
            address: Some(endpoint.address().to_string()),
        },
        Arc::new(UsageTotals::default()),
    );
    let main = factory.main("main", ModelIdentity::new("provider", "model"));
    let events = &main.observability.events;
    events.emit(AgentEvent::TurnStart { turn: 0 });
    events.emit(AgentEvent::ModelCallStarted {
        turn: 0,
        call_id: "model-call-350".to_string(),
        attempt: 0,
        provider: "provider".to_string(),
        model: "model".to_string(),
    });
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
        },
    });

    let child = factory.child(
        main.scope_id.clone(),
        "investigate",
        ModelIdentity::new("provider", "small-model"),
    );
    child.observability.events.emit(AgentEvent::AgentEnd {
        reason: AgentStop::Completed,
    });
    events.emit(AgentEvent::AgentEnd {
        reason: AgentStop::Completed,
    });
    drop(child);
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

fn wait_for_socket_collection(collector: &TraceCollector) {
    for _ in 0..200 {
        if collector
            .recover()
            .ok()
            .and_then(|runs| runs.first().map(|run| run.events.len() == 12))
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("child socket must durably collect every normalized frame");
}

fn workspace_context() -> WorkspaceContext {
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
