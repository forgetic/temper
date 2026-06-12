//! Deterministic, runtime-free unit tests for [`AgentMachine`].
//!
//! Each test feeds a synthetic completion sequence and asserts on the emitted
//! requests — the call/tool/stop cycle the pi loop hides behind async/await is
//! here a plain, replayable function from `(state, completion)` to `[request]`.

use std::collections::BTreeMap;
use std::sync::Arc;

use pi::model::{
    AssistantMessage, ContentBlock, Message, StopReason, TextContent, ToolCall, UserContent,
    UserMessage, Usage,
};
use pi::tools::{ToolEffects, ToolOutput};
use smith_io_engine::{EngineTime, Machine};

use super::{AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop};

/// A machine whose named tools are all read-only (parallel-safe), so adjacent
/// tool calls batch together and run concurrently.
fn machine_read_tools(names: &[&str]) -> AgentMachine {
    let effects: BTreeMap<String, ToolEffects> = names
        .iter()
        .map(|name| ((*name).to_string(), ToolEffects::read()))
        .collect();
    AgentMachine::with_effects(vec![user("do the thing")], 10, effects)
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
    })
}

fn assistant_text(text: &str) -> AssistantMessage {
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

fn assistant_tool_calls(calls: &[(&str, &str)]) -> AssistantMessage {
    AssistantMessage {
        content: calls
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolCall(ToolCall {
                    id: (*id).to_string(),
                    name: (*name).to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
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

fn assistant_error() -> AssistantMessage {
    let mut message = assistant_text("boom");
    message.stop_reason = StopReason::Error;
    message.error_message = Some("provider error".to_string());
    message
}

fn tool_output(text: &str, is_error: bool) -> ToolOutput {
    ToolOutput {
        content: vec![ContentBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        details: None,
        is_error,
    }
}

fn machine() -> AgentMachine {
    AgentMachine::new(vec![user("do the thing")], 10)
}

/// Drive the machine over a completion sequence, returning all emitted requests.
fn run(m: &mut AgentMachine, completions: Vec<AgentCompletion>) -> Vec<AgentRequest> {
    let mut requests = m.on_start(EngineTime::ZERO);
    for completion in completions {
        if m.is_stopped() {
            break;
        }
        requests.extend(m.on_completion(EngineTime::ZERO, completion));
    }
    requests
}

fn calls_llm(requests: &[AgentRequest]) -> usize {
    requests
        .iter()
        .filter(|r| matches!(r, AgentRequest::CallLlm { .. }))
        .count()
}

fn run_tools(requests: &[AgentRequest]) -> Vec<String> {
    requests
        .iter()
        .filter_map(|r| match r {
            AgentRequest::RunTool(call) => Some(call.id.clone()),
            _ => None,
        })
        .collect()
}

fn final_stop(requests: &[AgentRequest]) -> Option<AgentStop> {
    requests.iter().find_map(|r| match r {
        AgentRequest::Finished { stop, .. } => Some(*stop),
        _ => None,
    })
}

#[test]
fn on_start_calls_the_model_once() {
    let mut m = machine();
    let requests = m.on_start(EngineTime::ZERO);
    assert_eq!(calls_llm(&requests), 1);
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, AgentRequest::Emit(AgentEvent::TurnStart { turn: 0 }))),
        "expected a turn-start event: {:?}",
        requests.len()
    );
}

#[test]
fn text_only_response_completes_without_tools() {
    let mut m = machine();
    let requests = run(
        &mut m,
        vec![AgentCompletion::LlmResponded(assistant_text("all done"))],
    );
    assert_eq!(final_stop(&requests), Some(AgentStop::Completed));
    assert!(run_tools(&requests).is_empty());
    assert!(m.is_stopped());
}

#[test]
fn single_tool_round_then_completes() {
    let mut m = machine();
    let requests = run(
        &mut m,
        vec![
            AgentCompletion::LlmResponded(assistant_tool_calls(&[("call-1", "read")])),
            AgentCompletion::ToolFinished {
                id: "call-1".to_string(),
                output: tool_output("file contents", false),
            },
            AgentCompletion::LlmResponded(assistant_text("done after reading")),
        ],
    );
    // One tool dispatched, the model called twice (initial + after tool), and a
    // clean completion.
    assert_eq!(run_tools(&requests), vec!["call-1".to_string()]);
    assert_eq!(calls_llm(&requests), 2);
    assert_eq!(final_stop(&requests), Some(AgentStop::Completed));
    // The conversation ends with: user, assistant(toolcall), toolresult,
    // assistant(text). 4 messages.
    // (messages() is drained on finish, so check via the Finished payload.)
}

#[test]
fn parallel_batch_runs_concurrently_and_waits_for_all_before_next_call() {
    // Two read-only tools ⇒ one parallel batch: both dispatched at once, and the
    // model is not re-called until BOTH finish.
    let mut m = machine_read_tools(&["read", "grep"]);
    let mut requests = m.on_start(EngineTime::ZERO);
    requests.extend(m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmResponded(assistant_tool_calls(&[("a", "read"), ("b", "grep")])),
    ));
    // Both run together (one batch).
    assert_eq!(run_tools(&requests), vec!["a".to_string(), "b".to_string()]);

    // First tool finishes — must NOT call the model yet (batch incomplete).
    let after_first = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "a".to_string(),
            output: tool_output("a out", false),
        },
    );
    assert_eq!(
        calls_llm(&after_first),
        0,
        "must wait for the whole batch before re-calling the model"
    );
    // No second batch dispatched either — they were in the same batch.
    assert!(run_tools(&after_first).is_empty());

    // Second tool finishes — now the model is called again.
    let after_second = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "b".to_string(),
            output: tool_output("b out", false),
        },
    );
    assert_eq!(calls_llm(&after_second), 1);
}

#[test]
fn model_error_stops_immediately() {
    let mut m = machine();
    let requests = run(
        &mut m,
        vec![AgentCompletion::LlmResponded(assistant_error())],
    );
    assert_eq!(final_stop(&requests), Some(AgentStop::ModelError));
    assert!(m.is_stopped());
}

#[test]
fn transport_failure_stops_with_model_error() {
    let mut m = machine();
    let requests = run(
        &mut m,
        vec![AgentCompletion::LlmFailed("connection reset".to_string())],
    );
    assert_eq!(final_stop(&requests), Some(AgentStop::ModelError));
}

#[test]
fn iteration_budget_is_enforced() {
    // Budget of 2 tool rounds: the model keeps asking for tools forever.
    let mut m = AgentMachine::new(vec![user("loop")], 2);
    let mut requests = m.on_start(EngineTime::ZERO);
    let mut round = 0;
    while !m.is_stopped() && round < 10 {
        // model asks for a tool
        requests.extend(m.on_completion(
            EngineTime::ZERO,
            AgentCompletion::LlmResponded(assistant_tool_calls(&[("c", "read")])),
        ));
        if m.is_stopped() {
            break;
        }
        // tool finishes
        requests.extend(m.on_completion(
            EngineTime::ZERO,
            AgentCompletion::ToolFinished {
                id: "c".to_string(),
                output: tool_output("again", false),
            },
        ));
        round += 1;
    }
    assert_eq!(final_stop(&requests), Some(AgentStop::BudgetExhausted));
    // The model was called at most budget+1 times (initial + 2 rounds), never
    // unboundedly.
    assert!(calls_llm(&requests) <= 3, "called the model too many times");
}

#[test]
fn abort_between_turns_stops() {
    let mut m = machine();
    m.on_start(EngineTime::ZERO);
    // Model responded with text-less tool? No — abort while awaiting LLM.
    let requests = m.on_completion(EngineTime::ZERO, AgentCompletion::Abort);
    assert_eq!(final_stop(&requests), Some(AgentStop::Aborted));
    assert!(m.is_stopped());
}

#[test]
fn abort_during_tools_drains_the_batch_then_stops() {
    let mut m = machine();
    m.on_start(EngineTime::ZERO);
    m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmResponded(assistant_tool_calls(&[("t", "bash")])),
    );
    // Abort arrives mid-batch: the machine must not stop until the in-flight
    // tool drains (no torn tool-result state).
    let mid = m.on_completion(EngineTime::ZERO, AgentCompletion::Abort);
    assert_eq!(final_stop(&mid), None, "must not stop mid-tool-batch");
    assert!(!m.is_stopped());

    let after = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "t".to_string(),
            output: tool_output("done", false),
        },
    );
    assert_eq!(final_stop(&after), Some(AgentStop::Aborted));
    assert!(m.is_stopped());
}

#[test]
fn steering_is_injected_at_the_next_turn_boundary() {
    let mut m = machine();
    m.on_start(EngineTime::ZERO);
    // A tool round is in flight.
    m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmResponded(assistant_tool_calls(&[("s", "read")])),
    );
    // Steering arrives mid-round — queued, not applied yet.
    let steered = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::Steer(vec![user("actually, also check the logs")]),
    );
    assert!(
        !steered
            .iter()
            .any(|r| matches!(r, AgentRequest::Emit(AgentEvent::Steered { .. }))),
        "steering must wait for the turn boundary"
    );
    // Tool finishes ⇒ turn boundary ⇒ steering injected + model re-called.
    let after = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "s".to_string(),
            output: tool_output("read", false),
        },
    );
    assert!(
        after
            .iter()
            .any(|r| matches!(r, AgentRequest::Emit(AgentEvent::Steered { count: 1 }))),
        "steering should be injected at the turn boundary: {:?}",
        after.len()
    );
    assert_eq!(calls_llm(&after), 1);
}

#[test]
fn final_conversation_has_tool_results_in_order() {
    // Two read-only tools run in one parallel batch; results arrive out of order
    // (y before x) but the tool-result messages must still be appended in
    // original tool-call order (x before y).
    let mut m = machine_read_tools(&["read", "grep"]);
    let requests = run(
        &mut m,
        vec![
            AgentCompletion::LlmResponded(assistant_tool_calls(&[("x", "read"), ("y", "grep")])),
            AgentCompletion::ToolFinished {
                id: "y".to_string(),
                output: tool_output("y", false),
            },
            AgentCompletion::ToolFinished {
                id: "x".to_string(),
                output: tool_output("x", false),
            },
            AgentCompletion::LlmResponded(assistant_text("done")),
        ],
    );
    let messages = requests
        .iter()
        .find_map(|r| match r {
            AgentRequest::Finished { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .expect("a finished payload");

    // Tool-result messages appear in tool-call order (x before y), regardless of
    // the order results arrived.
    let tool_result_ids: Vec<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(result) => Some(result.tool_call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_result_ids, vec!["x".to_string(), "y".to_string()]);
    let _ = Arc::new(());
}

#[test]
fn mixed_effects_serialize_into_ordered_batches() {
    // read, write, read ⇒ three serialized batches (a write is a barrier): the
    // machine dispatches one call at a time, in order, never two at once.
    let mut effects = BTreeMap::new();
    effects.insert("read".to_string(), ToolEffects::read());
    effects.insert("write".to_string(), ToolEffects::write());
    let mut m = AgentMachine::with_effects(vec![user("mix")], 10, effects);

    let mut requests = m.on_start(EngineTime::ZERO);
    requests.extend(m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmResponded(assistant_tool_calls(&[
            ("r1", "read"),
            ("w", "write"),
            ("r2", "read"),
        ])),
    ));
    // Only the first batch (r1) is dispatched.
    assert_eq!(run_tools(&requests), vec!["r1".to_string()]);

    // r1 finishes ⇒ dispatch the write batch (w), still no model call.
    let after_r1 = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "r1".to_string(),
            output: tool_output("r1", false),
        },
    );
    assert_eq!(run_tools(&after_r1), vec!["w".to_string()]);
    assert_eq!(calls_llm(&after_r1), 0);

    // w finishes ⇒ dispatch the last read batch (r2).
    let after_w = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "w".to_string(),
            output: tool_output("w", false),
        },
    );
    assert_eq!(run_tools(&after_w), vec!["r2".to_string()]);
    assert_eq!(calls_llm(&after_w), 0);

    // r2 finishes ⇒ all batches done ⇒ the model is re-called.
    let after_r2 = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "r2".to_string(),
            output: tool_output("r2", false),
        },
    );
    assert_eq!(calls_llm(&after_r2), 1);
}

#[test]
fn unknown_tools_are_serialized_fail_closed() {
    // With no effect declarations (the default `machine()`), every tool is
    // treated as a write ⇒ each is its own serial batch. Two calls ⇒ the second
    // is not dispatched until the first finishes.
    let mut m = machine();
    let mut requests = m.on_start(EngineTime::ZERO);
    requests.extend(m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmResponded(assistant_tool_calls(&[("a", "mystery"), ("b", "mystery")])),
    ));
    assert_eq!(run_tools(&requests), vec!["a".to_string()]);

    let after_a = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            id: "a".to_string(),
            output: tool_output("a", false),
        },
    );
    assert_eq!(run_tools(&after_a), vec!["b".to_string()]);
    assert_eq!(calls_llm(&after_a), 0);
}

#[test]
fn results_across_serial_batches_preserve_original_order() {
    // read, write ⇒ two batches; even though they run in sequence, the appended
    // tool-result messages keep original order (r before w).
    let mut effects = BTreeMap::new();
    effects.insert("read".to_string(), ToolEffects::read());
    effects.insert("write".to_string(), ToolEffects::write());
    let mut m = AgentMachine::with_effects(vec![user("order")], 10, effects);

    let requests = run(
        &mut m,
        vec![
            AgentCompletion::LlmResponded(assistant_tool_calls(&[("r", "read"), ("w", "write")])),
            AgentCompletion::ToolFinished {
                id: "r".to_string(),
                output: tool_output("r", false),
            },
            AgentCompletion::ToolFinished {
                id: "w".to_string(),
                output: tool_output("w", false),
            },
            AgentCompletion::LlmResponded(assistant_text("done")),
        ],
    );
    let messages = requests
        .iter()
        .find_map(|r| match r {
            AgentRequest::Finished { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .expect("a finished payload");
    let ids: Vec<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(result) => Some(result.tool_call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["r".to_string(), "w".to_string()]);
}
