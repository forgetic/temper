//! Jig runtime for feature #1069's mapped decision-gap recovery scenario.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::{
    CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE, DECISION_ANCHOR_CONVERGENCE_MESSAGE,
};

use super::{ModelObservations, is_current_root_source_result, messages_contain};

const RECOVERY_GUIDANCE: &str = "decision-evidence recovery required; missing evidence: [caller]; permitted action: targeted_current_root_graph_call; remaining allowance: 4";

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected mapped decision-gap recovery fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start mapped decision-gap recovery Jig fake LLM: {error}"))
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
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_reply(
            "discover-routing-implementation-root",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "routing implementation affinity"}),
        ),
        1 => tool_reply(
            "discover-focused-behavior-root",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "focused alias retry behavior"}),
        ),
        2 => tool_reply(
            "refine-routing-implementation",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": independent_root_at(view, 0, "/results/0/results/0/name")}),
        ),
        3 => tool_reply(
            "trace-routing-caller",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": result_at(view, "/results/0/name")}),
        ),
        4 => tool_reply(
            "read-current-root-focused-test",
            "codebase_memory_get_code_snippet",
            serde_json::json!({
                "qualified_name": independent_root_at(view, 1, "/results/0/results/0/qualifiedName"),
                "decision_evidence_kind": "focused_test",
            }),
        ),
        5 => tool_reply(
            "duplicate-routing-refinement-one",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": result_at(view, "/function/name")}),
        ),
        6 => tool_reply(
            "duplicate-routing-refinement-exhausts-budget",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": result_at(view, "/results/0/name")}),
        ),
        7 => {
            assert!(
                messages_contain(view, RECOVERY_GUIDANCE),
                "budget exhaustion must report the exact missing caller and recovery action"
            );
            tool_batch(&[
                (
                    "recovery-broad-search-denied",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "broad repository architecture inventory"}),
                ),
                (
                    "recovery-duplicate-refinement-denied",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "worker_slot"}),
                ),
            ])
        }
        9 => {
            assert!(
                view.messages
                    .iter()
                    .filter(|message| message.content.contains(RECOVERY_GUIDANCE))
                    .count()
                    >= 2,
                "broad and duplicate recovery attempts must be denied with closed guidance"
            );
            tool_reply(
                "recover-current-root-caller",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": result_at(view, "/callers/0/qualified_name"),
                    "decision_evidence_kind": "caller",
                }),
            )
        }
        10 => {
            assert!(
                messages_contain(view, DECISION_ANCHOR_CONVERGENCE_MESSAGE),
                "targeted caller recovery must complete the typed chain"
            );
            tool_batch(&[
                (
                    "post-completion-broad-search-denied",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "post completion inventory"}),
                ),
                (
                    "post-completion-duplicate-refinement-denied",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "worker_slot"}),
                ),
                (
                    "post-completion-duplicate-source-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": "DeliveryRouter::worker_for"}),
                ),
            ])
        }
        13 => {
            assert_eq!(
                view.messages
                    .iter()
                    .filter(|message| message
                        .content
                        .contains(CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE))
                    .count(),
                3,
                "all post-completion graph attempts must be denied locally"
            );
            tool_reply(
                "read-route-after-recovery",
                "read",
                serde_json::json!({"path": "demo/src/route.rs"}),
            )
        }
        14 => tool_reply(
            "patch-retry-affinity-after-recovery",
            "apply_patch",
            serde_json::json!({
                "patch": "diff --git a/demo/src/route.rs b/demo/src/route.rs\n--- a/demo/src/route.rs\n+++ b/demo/src/route.rs\n@@ -3,11 +3,7 @@ use crate::DeliveryAttempt;\n pub(crate) fn worker_slot(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {\n     assert!(workers > 0, \"at least one delivery worker is required\");\n \n-    let routing_topic = if attempt.attempt == 0 {\n-        attempt.affinity_topic()\n-    } else {\n-        attempt.topic\n-    };\n+    let routing_topic = attempt.affinity_topic();\n     let mut hash = 0xcbf29ce484222325_u64;\n     for byte in attempt\n         .tenant\n"
            }),
        ),
        15 => tool_reply(
            "validate-recovered-minimal-repair",
            "bash",
            serde_json::json!({"command": "cd demo && cargo fmt --check && cargo test --quiet", "timeout": 60}),
        ),
        16 => tool_reply(
            "submit-recovered-minimal-repair",
            "submit_for_pr",
            serde_json::json!({"summary": "Consumed bounded current-root graph evidence and local convergence guidance before the minimal repair."}),
        ),
        17 => Reply::text(
            r##"{"title":"Keep alias retries on the selected worker","body":"# Implementation report\nConsumed bounded current-root graph evidence and local convergence guidance before the minimal repair. Exact recovery diagnostics, host validation, and Actions pass.","summary":"Applied and validated the minimal retry-affinity repair after bounded recovery."}"##,
        ),
        turn => panic!("unexpected mapped decision-gap recovery model turn {turn}"),
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
