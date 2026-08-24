//! Jig runtime for the mapped exact source-selection scenario.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::DECISION_ANCHOR_CONVERGENCE_MESSAGE;

use super::{ModelObservations, is_current_root_source_result, messages_contain};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected mapped exact source-selection fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start mapped exact source-selection Jig fake LLM: {error}"))
}

fn record_observations(view: &RequestView, observations: &mut ModelObservations) {
    observations.prompt_guidance_seen |= messages_contain(view, "CODEBASE MEMORY");
    observations.memory_result_seen |= provider_results(view)
        .iter()
        .any(|result| result.get("results").is_some());
    observations.graph_trace_seen |= provider_results(view)
        .iter()
        .any(|result| result.get("function").is_some());
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
            "discover-selection-root",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": "route affinity"}),
        ),
        1 => source_reply(
            "read-selection-implementation",
            implementation_target(view),
            "implementation",
        ),
        2 => tool_reply(
            "trace-selection-caller",
            "codebase_memory_trace_path",
            serde_json::json!({
                "function_name": implementation_target(view),
                "direction": "inbound",
            }),
        ),
        3 => source_reply("read-selection-caller", caller_target(view), "caller"),
        4 => tool_batch(&[
            (
                "selection-competing-generic-one",
                "codebase_memory_search_code",
                serde_json::json!({"pattern": implementation_target(view)}),
            ),
            (
                "selection-competing-generic-two",
                "codebase_memory_search_code",
                serde_json::json!({"pattern": implementation_target(view)}),
            ),
        ]),
        6 => tool_reply(
            "selection-competing-generic-irrelevant",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": implementation_target(view)}),
        ),
        7 => tool_reply(
            "selection-competing-forest-traversal",
            "codebase_memory_trace_path",
            serde_json::json!({
                "function_name": caller_target(view),
                "mode": "calls",
                "direction": "inbound",
                "include_tests": true,
                "depth": 1,
            }),
        ),
        8 => tool_reply(
            "selection-focused-test-fallback",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "focused alias retry behavior"}),
        ),
        9 => source_reply(
            "selection-mismatched-confirmation-denied",
            implementation_target(view),
            "focused_test",
        ),
        10 => {
            assert!(
                messages_contain(view, "missing evidence: [focused_test]"),
                "mismatched typed confirmation must remain fail closed"
            );
            source_reply(
                "selection-exact-focused-test-source",
                focused_test_target(view),
                "focused_test",
            )
        }
        11 => {
            assert!(
                messages_contain(view, DECISION_ANCHOR_CONVERGENCE_MESSAGE),
                "typed implementation, caller, and focused-test sources must complete first"
            );
            tool_batch(&[
                (
                    "malformed-selection-read",
                    "read",
                    serde_json::json!({"path": "repo/src/lib.rs"}),
                ),
                (
                    "interleaved-selection-file-discovery",
                    "find",
                    serde_json::json!({"path": "repo", "pattern": "*route.rs"}),
                ),
                (
                    "exact-route-selection-read",
                    "read",
                    serde_json::json!({"path": "repo/src/route.rs"}),
                ),
                (
                    "interleaved-selection-generic-search",
                    "grep",
                    serde_json::json!({
                        "path": "repo/src",
                        "pattern": "worker_slot",
                        "context": 2,
                    }),
                ),
            ])
        }
        15 => tool_reply(
            "patch-route-after-exact-selection",
            "apply_patch",
            serde_json::json!({
                "patch": "diff --git a/repo/src/route.rs b/repo/src/route.rs\n--- a/repo/src/route.rs\n+++ b/repo/src/route.rs\n@@ -3,11 +3,7 @@ use crate::DeliveryAttempt;\n pub(crate) fn worker_slot(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {\n     assert!(workers > 0, \"at least one delivery worker is required\");\n \n-    let routing_topic = if attempt.attempt == 0 {\n-        attempt.affinity_topic()\n-    } else {\n-        attempt.topic\n-    };\n+    let routing_topic = attempt.affinity_topic();\n     let mut hash = 0xcbf29ce484222325_u64;\n     for byte in attempt\n         .tenant\n"
            }),
        ),
        16 => tool_reply(
            "validate-exact-selection-repair",
            "bash",
            serde_json::json!({
                "command": "cd repo && cargo fmt --check && cargo test --quiet && git diff --check && test \"$(git diff --name-only)\" = src/route.rs && test \"$(git diff --numstat -- src/route.rs)\" = \"$(printf '1\\t5\\tsrc/route.rs')\"",
                "timeout": 60,
            }),
        ),
        17 => tool_reply(
            "submit-exact-selection-repair",
            "submit_for_pr",
            serde_json::json!({
                "summary": "Validated interleaved exact source selection and the minimal repair."
            }),
        ),
        18 => Reply::text(
            r##"{"title":"Keep alias retries on the selected worker","body":"# Implementation report\nValidated typed source evidence, interleaved exact selection, one repair, and host submission.","summary":"Applied and validated the retry-affinity repair after exact source selection."}"##,
        ),
        turn => panic!("unexpected mapped exact source-selection model turn {turn}"),
    }
}

fn source_reply(id: &str, qualified_name: String, kind: &str) -> Reply {
    tool_reply(
        id,
        "codebase_memory_get_code_snippet",
        serde_json::json!({
            "qualified_name": qualified_name,
            "decision_evidence_kind": kind,
        }),
    )
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

fn implementation_target(view: &RequestView) -> String {
    provider_results(view)
        .iter()
        .find_map(|result| result.pointer("/results/0/qualified_name"))
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .expect("targeted root omitted its implementation selector")
}

fn caller_target(view: &RequestView) -> String {
    provider_results(view)
        .iter()
        .find_map(|result| result.pointer("/callers/0/qualified_name"))
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .expect("implementation trace omitted its caller selector")
}

fn focused_test_target(view: &RequestView) -> String {
    provider_results(view)
        .iter()
        .rev()
        .filter_map(|result| result.get("results").and_then(JsonValue::as_array))
        .flatten()
        .find(|result| result.get("is_test").and_then(JsonValue::as_bool) == Some(true))
        .and_then(|result| result.get("qualified_name").and_then(JsonValue::as_str))
        .map(str::to_string)
        .expect("semantic fallback omitted its exact focused-test selector")
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
