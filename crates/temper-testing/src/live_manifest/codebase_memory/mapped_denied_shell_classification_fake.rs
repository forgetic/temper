//! Jig runtime for feature #1082's mapped denied-shell classification proof.
//!
//! The first model turn emits a graph read followed by a shell process barrier.
//! Raw invocation data and provider values remain transient; the scenario
//! retains only closed lifecycle and graph-lineage facts.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;

use super::{ModelObservations, is_current_root_source_result, messages_contain};

const DENIED_CANARY_COMMAND: &str = "printf 'executed\\n' > demo/.git/denied-shell-process-canary";
const REPAIRED_SOURCE: &str = "pub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n    preferred.unwrap_or(value)\n}\n";

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected denied-shell classification fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start denied-shell classification Jig fake LLM: {error}"))
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
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_batch(&[
            (
                "discover-routing-before-denied-shell",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "worker affinity routing"}),
            ),
            (
                "denied-shell-process-canary",
                "bash",
                serde_json::json!({"command": DENIED_CANARY_COMMAND, "timeout": 60}),
            ),
        ]),
        2 => tool_reply(
            "refine-routing-after-denied-shell",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": result_at(view, "/results/0/results/0/name")}),
        ),
        3 => tool_reply(
            "trace-routing-caller-model",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": result_at(view, "/results/0/name")}),
        ),
        4 => tool_reply(
            "read-current-root-caller-model",
            "codebase_memory_get_code_snippet",
            serde_json::json!({
                "qualified_name": result_at(view, "/callers/0/qualified_name"),
                "decision_evidence_kind": "caller",
            }),
        ),
        5 => tool_reply(
            "read-current-root-focused-test",
            "codebase_memory_get_code_snippet",
            serde_json::json!({
                "qualified_name": result_at(view, "/source_metadata/related_source_references/0/qualifiedName"),
                "decision_evidence_kind": "focused_test",
            }),
        ),
        6 => {
            assert_complete_source_evidence(view);
            tool_reply(
                "read-selected-source-after-complete-chain",
                "read",
                serde_json::json!({"path": "demo/src/lib.rs"}),
            )
        }
        7 => tool_reply(
            "write-minimal-denied-shell-repair",
            "write",
            serde_json::json!({"path": "demo/src/lib.rs", "content": REPAIRED_SOURCE}),
        ),
        8 => tool_reply(
            "validate-repair-and-process-canary",
            "bash",
            serde_json::json!({
                "command": "test ! -e demo/.git/denied-shell-process-canary && cd demo && cargo fmt --check && cargo test --quiet && test \"$(git diff --name-only)\" = src/lib.rs && git diff --check",
                "timeout": 60,
            }),
        ),
        9 => tool_reply(
            "submit-denied-shell-classification-repair",
            "submit_for_pr",
            serde_json::json!({
                "summary": "Used codebase-memory graph evidence, then validated the retry-worker repair."
            }),
        ),
        10 => Reply::text(
            r##"{"title":"Keep selected dispatch behavior","body":"# Implementation report\nThe first-turn shell barrier was denied locally without execution, the current-root graph chain remained consumable, and the minimal repair passed host validation and Actions.","summary":"Validated the privacy-safe denied-shell disposition and minimal repair."}"##,
        ),
        turn => panic!("unexpected denied-shell classification model turn {turn}"),
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
        "repair requires current-root caller/model and focused-test source evidence"
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
