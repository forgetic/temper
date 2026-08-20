//! Effect-batching tests: which tool calls run concurrently, where serialized
//! barriers fall, and that tool-result messages always land in original
//! tool-call order regardless of completion order.

use std::collections::BTreeMap;
use std::sync::Arc;

use temper_agent_io::{EngineTime, Machine};
use tongs::model::Message;
use tongs::tools::ToolEffects;

use super::common::{
    assistant_text, assistant_tool_calls, calls_llm, complete, llm_responded, machine,
    machine_read_tools, run, run_tools, tool_finished, tool_output, user,
};
use crate::machine::{
    AgentCompletion, AgentMachine, AgentRequest, ToolFailureDiagnostic, ToolFailureReason,
};

#[test]
fn parallel_batch_runs_concurrently_and_waits_for_all_before_next_call() {
    // Two read-only tools ⇒ one parallel batch: both dispatched at once, and the
    // model is not re-called until BOTH finish.
    let mut m = machine_read_tools(&["read", "grep"]);
    let mut requests = m.on_start(EngineTime::ZERO);
    requests.extend(complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[("a", "read"), ("b", "grep")])),
    ));
    // Both run together (one batch).
    assert_eq!(run_tools(&requests), vec!["a".to_string(), "b".to_string()]);

    // First tool finishes — must NOT call the model yet (batch incomplete).
    let after_first = complete(&mut m, tool_finished("a", tool_output("a out", false)));
    assert_eq!(
        calls_llm(&after_first),
        0,
        "must wait for the whole batch before re-calling the model"
    );
    // No second batch dispatched either — they were in the same batch.
    assert!(run_tools(&after_first).is_empty());

    // Second tool finishes — now the model is called again.
    let after_second = complete(&mut m, tool_finished("b", tool_output("b out", false)));
    assert_eq!(calls_llm(&after_second), 1);
}

#[test]
fn independent_codebase_memory_discovery_remains_parallel_safe() {
    // The static effect policy remains intentionally unaware of semantic
    // dependencies. Independent provider-shaped discovery issued in one model turn stays
    // in a concurrent read-only batch; the Jig decision-chain coverage issues
    // dependent producer/consumer calls in separate model turns instead.
    let mut m = machine_read_tools(&[
        "codebase_memory_search_graph",
        "codebase_memory_search_code",
    ]);
    let mut requests = m.on_start(EngineTime::ZERO);
    requests.extend(complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[
            ("implementation", "codebase_memory_search_graph"),
            ("behavior", "codebase_memory_search_code"),
        ])),
    ));

    assert_eq!(
        run_tools(&requests),
        vec!["implementation".to_string(), "behavior".to_string()],
        "independent read-only discovery must remain one parallel-safe batch"
    );

    let after_first = complete(
        &mut m,
        tool_finished(
            "implementation",
            tool_output(r#"{"results":[{"next":"opaque"}]}"#, false),
        ),
    );
    assert_eq!(calls_llm(&after_first), 0);
    let after_second = complete(
        &mut m,
        tool_finished(
            "behavior",
            tool_output(r#"{"results":[{"evidence":"opaque"}]}"#, false),
        ),
    );
    assert_eq!(calls_llm(&after_second), 1);
}

#[test]
fn producer_turn_dependent_reads_remain_one_static_batch() {
    // Static effects do not inspect opaque provider values. This documents the
    // deliberately unchanged scheduler boundary: model-visible decision-chain
    // guidance, not batching inference, keeps these dependent calls out of one
    // producer turn.
    let mut m = machine_read_tools(&[
        "codebase_memory_search_graph",
        "codebase_memory_search_code",
        "codebase_memory_trace_path",
        "codebase_memory_get_code_snippet",
    ]);
    let mut requests = m.on_start(EngineTime::ZERO);
    requests.extend(complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[
            ("producer", "codebase_memory_search_graph"),
            ("refinement", "codebase_memory_search_code"),
            ("trace", "codebase_memory_trace_path"),
            ("source", "codebase_memory_get_code_snippet"),
        ])),
    ));
    assert_eq!(
        run_tools(&requests),
        vec!["producer", "refinement", "trace", "source"],
        "read-only calls remain a single static batch even when a model wrongly groups them"
    );

    for id in ["producer", "refinement", "trace", "source"] {
        let after_result = complete(
            &mut m,
            tool_finished(id, tool_output(r#"{"results":[{"next":"opaque"}]}"#, false)),
        );
        if id == "source" {
            assert_eq!(calls_llm(&after_result), 1);
        } else {
            assert_eq!(calls_llm(&after_result), 0);
        }
    }
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
            llm_responded(assistant_tool_calls(&[("x", "read"), ("y", "grep")])),
            tool_finished("y", tool_output("y", false)),
            tool_finished("x", tool_output("x", false)),
            llm_responded(assistant_text("done")),
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
    requests.extend(complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[
            ("r1", "read"),
            ("w", "write"),
            ("r2", "read"),
        ])),
    ));
    // Only the first batch (r1) is dispatched.
    assert_eq!(run_tools(&requests), vec!["r1".to_string()]);

    // r1 finishes ⇒ dispatch the write batch (w), still no model call.
    let after_r1 = complete(&mut m, tool_finished("r1", tool_output("r1", false)));
    assert_eq!(run_tools(&after_r1), vec!["w".to_string()]);
    assert_eq!(calls_llm(&after_r1), 0);

    // w finishes ⇒ dispatch the last read batch (r2).
    let after_w = complete(&mut m, tool_finished("w", tool_output("w", false)));
    assert_eq!(run_tools(&after_w), vec!["r2".to_string()]);
    assert_eq!(calls_llm(&after_w), 0);

    // r2 finishes ⇒ all batches done ⇒ the model is re-called.
    let after_r2 = complete(&mut m, tool_finished("r2", tool_output("r2", false)));
    assert_eq!(calls_llm(&after_r2), 1);
}

#[test]
fn unknown_tools_are_serialized_fail_closed() {
    // With no effect declarations (the default `machine()`), every tool is
    // treated as a write ⇒ each is its own serial batch. Two calls ⇒ the second
    // is not dispatched until the first finishes.
    let mut m = machine();
    let mut requests = m.on_start(EngineTime::ZERO);
    requests.extend(complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[("a", "mystery"), ("b", "mystery")])),
    ));
    assert_eq!(run_tools(&requests), vec!["a".to_string()]);

    let after_a = complete(&mut m, tool_finished("a", tool_output("a", false)));
    assert_eq!(run_tools(&after_a), vec!["b".to_string()]);
    assert_eq!(calls_llm(&after_a), 0);
}

#[test]
fn duplicate_and_stale_parallel_completions_settle_each_call_once() {
    let mut m = machine_read_tools(&["read", "grep"]);
    let _ = m.on_start(EngineTime::ZERO);
    let dispatched = complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[("a", "read"), ("b", "grep")])),
    );
    assert_eq!(run_tools(&dispatched), vec!["a", "b"]);

    let (a_operation, batch_generation) = m.active_tool_generations("a").expect("active call a");
    let (b_operation, b_batch_generation) = m.active_tool_generations("b").expect("active call b");
    assert_eq!(batch_generation, b_batch_generation);

    let finish_a = AgentCompletion::ToolFinished {
        operation_generation: a_operation,
        batch_generation,
        id: "a".to_string(),
        output: tool_output("a", false),
        failure: None,
    };
    assert!(
        m.on_completion(EngineTime::ZERO, finish_a).is_empty(),
        "one parallel call cannot advance the batch"
    );
    assert!(
        m.on_completion(
            EngineTime::ZERO,
            AgentCompletion::ToolFinished {
                operation_generation: a_operation,
                batch_generation,
                id: "a".to_string(),
                output: tool_output("duplicate", false),
                failure: None,
            },
        )
        .is_empty(),
        "a duplicate completion must be ignored"
    );
    assert!(
        m.on_completion(
            EngineTime::ZERO,
            AgentCompletion::ToolFinished {
                operation_generation: b_operation,
                batch_generation: batch_generation.saturating_add(1),
                id: "b".to_string(),
                output: tool_output("stale", false),
                failure: None,
            },
        )
        .is_empty(),
        "a stale batch generation must be ignored"
    );

    let settled = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            operation_generation: b_operation,
            batch_generation,
            id: "b".to_string(),
            output: tool_output("b", false),
            failure: None,
        },
    );
    assert_eq!(calls_llm(&settled), 1, "the batch settles exactly once");

    let late = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            operation_generation: b_operation,
            batch_generation,
            id: "b".to_string(),
            output: tool_output("late", false),
            failure: None,
        },
    );
    assert!(
        late.is_empty(),
        "a prior-batch completion cannot re-dispatch"
    );
}

#[test]
fn typed_failure_completion_replaces_arbitrary_output_for_the_next_model_turn() {
    const SECRET: &str = "Authorization: Bearer MACHINE-TOOL-SECRET";
    let mut machine = machine_read_tools(&["read"]);
    let _ = machine.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[("failed", "read")])),
    );
    let (operation_generation, batch_generation) = machine
        .active_tool_generations("failed")
        .expect("active tool");
    let failure = ToolFailureDiagnostic::execution(ToolFailureReason::ToolReportedFailure);
    let requests = machine.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            operation_generation,
            batch_generation,
            id: "failed".to_string(),
            output: tool_output(SECRET, true),
            failure: Some(failure.clone()),
        },
    );
    let messages = requests
        .iter()
        .find_map(|request| match request {
            AgentRequest::CallLlm { messages, .. } => Some(messages),
            _ => None,
        })
        .expect("next model call");
    let result = messages
        .iter()
        .find_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .expect("tool result");
    let text = result.content.iter().find_map(|block| match block {
        tongs::model::ContentBlock::Text(text) => Some(text.text.as_str()),
        _ => None,
    });
    assert_eq!(text, Some(failure.message.as_str()));
    assert!(result.details.is_none());
    assert!(result.is_error);
    assert!(!format!("{messages:?}").contains(SECRET));
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
            llm_responded(assistant_tool_calls(&[("r", "read"), ("w", "write")])),
            tool_finished("r", tool_output("r", false)),
            tool_finished("w", tool_output("w", false)),
            llm_responded(assistant_text("done")),
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
