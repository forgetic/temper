//! Shared fixtures and drive helpers for the [`AgentMachine`] unit tests.
//!
//! Each test feeds a synthetic completion sequence and asserts on the emitted
//! requests — the call/tool/stop cycle the pi loop hides behind async/await is
//! here a plain, replayable function from `(state, completion)` to `[request]`.

use std::collections::BTreeMap;

use temper_agent_io::{EngineTime, Machine};
use tongs::model::{
    AssistantMessage, ContentBlock, Message, StopReason, TextContent, ToolCall, Usage, UserContent,
    UserMessage,
};
use tongs::tools::{ToolEffects, ToolOutput};

use crate::ModelFailureDiagnostic;
use crate::machine::{AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop};

/// A machine whose named tools are all read-only (parallel-safe), so adjacent
/// tool calls batch together and run concurrently.
pub(super) fn machine_read_tools(names: &[&str]) -> AgentMachine {
    let effects: BTreeMap<String, ToolEffects> = names
        .iter()
        .map(|name| ((*name).to_string(), ToolEffects::read()))
        .collect();
    AgentMachine::with_effects(vec![user("do the thing")], 10, effects)
}

pub(super) fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
    })
}

pub(super) fn assistant_text(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        api: "test".to_string(),
        provider: "test".to_string(),
        model: "test".to_string(),
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

pub(super) fn assistant_tool_calls(calls: &[(&str, &str)]) -> AssistantMessage {
    AssistantMessage {
        content: calls
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolCall(ToolCall {
                    id: (*id).to_string(),
                    name: (*name).to_string(),
                    arguments: serde_json::json!({}),
                })
            })
            .collect(),
        api: "test".to_string(),
        provider: "test".to_string(),
        model: "test".to_string(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    }
}

/// An assistant turn requesting a single tool call with explicit JSON arguments
/// (the `assistant_tool_calls` helper always uses empty `{}` args).
pub(super) fn assistant_tool_call_with_args(
    id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::ToolCall(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
        })],
        api: "test".to_string(),
        provider: "test".to_string(),
        model: "test".to_string(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    }
}

/// The `arg_preview` of every emitted `ToolStart`, in order.
pub(super) fn tool_start_previews(requests: &[AgentRequest]) -> Vec<Option<String>> {
    requests
        .iter()
        .filter_map(|r| match r {
            AgentRequest::Emit(AgentEvent::ToolStart { arg_preview, .. }) => {
                Some(arg_preview.clone())
            }
            _ => None,
        })
        .collect()
}

pub(super) fn assistant_error() -> AssistantMessage {
    let mut message = assistant_text("boom");
    message.stop_reason = StopReason::Error;
    message.error_message = Some("provider error".to_string());
    message
}

pub(super) fn tool_output(text: &str, is_error: bool) -> ToolOutput {
    ToolOutput {
        content: vec![ContentBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        details: None,
        is_error,
    }
}

pub(super) fn machine() -> AgentMachine {
    AgentMachine::new(vec![user("do the thing")], 10)
}

pub(super) enum TestCompletion {
    LlmResponded(AssistantMessage),
    LlmFailed(ModelFailureDiagnostic),
    ToolFinished { id: String, output: ToolOutput },
}

pub(super) fn llm_responded(message: AssistantMessage) -> TestCompletion {
    TestCompletion::LlmResponded(message)
}

pub(super) fn llm_failed(_message: impl Into<String>) -> TestCompletion {
    TestCompletion::LlmFailed(ModelFailureDiagnostic::redacted_unknown(
        "test-provider",
        "test-model",
        false,
    ))
}

pub(super) fn tool_finished(id: impl Into<String>, output: ToolOutput) -> TestCompletion {
    TestCompletion::ToolFinished {
        id: id.into(),
        output,
    }
}

/// Deliver a synthetic completion stamped with the operation identity the
/// machine most recently requested.
pub(super) fn complete(m: &mut AgentMachine, completion: TestCompletion) -> Vec<AgentRequest> {
    let completion = match completion {
        TestCompletion::LlmResponded(message) => {
            let (operation_generation, batch_generation) =
                m.active_generations().expect("active model operation");
            AgentCompletion::LlmResponded {
                operation_generation,
                batch_generation,
                message,
            }
        }
        TestCompletion::LlmFailed(diagnostic) => {
            let (operation_generation, batch_generation) =
                m.active_generations().expect("active model operation");
            AgentCompletion::LlmFailed {
                operation_generation,
                batch_generation,
                diagnostic,
            }
        }
        TestCompletion::ToolFinished { id, output } => {
            let (operation_generation, batch_generation) = m
                .active_tool_generations(&id)
                .expect("active tool operation");
            AgentCompletion::ToolFinished {
                operation_generation,
                batch_generation,
                id,
                output,
            }
        }
    };
    m.on_completion(EngineTime::ZERO, completion)
}

/// Drive the machine over a completion sequence, returning all emitted requests.
pub(super) fn run(m: &mut AgentMachine, completions: Vec<TestCompletion>) -> Vec<AgentRequest> {
    let mut requests = m.on_start(EngineTime::ZERO);
    for completion in completions {
        if m.is_stopped() {
            break;
        }
        requests.extend(complete(m, completion));
    }
    requests
}

pub(super) fn calls_llm(requests: &[AgentRequest]) -> usize {
    requests
        .iter()
        .filter(|r| matches!(r, AgentRequest::CallLlm { .. }))
        .count()
}

pub(super) fn run_tools(requests: &[AgentRequest]) -> Vec<String> {
    requests
        .iter()
        .filter_map(|r| match r {
            AgentRequest::RunTool { call, .. } => Some(call.id.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn final_stop(requests: &[AgentRequest]) -> Option<AgentStop> {
    requests.iter().find_map(|r| match r {
        AgentRequest::Finished { stop, .. } => Some(*stop),
        _ => None,
    })
}
