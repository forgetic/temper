//! Run-local ordinary-tool failure circuit tests.

use std::collections::BTreeMap;

use temper_agent_io::{EngineTime, Machine};
use tongs::model::{ContentBlock, Message};
use tongs::tools::ToolEffects;

use super::common::{
    assistant_tool_call_with_args, calls_llm, complete, llm_responded, run_tools, tool_failed,
    tool_finished, tool_output, user,
};
use crate::machine::ordinary_failure::ORDINARY_FAILURE_CAPACITY;
use crate::machine::{
    AgentMachine, AgentRequest, ToolFailureCategory, ToolFailureDiagnostic, ToolFailureReason,
};

fn machine_for(name: &str, max_iterations: usize) -> AgentMachine {
    AgentMachine::with_effects(
        vec![user("exercise the ordinary circuit")],
        max_iterations,
        BTreeMap::from([(name.to_string(), ToolEffects::read())]),
    )
}

fn redirect(requests: &[AgentRequest]) -> Option<ToolFailureDiagnostic> {
    requests.iter().find_map(|request| match request {
        AgentRequest::RedirectTool { failure, .. } => Some(failure.clone()),
        _ => None,
    })
}

fn non_retryable() -> ToolFailureDiagnostic {
    ToolFailureDiagnostic::execution(ToolFailureReason::ToolReportedFailure)
}

fn reordered_arguments(reverse: bool, value: u64) -> serde_json::Value {
    let mut nested = serde_json::Map::new();
    if reverse {
        nested.insert("second".into(), serde_json::json!([true, value]));
        nested.insert("first".into(), serde_json::json!("opaque"));
    } else {
        nested.insert("first".into(), serde_json::json!("opaque"));
        nested.insert("second".into(), serde_json::json!([true, value]));
    }
    let mut root = serde_json::Map::new();
    if reverse {
        root.insert("nested".into(), nested.into());
        root.insert("value".into(), value.into());
    } else {
        root.insert("value".into(), value.into());
        root.insert("nested".into(), nested.into());
    }
    root.into()
}

#[test]
fn second_canonical_non_retryable_call_redirects_but_corrected_arguments_run() {
    const PRIVATE_ARGUMENT: &str = "Authorization: Bearer CIRCUIT-ARGUMENT";
    let mut machine = machine_for("read", 10);
    let _ = machine.on_start(EngineTime::ZERO);

    let first = complete(
        &mut machine,
        llm_responded(assistant_tool_call_with_args(
            "first",
            "read",
            serde_json::json!({
                "path": PRIVATE_ARGUMENT,
                "options": reordered_arguments(false, 1)
            }),
        )),
    );
    assert_eq!(run_tools(&first), ["first"]);
    let _ = complete(
        &mut machine,
        tool_failed(
            "first",
            tool_output("private raw failure", true),
            non_retryable(),
        ),
    );

    let repeated = complete(
        &mut machine,
        llm_responded(assistant_tool_call_with_args(
            "repeated",
            "read",
            serde_json::json!({
                "options": reordered_arguments(true, 1),
                "path": PRIVATE_ARGUMENT
            }),
        )),
    );
    assert!(
        run_tools(&repeated).is_empty(),
        "a redirect must not emit RunTool"
    );
    let failure = redirect(&repeated).expect("identical invocation redirects locally");
    assert_eq!(failure.category, ToolFailureCategory::CircuitRedirect);
    assert_eq!(failure.reason, ToolFailureReason::RepeatedNonRetryable);
    assert!(!format!("{failure:?}").contains(PRIVATE_ARGUMENT));

    let next_turn = complete(
        &mut machine,
        tool_failed(
            "repeated",
            tool_output("must be replaced", true),
            failure.clone(),
        ),
    );
    assert_eq!(calls_llm(&next_turn), 1);
    let result_text = next_turn
        .iter()
        .find_map(|request| match request {
            AgentRequest::CallLlm { messages, .. } => messages.iter().rev().find_map(|message| {
                let Message::ToolResult(result) = message else {
                    return None;
                };
                result.content.iter().find_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
            }),
            _ => None,
        })
        .expect("redirect produced a model tool result");
    assert_eq!(result_text, failure.message);
    assert!(!result_text.contains(PRIVATE_ARGUMENT));

    let corrected = complete(
        &mut machine,
        llm_responded(assistant_tool_call_with_args(
            "corrected",
            "read",
            serde_json::json!({
                "path": PRIVATE_ARGUMENT,
                "options": reordered_arguments(false, 2)
            }),
        )),
    );
    assert_eq!(run_tools(&corrected), ["corrected"]);
    assert!(redirect(&corrected).is_none());
}

#[test]
fn retryable_failures_have_two_execution_attempts_then_redirect() {
    let mut machine = machine_for("bash", 10);
    let _ = machine.on_start(EngineTime::ZERO);
    let timeout = ToolFailureDiagnostic::timeout();

    for id in ["attempt-1", "attempt-2"] {
        let requests = complete(
            &mut machine,
            llm_responded(assistant_tool_call_with_args(
                id,
                "bash",
                serde_json::json!({"command":"bounded"}),
            )),
        );
        assert_eq!(run_tools(&requests), [id]);
        assert!(redirect(&requests).is_none());
        let _ = complete(
            &mut machine,
            tool_failed(id, tool_output("timeout", true), timeout.clone()),
        );
    }

    let exhausted = complete(
        &mut machine,
        llm_responded(assistant_tool_call_with_args(
            "attempt-3",
            "bash",
            serde_json::json!({"command":"bounded"}),
        )),
    );
    assert!(run_tools(&exhausted).is_empty());
    assert_eq!(
        redirect(&exhausted).expect("retry budget redirect").reason,
        ToolFailureReason::RetryBudgetExhausted
    );
}

#[test]
fn fifo_capacity_evicts_the_oldest_identity_deterministically() {
    let mut machine = machine_for("read", ORDINARY_FAILURE_CAPACITY + 4);
    let _ = machine.on_start(EngineTime::ZERO);

    for value in 0..=ORDINARY_FAILURE_CAPACITY {
        let id = format!("fill-{value}");
        let dispatched = complete(
            &mut machine,
            llm_responded(assistant_tool_call_with_args(
                &id,
                "read",
                serde_json::json!({"slot":value}),
            )),
        );
        assert_eq!(run_tools(&dispatched), [id.as_str()]);
        let _ = complete(
            &mut machine,
            tool_failed(&id, tool_output("failed", true), non_retryable()),
        );
    }

    let oldest = complete(
        &mut machine,
        llm_responded(assistant_tool_call_with_args(
            "oldest",
            "read",
            serde_json::json!({"slot":0}),
        )),
    );
    assert_eq!(
        run_tools(&oldest),
        ["oldest"],
        "oldest entry is evicted first"
    );
    let _ = complete(
        &mut machine,
        tool_finished("oldest", tool_output("ok", false)),
    );

    let retained = complete(
        &mut machine,
        llm_responded(assistant_tool_call_with_args(
            "retained",
            "read",
            serde_json::json!({"slot":1}),
        )),
    );
    assert!(run_tools(&retained).is_empty());
    assert_eq!(
        redirect(&retained)
            .expect("second-oldest identity remains")
            .reason,
        ToolFailureReason::RepeatedNonRetryable
    );
}

#[test]
fn fresh_machine_resets_state_and_graph_calls_bypass_the_ordinary_circuit() {
    let mut first = machine_for("read", 5);
    let _ = first.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut first,
        llm_responded(assistant_tool_call_with_args(
            "failed",
            "read",
            serde_json::json!({"path":"same"}),
        )),
    );
    let _ = complete(
        &mut first,
        tool_failed("failed", tool_output("failed", true), non_retryable()),
    );
    let blocked = complete(
        &mut first,
        llm_responded(assistant_tool_call_with_args(
            "blocked",
            "read",
            serde_json::json!({"path":"same"}),
        )),
    );
    assert!(run_tools(&blocked).is_empty());

    let mut fresh = machine_for("read", 5);
    let _ = fresh.on_start(EngineTime::ZERO);
    let reset = complete(
        &mut fresh,
        llm_responded(assistant_tool_call_with_args(
            "reset",
            "read",
            serde_json::json!({"path":"same"}),
        )),
    );
    assert_eq!(run_tools(&reset), ["reset"]);

    let graph_name = "codebase_memory_search_graph";
    let mut graph = machine_for(graph_name, 5);
    let _ = graph.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut graph,
        llm_responded(assistant_tool_call_with_args(
            "graph-failed",
            graph_name,
            serde_json::json!({"query":"same"}),
        )),
    );
    let _ = complete(
        &mut graph,
        tool_failed("graph-failed", tool_output("failed", true), non_retryable()),
    );
    let graph_retry = complete(
        &mut graph,
        llm_responded(assistant_tool_call_with_args(
            "graph-retry",
            graph_name,
            serde_json::json!({"query":"same"}),
        )),
    );
    assert_eq!(run_tools(&graph_retry), ["graph-retry"]);
    assert!(redirect(&graph_retry).is_none());
}
