use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;

use super::{
    BOUNDED_GRAPH_RESULT_NEEDLE, MAX_MODEL_MESSAGE_BYTES, MEMORY_RESULT_NEEDLE, ModelObservations,
    RAW_PROVIDER_FAILURE_NEEDLE, SAFE_PROVIDER_FAILURE, is_current_root_source_result,
    messages_contain,
};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected codebase-memory fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_model_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start result-driven decision Jig fake LLM: {error}"))
}

fn record_model_observations(view: &RequestView, observations: &mut ModelObservations) {
    if messages_contain(view, "CODEBASE MEMORY") {
        observations.prompt_guidance_seen = true;
    }
    if messages_contain(view, MEMORY_RESULT_NEEDLE)
        || messages_contain(view, "SEQUENTIAL_GRAPH_RESULT")
        || messages_contain(view, "RESULT_DRIVEN_GRAPH_RESULT")
    {
        observations.memory_result_seen = true;
    }
    if messages_contain(view, "FAKE_MCP_CODE_RESULT")
        || messages_contain(view, "SEQUENTIAL_CODE_RESULT")
        || messages_contain(view, "RESULT_DRIVEN_CODE_RESULT")
    {
        observations.code_refinement_seen = true;
    }
    if messages_contain(view, "FAKE_MCP_TRACE_RESULT")
        || messages_contain(view, "SEQUENTIAL_TRACE_RESULT")
        || messages_contain(view, "RESULT_DRIVEN_TRACE_RESULT")
    {
        observations.graph_trace_seen = true;
    }
    let current_root_source_results = view
        .messages
        .iter()
        .filter(|message| is_current_root_source_result(&message.content))
        .count();
    observations.current_root_source_seen |= current_root_source_results > 0;
    observations.current_root_source_results += current_root_source_results;
    if messages_contain(view, SAFE_PROVIDER_FAILURE) {
        observations.safe_failure_seen = true;
    }
    if messages_contain(view, RAW_PROVIDER_FAILURE_NEEDLE) {
        observations.raw_provider_text_seen = true;
    }
    if messages_contain(view, BOUNDED_GRAPH_RESULT_NEEDLE) {
        observations.bounded_graph_result_seen = true;
    }
    if view
        .messages
        .iter()
        .any(|message| message.content.len() > MAX_MODEL_MESSAGE_BYTES)
    {
        observations.oversized_message_seen = true;
    }
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_reply(
            "discover-implementation",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "implementation selection"}),
        ),
        1 => {
            require_fact(
                view,
                "current_root",
                "refinement requires a consumed current-root producer result",
            );
            tool_reply(
                "refine-provider-selection",
                "codebase_memory_search_code",
                serde_json::json!({"pattern": next_target(view)}),
            )
        }
        2 => tool_reply(
            "trace-provider-selection",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": next_target(view)}),
        ),
        3 => {
            require_fact(
                view,
                "caller_model",
                "source selection requires consumed caller/model evidence",
            );
            tool_reply(
                "read-provider-selected-implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": next_target(view)}),
            )
        }
        4 => {
            require_fact(
                view,
                "implementation_source",
                "focused test selection requires consumed implementation evidence",
            );
            tool_reply(
                "read-provider-selected-behavioral-test",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": next_target(view)}),
            )
        }
        5 => {
            for fact in [
                "current_root",
                "caller_model",
                "implementation_source",
                "behavioral_test",
            ] {
                require_fact(
                    view,
                    fact,
                    "mutation requires consumed current-root implementation, caller/model, and focused behavioral-test evidence",
                );
            }
            tool_reply(
                "write-minimal-repair-after-evidence",
                "write",
                serde_json::json!({
                    "path": "demo/src/lib.rs",
                    "content": "pub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n    preferred.unwrap_or(value)\n}\n"
                }),
            )
        }
        6 => tool_reply(
            "validate-minimal-repair",
            "bash",
            serde_json::json!({"command": "cd demo && cargo fmt --check && cargo test --quiet", "timeout": 60}),
        ),
        7 => tool_reply(
            "submit-result-driven-repair",
            "submit_for_pr",
            serde_json::json!({"summary": "Consumed result-derived current-root evidence before the minimal repair."}),
        ),
        8 => Reply::text(
            r##"{"title":"Keep selected dispatch behavior","body":"# Implementation report\nConsumed provider-derived current-root evidence before the minimal repair. `cargo fmt --check` and `cargo test --quiet` pass.","summary":"Consumed result-derived evidence before the minimal repair."}"##,
        ),
        turn => panic!("unexpected result-driven decision model turn {turn}"),
    }
}

fn tool_reply(id: &str, name: &str, args: JsonValue) -> Reply {
    Reply {
        turns: vec![Turn::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            args,
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn next_target(view: &RequestView) -> String {
    result_values(view, "next")
        .pop()
        .expect("successful provider result selected a later-turn dependent target")
}

fn require_fact(view: &RequestView, fact: &str, reason: &str) {
    assert!(!result_values(view, fact).is_empty(), "{reason}");
}

fn result_values(view: &RequestView, field: &str) -> Vec<String> {
    view.messages
        .iter()
        .filter_map(|message| {
            if message.role != "tool" {
                return None;
            }
            let value = serde_json::from_str::<JsonValue>(&message.content).ok()?;
            value
                .get(field)
                .or_else(|| value.pointer(format!("/results/0/{field}").as_str()))
                .and_then(JsonValue::as_str)
                .map(str::to_string)
        })
        .collect()
}
