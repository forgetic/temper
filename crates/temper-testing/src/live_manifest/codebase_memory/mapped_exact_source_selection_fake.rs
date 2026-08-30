//! Jig runtime for the mapped decision-evidence convergence scenario.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::{
    CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE, DECISION_ANCHOR_CONVERGENCE_MESSAGE,
    DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE,
};

use super::{ModelObservations, is_current_root_source_result, messages_contain};

const INCOMPLETE_GUIDANCE: &str = "decision-evidence recovery required; missing evidence: [trace, implementation, caller, focused_test]; permitted action: targeted_current_root_graph_call; remaining allowance: 4";

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected mapped decision-evidence convergence request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start mapped decision-evidence convergence Jig: {error}"))
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
                "discover-convergence-routing-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "routing implementation affinity"}),
            ),
            (
                "discover-convergence-focused-test-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "focused alias retry behavior"}),
            ),
        ]),
        2 => tool_reply(
            "early-route-read-before-complete-evidence",
            "read",
            serde_json::json!({"path": "repo/src/route.rs"}),
        ),
        3 => tool_batch(&[
            (
                "recovery-cross-root-caller-denied",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": root_result_at(view, 1, 0, "qualifiedName"),
                    "decision_evidence_kind": "caller",
                }),
            ),
            (
                "recovery-irrelevant-broad-denied",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "unscoped inventory"}),
            ),
            (
                "recovery-active-root-implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": root_result_at(view, 0, 0, "qualifiedName"),
                    "decision_evidence_kind": "implementation",
                }),
            ),
            (
                "recovery-malformed-selector-denied",
                "codebase_memory_trace_path",
                serde_json::json!({"function_name": "src/private.rs::shared()"}),
            ),
            (
                "recovery-failed-duplicate-implementation-denied",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": root_result_at(view, 1, 0, "qualifiedName"),
                    "decision_evidence_kind": "implementation",
                }),
            ),
        ]),
        8 => {
            assert!(
                view.messages
                    .iter()
                    .filter(|message| message.content.contains(INCOMPLETE_GUIDANCE))
                    .count()
                    >= 4,
                "cross-root, broad, and malformed attempts must share the incomplete snapshot"
            );
            tool_batch(&[
                (
                    "recovery-active-root-trace",
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "function_name": recovery_selector(view, "implementation_evidence_result"),
                    }),
                ),
                (
                    "recovery-satisfied-implementation-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": root_result_at(view, 0, 0, "qualifiedName"),
                        "decision_evidence_kind": "implementation",
                    }),
                ),
            ])
        }
        10 => source_reply(
            "recovery-active-root-caller",
            recovery_selector(view, "caller_traversal_result"),
            "caller",
        ),
        11 => source_reply(
            "recovery-focused-test-root-source",
            recovery_selector(view, "focused_test_result"),
            "focused_test",
        ),
        12 => {
            assert!(
                messages_contain(view, DECISION_ANCHOR_CONVERGENCE_MESSAGE),
                "the active-root source batch must complete the retained forest"
            );
            tool_batch(&[
                (
                    "post-completion-broad-architecture-denied",
                    "codebase_memory_get_architecture",
                    serde_json::json!({}),
                ),
                (
                    "post-completion-irrelevant-search-denied",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "irrelevant completed inventory"}),
                ),
                (
                    "post-completion-selectorless-source-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": root_result_at(view, 0, 1, "qualifiedName"),
                    }),
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
                "all post-completion graph activity must remain local"
            );
            patch_reply("patch-route-before-post-source-read")
        }
        16 => {
            let denial = latest_tool_result(view);
            assert!(denial.contains(DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE));
            assert!(!denial.contains("let routing_topic = attempt.affinity_topic();"));
            tool_reply(
                "read-route-after-complete-source-evidence",
                "read",
                serde_json::json!({"path": "repo/src/route.rs"}),
            )
        }
        17 => {
            assert!(
                latest_tool_result(view).contains("let routing_topic = if attempt.attempt == 0"),
                "the denied mutation must leave the exact target unchanged"
            );
            patch_reply("patch-route-after-complete-source-evidence")
        }
        18 => tool_reply(
            "validate-converged-repair",
            "bash",
            serde_json::json!({
                "command": "cd repo && cargo fmt --check && cargo test --quiet && git diff --check && test \"$(git diff --name-only)\" = src/route.rs && test \"$(git diff --numstat -- src/route.rs)\" = \"$(printf '1\\t5\\tsrc/route.rs')\"",
                "timeout": 60,
            }),
        ),
        19 => tool_reply(
            "submit-converged-repair",
            "submit_for_pr",
            serde_json::json!({
                "summary": "Validated complete retained decision evidence, one exact read, and the minimal repair."
            }),
        ),
        20 => Reply::text(
            r##"{"title":"Keep alias retries on the selected worker","body":"# Implementation report\nValidated deterministic root-local evidence recovery, the post-source exact read, one repair, host submission, and Actions.","summary":"Applied and validated the retry-affinity repair after complete decision-evidence convergence."}"##,
        ),
        turn => panic!("unexpected mapped decision-evidence convergence turn {turn}"),
    }
}

fn patch_reply(id: &str) -> Reply {
    tool_reply(
        id,
        "apply_patch",
        serde_json::json!({
            "patch": "diff --git a/repo/src/route.rs b/repo/src/route.rs\n--- a/repo/src/route.rs\n+++ b/repo/src/route.rs\n@@ -3,11 +3,7 @@ use crate::DeliveryAttempt;\n pub(crate) fn worker_slot(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {\n     assert!(workers > 0, \"at least one delivery worker is required\");\n \n-    let routing_topic = if attempt.attempt == 0 {\n-        attempt.affinity_topic()\n-    } else {\n-        attempt.topic\n-    };\n+    let routing_topic = attempt.affinity_topic();\n     let mut hash = 0xcbf29ce484222325_u64;\n     for byte in attempt\n         .tenant\n"
        }),
    )
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

fn recovery_selector(view: &RequestView, label: &str) -> String {
    let marker = format!("{label}=temper-recovery-selector:");
    view.messages
        .iter()
        .rev()
        .find_map(|message| {
            let start = message.content.find(&marker)? + label.len() + 1;
            let value = &message.content[start..];
            let end = value.find([',', ' ', ']', '.']).unwrap_or(value.len());
            Some(value[..end].to_string())
        })
        .unwrap_or_else(|| panic!("missing provider-derived {label} reference"))
}

fn latest_tool_result(view: &RequestView) -> &str {
    view.messages
        .iter()
        .rev()
        .find(|message| message.role == "tool")
        .map(|message| message.content.as_str())
        .expect("expected a prior tool result")
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

fn root_result_at(
    view: &RequestView,
    root_index: usize,
    result_index: usize,
    field: &str,
) -> String {
    let pointer = format!("/results/0/results/{result_index}/{field}");
    root_results(view)
        .nth(root_index)
        .and_then(|result| {
            result
                .pointer(&pointer)
                .and_then(JsonValue::as_str)
                .map(str::to_string)
        })
        .expect("independent root omitted an approved later-turn selector")
}

fn root_results(view: &RequestView) -> impl Iterator<Item = JsonValue> {
    let mut roots = provider_results(view)
        .into_iter()
        .filter(|result| {
            result
                .pointer("/results/0/results/0/qualifiedName")
                .is_some()
        })
        .collect::<Vec<_>>();
    roots.sort_by_key(|result| {
        std::cmp::Reverse(
            result
                .pointer("/results/0/results")
                .and_then(JsonValue::as_array)
                .map_or(0, Vec::len),
        )
    });
    roots.into_iter()
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
