//! Jig runtime for feature #1026's mapped graph-convergence scenario.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::{
    CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE, DECISION_ANCHOR_CONVERGENCE_MESSAGE,
};

use super::{
    ModelObservations, SAFE_PROVIDER_FAILURE, is_current_root_source_result, messages_contain,
};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected mapped graph-convergence fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start mapped graph-convergence Jig fake LLM: {error}"))
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
        .any(|result| result.get("callers").is_some());
    let sources = view
        .messages
        .iter()
        .filter(|message| is_current_root_source_result(&message.content))
        .count();
    observations.current_root_source_seen |= sources > 0;
    observations.current_root_source_results += sources;
    observations.safe_failure_seen |= messages_contain(view, SAFE_PROVIDER_FAILURE)
        || messages_contain(view, "codebase-memory request input was invalid");
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_reply(
            "preflight-current-root-availability",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "availability preflight"}),
        ),
        1 => tool_reply(
            "trace-preflight-source",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": result_at(view, "/results/0/results/0/name")}),
        ),
        2 => tool_reply(
            "read-preflight-unavailable-source",
            "codebase_memory_get_code_snippet",
            serde_json::json!({"qualified_name": result_at(view, "/callers/0/qualified_name")}),
        ),
        3 => {
            assert!(
                messages_contain(view, "codebase-memory request input was invalid"),
                "unavailable pre-completion source must return bounded safe guidance"
            );
            tool_reply(
                "conventional-fallback-after-unavailable",
                "bash",
                serde_json::json!({"command": "cd demo && rg worker_slot src", "timeout": 60}),
            )
        }
        // Keep the independent roots on distinct model turns. The fixture's
        // closed provider chain is ordered, while calls within one tool batch
        // may reach the provider in either order.
        4 => tool_reply(
            "discover-routing-implementation-root",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "routing implementation affinity"}),
        ),
        5 => tool_reply(
            "discover-focused-behavior-root",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "focused alias retry behavior"}),
        ),
        6 => tool_reply(
            "refine-routing-implementation",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": independent_root_at(view, 1, "/results/0/results/0/name")}),
        ),
        7 => tool_reply(
            "trace-routing-caller",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": result_at(view, "/results/0/name")}),
        ),
        8 => tool_reply(
            "duplicate-routing-refinement",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": result_at(view, "/function/name")}),
        ),
        9 => tool_batch(&[
            (
                "read-current-root-routing-implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": result_at(view, "/callers/0/qualified_name")}),
            ),
            (
                "read-current-root-focused-test",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": result_at(view, "/results/0/results/0/qualifiedName")}),
            ),
        ]),
        11 => {
            assert!(
                messages_contain(view, DECISION_ANCHOR_CONVERGENCE_MESSAGE),
                "complete source evidence must inject one local convergence instruction"
            );
            tool_batch(&[
                (
                    "post-decision-broad-search",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "broad repository architecture inventory"}),
                ),
                (
                    "post-decision-duplicate-refinement",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "worker_slot"}),
                ),
                (
                    "post-decision-duplicate-source",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": "DeliveryRouter::worker_for"}),
                ),
            ])
        }
        14 => {
            assert_eq!(
                view.messages
                    .iter()
                    .filter(|message| {
                        message
                            .content
                            .contains(CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE)
                    })
                    .count(),
                3,
                "all post-decision graph attempts must be denied locally"
            );
            tool_reply(
                "read-route-after-local-convergence",
                "read",
                serde_json::json!({"path": "demo/src/route.rs"}),
            )
        }
        15 => tool_reply(
            "patch-retry-affinity",
            "apply_patch",
            serde_json::json!({
                "patch": "diff --git a/demo/src/route.rs b/demo/src/route.rs\n--- a/demo/src/route.rs\n+++ b/demo/src/route.rs\n@@ -3,11 +3,7 @@ use crate::DeliveryAttempt;\n pub(crate) fn worker_slot(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {\n     assert!(workers > 0, \"at least one delivery worker is required\");\n \n-    let routing_topic = if attempt.attempt == 0 {\n-        attempt.affinity_topic()\n-    } else {\n-        attempt.topic\n-    };\n+    let routing_topic = attempt.affinity_topic();\n     let mut hash = 0xcbf29ce484222325_u64;\n     for byte in attempt\n         .tenant\n"
            }),
        ),
        16 => tool_reply(
            "validate-converged-repair",
            "bash",
            serde_json::json!({"command": "cd demo && cargo fmt --check && cargo test --quiet", "timeout": 60}),
        ),
        17 => tool_reply(
            "submit-converged-repair",
            "submit_for_pr",
            serde_json::json!({"summary": "Consumed bounded current-root graph evidence and local convergence guidance before the minimal repair."}),
        ),
        18 => Reply::text(
            r##"{"title":"Keep alias retries on the selected worker","body":"# Implementation report\nConsumed bounded current-root graph evidence and local convergence guidance before the minimal repair. Host validation passes.","summary":"Applied and validated the minimal retry-affinity repair."}"##,
        ),
        turn => panic!("unexpected mapped graph-convergence model turn {turn}"),
    }
}

fn tool_reply(id: &str, name: &str, args: JsonValue) -> Reply {
    tool_batch(&[(id, name, args)])
}

fn tool_batch(calls: &[(&str, &str, JsonValue)]) -> Reply {
    Reply {
        turns: calls
            .iter()
            .map(|(id, name, args)| Turn::ToolCall {
                id: (*id).to_string(),
                name: (*name).to_string(),
                args: args.clone(),
            })
            .collect(),
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn result_at(view: &RequestView, pointer: &str) -> String {
    provider_results(view)
        .iter()
        .rev()
        .find_map(|result| result.pointer(pointer).and_then(JsonValue::as_str))
        .map(str::to_string)
        .expect("provider transcript omitted a required later-turn selector")
}

fn independent_root_at(view: &RequestView, index: usize, pointer: &str) -> String {
    provider_results(view)
        .iter()
        .filter(|result| {
            result
                .pointer("/results/0/results/0/qualifiedName")
                .is_some()
        })
        .nth(index)
        .and_then(|result| result.pointer(pointer))
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .expect("independent root omitted its later source selector")
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
