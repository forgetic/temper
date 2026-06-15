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

use crate::machine::{AgentCompletion, AgentMachine, AgentRequest, AgentStop};

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

/// Drive the machine over a completion sequence, returning all emitted requests.
pub(super) fn run(m: &mut AgentMachine, completions: Vec<AgentCompletion>) -> Vec<AgentRequest> {
    let mut requests = m.on_start(EngineTime::ZERO);
    for completion in completions {
        if m.is_stopped() {
            break;
        }
        requests.extend(m.on_completion(EngineTime::ZERO, completion));
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
            AgentRequest::RunTool(call) => Some(call.id.clone()),
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
