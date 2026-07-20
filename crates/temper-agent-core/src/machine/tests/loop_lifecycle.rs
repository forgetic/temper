//! Loop-lifecycle tests: how the machine starts, completes, handles model and
//! transport errors, enforces the iteration budget, and reacts to abort and
//! steering at turn boundaries.

use temper_agent_io::{EngineTime, Machine};

use std::sync::Arc;

use super::common::{
    assistant_error, assistant_text, assistant_tool_call_with_args, assistant_tool_calls,
    calls_llm, complete, final_stop, llm_failed, llm_responded, machine, run, run_tools,
    tool_finished, tool_output, tool_start_previews, user,
};
use crate::machine::{AgentCompletion, AgentEvent, AgentMachine, AgentRequest, AgentStop};
use crate::{ModelFailureCategory, ModelFailureDiagnostic};

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
    let requests = run(&mut m, vec![llm_responded(assistant_text("all done"))]);
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
            llm_responded(assistant_tool_calls(&[("call-1", "read")])),
            tool_finished("call-1", tool_output("file contents", false)),
            llm_responded(assistant_text("done after reading")),
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
fn model_error_stops_immediately() {
    let mut m = machine();
    let requests = run(&mut m, vec![llm_responded(assistant_error())]);
    assert_eq!(final_stop(&requests), Some(AgentStop::ModelError));
    assert!(m.is_stopped());
}

#[test]
fn transport_failure_stops_with_model_error() {
    let mut m = machine();
    let requests = run(&mut m, vec![llm_failed("connection reset")]);
    assert_eq!(final_stop(&requests), Some(AgentStop::ModelError));
}

#[test]
fn typed_model_failure_is_authoritative_on_finished_request() {
    let mut m = machine();
    m.on_start(EngineTime::ZERO);
    let (operation_generation, batch_generation) =
        m.active_generations().expect("active model call");
    let diagnostic = ModelFailureDiagnostic::redacted_unknown("test-provider", "test-model", false);
    let requests = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmFailed {
            operation_generation,
            batch_generation,
            diagnostic,
        },
    );

    let retained = requests.iter().find_map(|request| match request {
        AgentRequest::Finished { model_failure, .. } => model_failure.as_ref(),
        _ => None,
    });
    assert_eq!(
        retained.map(ModelFailureDiagnostic::category),
        Some(ModelFailureCategory::RedactedUnknown)
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
        requests.extend(complete(
            &mut m,
            llm_responded(assistant_tool_calls(&[("c", "read")])),
        ));
        if m.is_stopped() {
            break;
        }
        // tool finishes
        requests.extend(complete(
            &mut m,
            tool_finished("c", tool_output("again", false)),
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
    let cancel = m.on_completion(EngineTime::ZERO, AgentCompletion::Abort);
    let (operation_generation, batch_generation) = cancel
        .iter()
        .find_map(|request| match request {
            AgentRequest::CancelActive {
                operation_generation,
                batch_generation,
            } => Some((*operation_generation, *batch_generation)),
            _ => None,
        })
        .expect("cancellation request");
    assert_eq!(final_stop(&cancel), None);
    let requests = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::TasksQuiesced {
            operation_generation,
            batch_generation,
        },
    );
    assert_eq!(final_stop(&requests), Some(AgentStop::Aborted));
    assert!(m.is_stopped());
}

#[test]
fn abort_during_tools_cancels_immediately_and_waits_for_quiescence() {
    let mut m = machine();
    m.on_start(EngineTime::ZERO);
    complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[("t", "bash")])),
    );
    // Abort immediately asks the shell to cancel the hung task and clears all
    // pending batches. It does not wait for a ToolFinished completion.
    let mid = m.on_completion(EngineTime::ZERO, AgentCompletion::Abort);
    let (operation_generation, batch_generation) = mid
        .iter()
        .find_map(|request| match request {
            AgentRequest::CancelActive {
                operation_generation,
                batch_generation,
            } => Some((*operation_generation, *batch_generation)),
            _ => None,
        })
        .expect("cancellation request");
    assert_eq!(final_stop(&mid), None);
    assert!(!m.is_stopped());

    let after = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::TasksQuiesced {
            operation_generation,
            batch_generation,
        },
    );
    assert_eq!(final_stop(&after), Some(AgentStop::Aborted));
    assert!(m.is_stopped());
}

#[test]
fn abort_fences_late_operations_and_stale_quiescence() {
    let mut m = machine();
    let _ = m.on_start(EngineTime::ZERO);
    let (model_operation, model_batch) = m.active_generations().expect("active model");

    let cancel = m.on_completion(EngineTime::ZERO, AgentCompletion::Abort);
    let (cancel_operation, cancel_batch) = cancel
        .iter()
        .find_map(|request| match request {
            AgentRequest::CancelActive {
                operation_generation,
                batch_generation,
            } => Some((*operation_generation, *batch_generation)),
            _ => None,
        })
        .expect("cancel request");

    let late_model = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::LlmResponded {
            operation_generation: model_operation,
            batch_generation: model_batch,
            message: assistant_tool_calls(&[("late", "read")]),
        },
    );
    assert!(late_model.is_empty());
    assert!(run_tools(&late_model).is_empty());
    assert_eq!(calls_llm(&late_model), 0);

    let stale_quiescence = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::TasksQuiesced {
            operation_generation: cancel_operation.saturating_add(1),
            batch_generation: cancel_batch,
        },
    );
    assert!(stale_quiescence.is_empty());
    assert!(!m.is_stopped());

    let finished = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::TasksQuiesced {
            operation_generation: cancel_operation,
            batch_generation: cancel_batch,
        },
    );
    assert_eq!(final_stop(&finished), Some(AgentStop::Aborted));

    let late_tool = m.on_completion(
        EngineTime::ZERO,
        AgentCompletion::ToolFinished {
            operation_generation: model_operation,
            batch_generation: 99,
            id: "late".to_string(),
            output: tool_output("late", false),
        },
    );
    assert!(late_tool.is_empty());
}

#[test]
fn steering_is_injected_at_the_next_turn_boundary() {
    let mut m = machine();
    m.on_start(EngineTime::ZERO);
    // A tool round is in flight.
    complete(
        &mut m,
        llm_responded(assistant_tool_calls(&[("s", "read")])),
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
    let after = complete(&mut m, tool_finished("s", tool_output("read", false)));
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
fn arg_preview_hook_fills_tool_start_field_from_call_args() {
    let mut m = AgentMachine::new(vec![user("inspect")], 10).with_arg_preview(Arc::new(
        |name: &str, args: &serde_json::Value| {
            // A trivial stand-in for the shell's real per-tool renderer: echo
            // "<name>:<path>" when a `path` arg is present.
            args.get("path")
                .and_then(|p| p.as_str())
                .map(|path| format!("{name}:{path}"))
        },
    ));
    let requests = run(
        &mut m,
        vec![llm_responded(assistant_tool_call_with_args(
            "call-1",
            "read",
            serde_json::json!({"path": "src/main.rs"}),
        ))],
    );
    assert_eq!(
        tool_start_previews(&requests),
        vec![Some("read:src/main.rs".to_string())],
        "the preview hook should populate ToolStart.arg_preview",
    );
}

#[test]
fn tool_start_arg_preview_is_none_without_a_hook() {
    let mut m = machine();
    let requests = run(
        &mut m,
        vec![llm_responded(assistant_tool_call_with_args(
            "call-1",
            "read",
            serde_json::json!({"path": "src/main.rs"}),
        ))],
    );
    assert_eq!(
        tool_start_previews(&requests),
        vec![None],
        "without a preview hook the field stays None (the pure default)",
    );
}
