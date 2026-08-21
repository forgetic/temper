//! Live Jig runtime for feature #1041's graph-to-ordinary convergence proof.
//!
//! Provider arguments and fixture source remain transient. The runtime emits a
//! fixed minimal repair after graph discovery, graph-local closure, invocation
//! rejection, and one circuit redirect have all been observed.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;

use super::{
    BOUNDED_GRAPH_RESULT_NEEDLE, MAX_MODEL_MESSAGE_BYTES, ModelObservations,
    RAW_PROVIDER_FAILURE_NEEDLE, SAFE_PROVIDER_FAILURE, messages_contain,
};

const FAILED_COMMAND: &str = "counter=demo/.git/ordinary-tool-attempts; grep -q 'if attempt == 0' demo/src/lib.rs || exit 8; printf 'attempt\\n' >> \"$counter\"; exit 9";
const REPAIRED_SOURCE: &str = "pub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n    preferred.unwrap_or(value)\n}\n";

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected ordinary-convergence fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record_observations(view, &mut observations.lock().expect("observations lock"));
        reply(view)
    }))
    .map_err(|error| format!("start ordinary-convergence Jig fake LLM: {error}"))
}

fn record_observations(view: &RequestView, observations: &mut ModelObservations) {
    observations.prompt_guidance_seen |= messages_contain(view, "CODEBASE MEMORY");
    // Jig's normalized Anthropic request view counts tool_result blocks but
    // deliberately omits their nested text. The temporary MCP validator owns
    // target/value consumption; this observer retains only bounded stage facts.
    observations.memory_result_seen |= view.prior_tool_results >= 1;
    observations.code_refinement_seen |= view.prior_tool_results >= 2;
    observations.graph_trace_seen |= view.prior_tool_results >= 3;
    observations.current_root_source_seen |= view.prior_tool_results >= 4;
    observations.current_root_source_results = observations
        .current_root_source_results
        .max(view.prior_tool_results.saturating_sub(3).min(2));
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
            "discover-convergence-routing-root",
            "codebase_memory_search_graph",
            serde_json::json!({"query": "worker affinity routing"}),
        ),
        1 => tool_reply(
            "refine-convergence-routing-symbol",
            "codebase_memory_search_code",
            serde_json::json!({"pattern": "worker_slot"}),
        ),
        2 => tool_reply(
            "trace-convergence-routing-caller",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": "worker_slot"}),
        ),
        3 => tool_reply(
            "read-convergence-current-root-implementation",
            "codebase_memory_get_code_snippet",
            serde_json::json!({
                "qualified_name": "fixture::delivery::DeliveryAttempt",
                "decision_evidence_kind": "caller",
            }),
        ),
        4 => tool_reply(
            "read-convergence-current-root-focused-test",
            "codebase_memory_get_code_snippet",
            serde_json::json!({
                "qualified_name": "fixture::delivery::worker_for",
                "decision_evidence_kind": "focused_test",
            }),
        ),
        5 => tool_reply(
            "close-convergence-graph-exploration",
            "codebase_memory_get_code_snippet",
            serde_json::json!({"qualified_name": "fixture::unavailable"}),
        ),
        6 => tool_reply(
            "provider-native-read-after-closure",
            "Read",
            serde_json::json!({"file_path": "demo/src/lib.rs"}),
        ),
        7 => tool_reply(
            "reject-ambiguous-provider-native-write",
            "Write",
            serde_json::json!({
                "path": "demo/src/lib.rs",
                "file_path": "demo/src/lib.rs",
                "content": REPAIRED_SOURCE,
            }),
        ),
        8 => tool_reply(
            "ordinary-failure-first-execution",
            "Bash",
            serde_json::json!({"command": FAILED_COMMAND, "timeout": 60}),
        ),
        9 => tool_reply(
            "ordinary-failure-identical-retry",
            "Bash",
            serde_json::json!({"command": FAILED_COMMAND, "timeout": 60}),
        ),
        10 => tool_reply(
            "ordinary-failure-corrected-counter-check",
            "Bash",
            serde_json::json!({
                "command": "test \"$(wc -l < demo/.git/ordinary-tool-attempts)\" -eq 1 && grep -q 'if attempt == 0' demo/src/lib.rs",
                "timeout": 60,
            }),
        ),
        11 => tool_reply(
            "corrected-provider-native-write",
            "Write",
            serde_json::json!({
                "file_path": "demo/src/lib.rs",
                "content": REPAIRED_SOURCE,
            }),
        ),
        12 => tool_reply(
            "validate-corrected-minimal-diff",
            "Bash",
            serde_json::json!({
                "command": "cd demo && cargo fmt --check && cargo test --quiet && test \"$(git diff --name-only)\" = src/lib.rs && git diff --check",
                "timeout": 60,
            }),
        ),
        13 => tool_reply(
            "submit-corrected-minimal-diff",
            "submit_for_pr",
            serde_json::json!({
                "summary": "Used codebase-memory graph evidence, then validated the retry-worker repair."
            }),
        ),
        14 => Reply::text(
            r##"{"title":"Keep selected dispatch behavior","body":"# Implementation report\nUsed codebase-memory graph evidence, then validated the retry-worker repair. The graph-local denial, canonical invocation diagnostics, bounded ordinary circuit, focused shell checks, and host submission gate all passed.","summary":"Converged after one bounded ordinary-tool redirect."}"##,
        ),
        turn => panic!("unexpected ordinary-convergence model turn {turn}"),
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
