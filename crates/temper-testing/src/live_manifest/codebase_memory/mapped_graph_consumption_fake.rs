//! Jig runtime for feature #1009's mapped provider transcript.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE;

use super::{
    BOUNDED_GRAPH_RESULT_NEEDLE, MAX_MODEL_MESSAGE_BYTES, ModelObservations,
    RAW_PROVIDER_FAILURE_NEEDLE, SAFE_PROVIDER_FAILURE, is_current_root_source_result,
    messages_contain,
};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected mapped graph-consumption fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start mapped graph-consumption Jig fake LLM: {error}"))
}

fn record_observations(view: &RequestView, observations: &mut ModelObservations) {
    observations.prompt_guidance_seen |= messages_contain(view, "CODEBASE MEMORY");
    observations.memory_result_seen |= provider_results(view).iter().any(|result| {
        result
            .pointer("/results/0/results/0/qualifiedName")
            .is_some()
    });
    observations.code_refinement_seen |= provider_results(view).iter().any(|result| {
        result
            .pointer("/results/0/related_source_references/0/qualifiedName")
            .is_some()
    });
    observations.graph_trace_seen |= provider_results(view)
        .iter()
        .any(|result| result.get("related_sources").is_some());
    let sources = view
        .messages
        .iter()
        .filter(|message| is_current_root_source_result(&message.content))
        .count();
    observations.current_root_source_seen |= sources > 0;
    observations.current_root_source_results += sources;
    observations.safe_failure_seen |= messages_contain(view, SAFE_PROVIDER_FAILURE);
    observations.raw_provider_text_seen |= messages_contain(view, RAW_PROVIDER_FAILURE_NEEDLE);
    observations.bounded_graph_result_seen |= messages_contain(view, BOUNDED_GRAPH_RESULT_NEEDLE);
    observations.oversized_message_seen |= view
        .messages
        .iter()
        .any(|message| message.content.len() > MAX_MODEL_MESSAGE_BYTES);
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_reply(
            "discover-mapped-routing-root",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "worker affinity routing"}),
        ),
        1 => tool_reply(
            "refine-mapped-routing-symbol",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": result_at(view, "/results/0/results/0/name")}),
        ),
        2 => tool_reply(
            "trace-mapped-routing-caller",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": result_at(view, "/results/0/name")}),
        ),
        3 => tool_reply(
            "read-mapped-current-root-implementation",
            "codebase_memory_get_code_snippet",
            serde_json::json!({"qualified_name": result_at(view, "/callers/0/qualified_name")}),
        ),
        4 => tool_reply(
            "read-mapped-current-root-focused-test",
            "codebase_memory_get_code_snippet",
            serde_json::json!({"qualified_name": result_at(view, "/source_metadata/related_source_references/0/qualifiedName")}),
        ),
        5 => {
            assert_complete_source_evidence(view);
            tool_reply(
                "observe-expected-unavailable-descendant",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": result_at(view, "/source_metadata/next_target/qualifiedName")}),
            )
        }
        6 => {
            assert!(
                messages_contain(view, CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE),
                "post-decision descendant must receive local convergence guidance"
            );
            tool_reply(
                "read-conventional-fallback-source",
                "read",
                serde_json::json!({"path": "demo/src/lib.rs"}),
            )
        }
        7 => tool_reply(
            "write-minimal-mapped-repair",
            "write",
            serde_json::json!({
                "path": "demo/src/lib.rs",
                "content": "pub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n    preferred.unwrap_or(value)\n}\n"
            }),
        ),
        8 => tool_reply(
            "validate-minimal-mapped-repair",
            "bash",
            serde_json::json!({"command": "cd demo && cargo fmt --check && cargo test --quiet", "timeout": 60}),
        ),
        9 => tool_reply(
            "submit-mapped-graph-repair",
            "submit_for_pr",
            serde_json::json!({"summary": "Consumed the mapped multi-part current-root lineage before the minimal repair."}),
        ),
        10 => Reply::text(
            r##"{"title":"Keep selected dispatch behavior","body":"# Implementation report\nConsumed the mapped multi-part current-root lineage before the minimal repair. `cargo fmt --check` and `cargo test --quiet` pass.","summary":"Consumed mapped graph lineage before the minimal repair."}"##,
        ),
        turn => panic!("unexpected mapped graph-consumption model turn {turn}"),
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

fn assert_complete_source_evidence(view: &RequestView) {
    let results = provider_results(view);
    assert!(results.iter().any(|result| result.get("callers").is_some()));
    assert!(
        results
            .iter()
            .any(|result| result.get("related_sources").is_some())
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| {
                result.get("binding").and_then(JsonValue::as_str)
                    == Some("current_prepared_checkout")
                    && result.get("source").and_then(JsonValue::as_str).is_some()
            })
            .count(),
        2,
        "mutation requires two complete current-root source results"
    );
}

fn result_at(view: &RequestView, pointer: &str) -> String {
    provider_results(view)
        .iter()
        .rev()
        .find_map(|result| result.pointer(pointer).and_then(JsonValue::as_str))
        .map(str::to_string)
        .expect("provider transcript omitted the approved later-turn selector")
}

fn provider_results(view: &RequestView) -> Vec<JsonValue> {
    view.messages
        .iter()
        .filter(|message| message.role == "tool")
        .filter_map(|message| {
            let content = message
                .content
                .split_once("\n\n[Decision anchor:")
                .map_or(message.content.as_str(), |(result, _)| result);
            serde_json::from_str(content).ok()
        })
        .collect()
}
