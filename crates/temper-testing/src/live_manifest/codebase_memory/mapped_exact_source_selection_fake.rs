//! Jig runtime for the mapped post-correction authorization scenario.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent_core::CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE;

use super::{ModelObservations, is_current_root_source_result, messages_contain};

const INCOMPLETE_GUIDANCE: &str = "decision-evidence recovery required; missing evidence: [trace, implementation, caller, focused_test]; permitted action: targeted_current_root_graph_call; remaining allowance: 4";
const CORRECTION_INSPECTION_MESSAGE: &str = "ordinary exact read blocked: inspect every candidate in the bounded implementation-correction handoff without an evidence purpose";
const REFERENCE_PREFIX: &str = "temper-recovery-selector:";

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected mapped post-correction authorization request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start mapped post-correction authorization Jig: {error}"))
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
                "discover-correction-implementation-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "routing implementation authority"}),
            ),
            (
                "discover-correction-focused-test-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "focused routing regression"}),
            ),
        ]),
        2 => {
            let initial = handoff_references(view, "current-active-root candidate references");
            assert_eq!(
                initial.len(),
                4,
                "the implementation root must present four bounded candidates"
            );
            tool_batch(&[
                (
                    "preview-provisional-model",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": initial[0]}),
                ),
                (
                    "initial-sibling-root-caller-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": root_result_at(view, 1, 0, "qualifiedName"),
                        "decision_evidence_kind": "caller",
                    }),
                ),
                (
                    "initial-broad-search-denied",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "unscoped repository inventory"}),
                ),
                (
                    "initial-malformed-selector-denied",
                    "codebase_memory_trace_path",
                    serde_json::json!({"function_name": "src/private.rs::shared()"}),
                ),
                (
                    "initial-unpresented-selector-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": root_result_at(view, 0, 4, "qualifiedName"),
                        "decision_evidence_kind": "implementation",
                    }),
                ),
                (
                    "initial-unrelated-wrong-tool-denied",
                    "codebase_memory_trace_path",
                    serde_json::json!({"function_name": initial[3]}),
                ),
            ])
        }
        8 => {
            assert!(
                view.messages
                    .iter()
                    .filter(|message| message.content.contains(INCOMPLETE_GUIDANCE))
                    .count()
                    >= 4,
                "sibling-root, broad, malformed, and unpresented calls must share the incomplete snapshot"
            );
            let initial = handoff_references(view, "current-active-root candidate references");
            source_reply(
                "commit-provisional-model",
                initial[0].clone(),
                "implementation",
            )
        }
        9 => tool_reply(
            "trace-provisional-model",
            "codebase_memory_trace_path",
            serde_json::json!({
                "function_name": recovery_selector(view, "implementation_evidence_result"),
                "mode": "calls",
                "direction": "inbound",
                "include_tests": false,
            }),
        ),
        10 => source_reply(
            "read-active-root-caller",
            recovery_selector(view, "caller_traversal_result"),
            "caller",
        ),
        11 => source_reply(
            "read-focused-test-root",
            root_result_at(view, 1, 0, "qualifiedName"),
            "focused_test",
        ),
        12 => {
            assert!(
                messages_contain(
                    view,
                    "required bounded implementation correction inspection"
                ),
                "forest convergence must publish a bounded correction checkpoint"
            );
            tool_batch(&[
                (
                    "post-convergence-broad-architecture-denied",
                    "codebase_memory_get_architecture",
                    serde_json::json!({}),
                ),
                (
                    "post-convergence-unrelated-search-denied",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "unrelated exploration after convergence"}),
                ),
                (
                    "post-convergence-selectorless-source-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": root_result_at(view, 0, 3, "qualifiedName")}),
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
                "all post-convergence exploration must remain local"
            );
            tool_reply(
                "read-provisional-model-before-inspection",
                "read",
                serde_json::json!({"path": "repo/src/model.rs"}),
            )
        }
        16 => {
            let result = latest_tool_result(view);
            assert!(
                result.contains("correction_inspection_required")
                    || result.contains(CORRECTION_INSPECTION_MESSAGE),
                "the provisional exact read must require correction inspection: {result}"
            );
            let correction =
                handoff_references_with_count(view, "typed current-root correction candidates", 2);
            assert_eq!(
                correction.len(),
                2,
                "trace-supported caller and route corrections"
            );
            tool_reply(
                "inspect-correction-one",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": correction[0]}),
            )
        }
        17 => {
            let correction = handoff_references(view, "typed current-root correction candidates");
            tool_reply(
                "inspect-correction-two",
                "codebase_memory_get_code_snippet",
                serde_json::json!({"qualified_name": correction[0]}),
            )
        }
        18 => {
            assert!(
                messages_contain(view, "bounded correction inspection complete"),
                "the correction decision must occur after both previews settle"
            );
            let correction =
                handoff_references_with_count(view, "typed current-root correction candidates", 2);
            let implementation = correction_implementation_reference(view, &correction);
            source_reply(
                "select-corrected-route-implementation",
                implementation,
                "implementation",
            )
        }
        19 => {
            let correction =
                handoff_references_with_count(view, "typed current-root correction candidates", 2);
            let implementation = correction_implementation_reference(view, &correction);
            let result = latest_tool_result(view);
            assert!(result.contains("worker_slot"));
            let unchosen = correction
                .iter()
                .find(|reference| **reference != implementation)
                .expect("correction handoff must retain one unchosen candidate")
                .clone();
            tool_batch(&[
                (
                    "stale-selected-correction-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": implementation,
                        "decision_evidence_kind": "implementation",
                    }),
                ),
                (
                    "stale-unchosen-correction-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": unchosen,
                        "decision_evidence_kind": "implementation",
                    }),
                ),
            ])
        }
        21 => tool_batch(&[
            (
                "old-model-authority-mutation-denied",
                "write",
                serde_json::json!({"path": "repo/src/model.rs", "content": "changed"}),
            ),
            (
                "unrelated-mutation-denied",
                "write",
                serde_json::json!({"path": "repo/src/unrelated.rs", "content": "changed"}),
            ),
        ]),
        23 => tool_reply(
            "post-decision-graph-remains-closed",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "attempt to reopen completed correction"}),
        ),
        24 => tool_reply(
            "read-exact-corrected-route",
            "read",
            serde_json::json!({"path": "repo/src/route.rs"}),
        ),
        25 => {
            assert!(
                latest_tool_result(view).contains("let routing_topic = if attempt.attempt == 0"),
                "all denied calls must leave the corrected target unchanged"
            );
            patch_reply("patch-corrected-route")
        }
        26 => tool_reply(
            "validate-corrected-route-repair",
            "bash",
            serde_json::json!({
                "command": "cd repo && cargo fmt --check && cargo test --quiet && git diff --check && test \"$(git diff --name-only)\" = src/route.rs && test \"$(git diff --numstat -- src/route.rs)\" = \"$(printf '1\\t5\\tsrc/route.rs')\"",
                "timeout": 60,
            }),
        ),
        27 => tool_reply(
            "submit-corrected-route-repair",
            "submit_for_pr",
            serde_json::json!({
                "summary": "Validated post-convergence correction, retired stale authority, one exact route read, and one repair."
            }),
        ),
        28 => Reply::text(
            r##"{"title":"Keep alias retries on the selected worker","body":"# Implementation report\nCorrected provisional model authority to the graph-supported route after bounded inspection, retired stale authority, then validated one exact read and one route repair.","summary":"Applied and validated the retry-affinity repair through post-correction authorization."}"##,
        ),
        turn => panic!("unexpected mapped post-correction authorization turn {turn}"),
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
    let exact = format!("{label}=");
    let indexed = format!("{label}_1=");
    view.messages
        .iter()
        .rev()
        .find_map(|message| {
            let start = message
                .content
                .find(&exact)
                .map(|index| index + exact.len())
                .or_else(|| {
                    message
                        .content
                        .find(&indexed)
                        .map(|index| index + indexed.len())
                })?;
            reference_at(&message.content[start..])
        })
        .unwrap_or_else(|| panic!("missing provider-derived {label} reference"))
}

fn handoff_references(view: &RequestView, marker: &str) -> Vec<String> {
    view.messages
        .iter()
        .rev()
        .find(|message| message.content.contains(marker))
        .map(|message| references(&message.content))
        .filter(|references| !references.is_empty())
        .unwrap_or_else(|| panic!("missing {marker} handoff"))
}

fn handoff_references_with_count(view: &RequestView, marker: &str, expected: usize) -> Vec<String> {
    view.messages
        .iter()
        .rev()
        .filter(|message| message.content.contains(marker))
        .map(|message| references(&message.content))
        .find(|references| references.len() == expected)
        .unwrap_or_else(|| panic!("missing {marker} handoff with {expected} references"))
}

fn correction_implementation_reference(view: &RequestView, correction: &[String]) -> String {
    let remaining =
        handoff_references_with_count(view, "typed current-root correction candidates", 1);
    let tool_results = view
        .messages
        .iter()
        .filter(|message| message.role == "tool")
        .rev()
        .collect::<Vec<_>>();
    let second_preview = if messages_contain(view, "implementation_authority_corrected") {
        tool_results.get(1)
    } else {
        tool_results.first()
    }
    .expect("missing second correction preview");
    if second_preview.content.contains("worker_slot") {
        return remaining[0].clone();
    }
    correction
        .iter()
        .find(|reference| **reference != remaining[0])
        .expect("missing first correction reference")
        .clone()
}

fn references(text: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut remaining = text;
    while let Some(index) = remaining.find(REFERENCE_PREFIX) {
        let candidate = &remaining[index..];
        let length = REFERENCE_PREFIX.len() + 36;
        if candidate.len() < length {
            break;
        }
        let reference = candidate[..length].to_string();
        if !references.contains(&reference) {
            references.push(reference);
        }
        remaining = &candidate[length..];
    }
    references
}

fn reference_at(text: &str) -> Option<String> {
    let start = text.find(REFERENCE_PREFIX)?;
    let candidate = &text[start..];
    let length = REFERENCE_PREFIX.len() + 36;
    (candidate.len() >= length).then(|| candidate[..length].to_string())
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
