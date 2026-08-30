//! Jig runtime for the mapped exact source-selection scenario.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE;

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
        1 => tool_batch(&[
            (
                "early-route-read-before-source",
                "read",
                serde_json::json!({"path": "repo/src/route.rs"}),
            ),
            (
                "early-caller-read-before-source",
                "read",
                serde_json::json!({"path": "repo/src/lib.rs"}),
            ),
            (
                "early-focused-test-read-before-source",
                "read",
                serde_json::json!({"path": "repo/tests/alias_retry.rs"}),
            ),
        ]),
        4 => source_reply(
            "read-selection-implementation",
            implementation_target(view),
            "implementation",
        ),
        5 => tool_reply(
            "trace-selection-caller",
            "codebase_memory_trace_path",
            serde_json::json!({
                "function_name": implementation_target(view),
                "direction": "inbound",
            }),
        ),
        6 => source_reply("read-selection-caller", caller_target(view), "caller"),
        7 => tool_batch(&[
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
        9 => tool_reply(
            "selection-competing-generic-irrelevant",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": implementation_target(view)}),
        ),
        10 => tool_reply(
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
        11 => tool_reply(
            "selection-focused-test-fallback",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "focused alias retry behavior"}),
        ),
        12 => source_reply(
            "selection-mismatched-confirmation-denied",
            implementation_target(view),
            "focused_test",
        ),
        13 => source_reply(
            "selection-exact-focused-test-source",
            focused_test_target(view),
            "focused_test",
        ),
        14 => patch_reply("patch-route-before-post-source-read"),
        15 => {
            let denial = latest_tool_result(view);
            assert!(
                denial.contains(DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE),
                "direct mutation must return actionable exact-read guidance"
            );
            assert!(
                !denial.contains("let routing_topic = attempt.affinity_topic();"),
                "local denial must not retain mutation content"
            );
            tool_reply(
                "read-route-after-source-evidence",
                "read",
                serde_json::json!({"path": "repo/src/route.rs"}),
            )
        }
        16 => {
            assert!(
                latest_tool_result(view).contains("let routing_topic = if attempt.attempt == 0"),
                "the denied direct mutation must leave the route source unchanged"
            );
            patch_reply("patch-route-after-post-source-read")
        }
        17 => tool_reply(
            "validate-post-source-read-repair",
            "bash",
            serde_json::json!({
                "command": "cd repo && cargo fmt --check && cargo test --quiet && git diff --check && test \"$(git diff --name-only)\" = src/route.rs && test \"$(git diff --numstat -- src/route.rs)\" = \"$(printf '1\\t5\\tsrc/route.rs')\"",
                "timeout": 60,
            }),
        ),
        18 => tool_reply(
            "submit-post-source-read-repair",
            "submit_for_pr",
            serde_json::json!({
                "summary": "Validated denied-then-recovered post-source exact selection and the minimal repair."
            }),
        ),
        19 => Reply::text(
            r##"{"title":"Keep alias retries on the selected worker","body":"# Implementation report\nValidated typed source evidence, local precondition denial, post-source exact selection, one repair, and host submission.","summary":"Applied and validated the retry-affinity repair only after the required post-source exact read."}"##,
        ),
        turn => panic!("unexpected mapped exact source-selection model turn {turn}"),
    }
}

fn patch_reply(id: &str) -> Reply {
    tool_reply(id, "apply_patch", patch_args())
}

fn patch_args() -> JsonValue {
    serde_json::json!({
        "patch": "diff --git a/repo/src/route.rs b/repo/src/route.rs\n--- a/repo/src/route.rs\n+++ b/repo/src/route.rs\n@@ -3,11 +3,7 @@ use crate::DeliveryAttempt;\n pub(crate) fn worker_slot(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {\n     assert!(workers > 0, \"at least one delivery worker is required\");\n \n-    let routing_topic = if attempt.attempt == 0 {\n-        attempt.affinity_topic()\n-    } else {\n-        attempt.topic\n-    };\n+    let routing_topic = attempt.affinity_topic();\n     let mut hash = 0xcbf29ce484222325_u64;\n     for byte in attempt\n         .tenant\n"
    })
}

fn latest_tool_result(view: &RequestView) -> &str {
    view.messages
        .iter()
        .rev()
        .find(|message| message.role == "tool")
        .map(|message| message.content.as_str())
        .expect("expected a prior tool result")
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
