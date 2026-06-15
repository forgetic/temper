//! Effect-batching tests: which tool calls run concurrently, where serialized
//! barriers fall, and that tool-result messages always land in original
//! tool-call order regardless of completion order.

use std::collections::BTreeMap;
use std::sync::Arc;

use temper_agent_io::{EngineTime, Machine};
use tongs::model::Message;
use tongs::tools::ToolEffects;

use super::common::{
    assistant_text, assistant_tool_calls, calls_llm, machine, machine_read_tools, run, run_tools,
    tool_output, user,
};
use crate::machine::{AgentCompletion, AgentMachine, AgentRequest};

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
