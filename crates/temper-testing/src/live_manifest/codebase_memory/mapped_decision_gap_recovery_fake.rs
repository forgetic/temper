//! Jig runtime for #1275's route-specific strengthening of mapped recovery.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::{
    CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE, DECISION_ANCHOR_CONVERGENCE_MESSAGE,
};

use super::{ModelObservations, is_current_root_source_result, messages_contain};

const MIXED_RECOVERY_GUIDANCE: &str = "decision-evidence recovery required; missing evidence: [trace, implementation, caller, focused_test]; permitted action: targeted_current_root_graph_call; remaining allowance: 4";
const DENIED_CANARY_COMMAND: &str =
    "printf 'executed\\n' > demo/.git/decision-gap-denied-shell-canary";
const ROOT_A_QUERY: &str = "routing implementation affinity";
const ROOT_B_QUERY: &str = "focused alias retry behavior";

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
    observations.memory_result_seen |= root_results(view).next().is_some();
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
        0 => tool_batch(&[
            (
                "discover-active-routing-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "routing implementation affinity"}),
            ),
            (
                "discover-sibling-behavior-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "focused alias retry behavior"}),
            ),
            (
                "denied-shell-process-canary",
                "bash",
                serde_json::json!({"command": DENIED_CANARY_COMMAND, "timeout": 60}),
            ),
        ]),
        3 => source_reply(
            "prime-cross-root-recovery-guidance",
            root_result_for(view, ROOT_B_QUERY, 0, "qualifiedName"),
            "caller",
        ),
        4 => {
            assert!(
                messages_contain(view, MIXED_RECOVERY_GUIDANCE),
                "two roots must begin with all active-root kinds missing"
            );
            tool_batch(&[
                (
                    "recovery-cross-root-caller-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": root_result_for(view, ROOT_B_QUERY, 0, "qualifiedName"),
                        "decision_evidence_kind": "caller",
                    }),
                ),
                (
                    "first-root-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": root_result_for(view, ROOT_A_QUERY, 0, "qualifiedName"),
                        "decision_evidence_kind": "implementation",
                    }),
                ),
                (
                    "recovery-cross-root-focused-test-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": root_result_for(view, ROOT_B_QUERY, 0, "qualifiedName"),
                        "decision_evidence_kind": "focused_test",
                    }),
                ),
            ])
        }
        7 => {
            assert!(
                view.messages
                    .iter()
                    .filter(|message| message.content.contains(MIXED_RECOVERY_GUIDANCE))
                    .count()
                    >= 2,
                "both cross-root siblings must retain the immutable pre-batch diagnostic"
            );
            tool_reply(
                "first-root-empty-trace",
                "codebase_memory_trace_path",
                serde_json::json!({"function_name": root_result_for(view, ROOT_A_QUERY, 0, "name")}),
            )
        }
        8 => source_reply(
            "second-root-implementation",
            root_result_for(view, ROOT_B_QUERY, 0, "qualifiedName"),
            "implementation",
        ),
        9 => tool_reply(
            "second-root-trace",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": root_result_for(view, ROOT_B_QUERY, 0, "name")}),
        ),
        10 => source_reply(
            "second-root-caller",
            root_result_for(view, ROOT_B_QUERY, 1, "qualifiedName"),
            "caller",
        ),
        11 => source_reply(
            "first-root-focused-test",
            root_result_for(view, ROOT_A_QUERY, 2, "qualifiedName"),
            "focused_test",
        ),
        12 => {
            assert!(
                messages_contain(view, DECISION_ANCHOR_CONVERGENCE_MESSAGE),
                "the independent focused route must complete the retained forest"
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
        15 => {
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
            tool_batch(&[
                (
                    "patch-before-exact-read-denied",
                    "apply_patch",
                    route_patch_arguments(),
                ),
                (
                    "unrelated-mutation-denied",
                    "write",
                    serde_json::json!({"path": "demo/src/unrelated.rs", "content": "changed"}),
                ),
            ])
        }
        17 => tool_reply(
            "classify-compound-shell-discovery",
            "bash",
            serde_json::json!({
                "command": "cd demo && rg worker_slot src",
                "timeout": 60,
            }),
        ),
        18 => tool_reply(
            "read-route-after-recovery",
            "read",
            serde_json::json!({"path": "demo/src/route.rs"}),
        ),
        19 => tool_reply(
            "patch-retry-affinity-after-recovery",
            "apply_patch",
            route_patch_arguments(),
        ),
        20 => tool_reply(
            "validate-exact-recovered-repair",
            "bash",
            serde_json::json!({
                "command": "test ! -e demo/.git/decision-gap-denied-shell-canary && cd demo && cargo fmt --check && cargo test --quiet && git diff --check && test \"$(git diff --name-only)\" = src/route.rs && test \"$(git diff --numstat -- src/route.rs)\" = \"$(printf '1\\t5\\tsrc/route.rs')\"",
                "timeout": 60,
            }),
        ),
        21 => tool_reply(
            "submit-recovered-minimal-repair",
            "submit_for_pr",
            serde_json::json!({
                "summary": "Validated root-coherent recovery, exact repair, and host gates."
            }),
        ),
        22 => Reply::text(
            r##"{"title":"Keep alias retries on the selected worker","body":"# Implementation report\nValidated immutable root-local recovery, progress-based missing kinds, one exact repair, host submission, and Actions.","summary":"Applied and validated the exact retry-affinity repair after root-coherent recovery."}"##,
        ),
        turn => panic!("unexpected mapped decision-gap recovery model turn {turn}"),
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

fn route_patch_arguments() -> JsonValue {
    serde_json::json!({
        "patch": "diff --git a/demo/src/route.rs b/demo/src/route.rs\n--- a/demo/src/route.rs\n+++ b/demo/src/route.rs\n@@ -3,11 +3,7 @@ use crate::DeliveryAttempt;\n pub(crate) fn worker_slot(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {\n     assert!(workers > 0, \"at least one delivery worker is required\");\n \n-    let routing_topic = if attempt.attempt == 0 {\n-        attempt.affinity_topic()\n-    } else {\n-        attempt.topic\n-    };\n+    let routing_topic = attempt.affinity_topic();\n     let mut hash = 0xcbf29ce484222325_u64;\n     for byte in attempt\n         .tenant\n"
    })
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

fn root_result_for(
    view: &RequestView,
    root_query: &str,
    result_index: usize,
    field: &str,
) -> String {
    let pointer = format!("/results/0/results/{result_index}/{field}");
    root_results(view)
        .find(|result| {
            result
                .pointer("/results/0/root_query")
                .and_then(JsonValue::as_str)
                == Some(root_query)
        })
        .and_then(|result| {
            result
                .pointer(&pointer)
                .and_then(JsonValue::as_str)
                .map(str::to_string)
        })
        .expect("independent root omitted an approved later-turn selector")
}

fn root_results(view: &RequestView) -> impl Iterator<Item = JsonValue> {
    provider_results(view).into_iter().filter(|result| {
        result
            .pointer("/results/0/results/0/qualifiedName")
            .is_some()
    })
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
