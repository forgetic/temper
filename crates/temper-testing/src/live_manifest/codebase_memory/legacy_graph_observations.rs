//! Model-visible evidence for the original six-call graph-consumption fixture.
//!
//! The production source guard removes provider markers and binding claims.
//! Observe the selected identities and exact fixture source instead; the live
//! MCP contract separately verifies checkout binding and the six-call order.

use super::{ModelObservations, RequestView};
use serde_json::Value;

struct ExpectedSource {
    selector: &'static str,
    symbol_field: &'static str,
    symbol: &'static str,
    path: &'static str,
    source: &'static str,
}

const SOURCES: [ExpectedSource; 3] = [
    ExpectedSource {
        selector: "/results/0/results/0",
        symbol_field: "qualifiedName",
        symbol: "retry_worker_topic",
        path: "src/lib.rs",
        source: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/codebase-memory-graph-consumption/repo/src/lib.rs"
        )),
    },
    ExpectedSource {
        selector: "/callers/0",
        symbol_field: "qualified_name",
        symbol: "dispatch",
        path: "src/caller.rs",
        source: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/codebase-memory-graph-consumption/repo/src/caller.rs"
        )),
    },
    ExpectedSource {
        selector: "/results/0/results/1",
        symbol_field: "qualifiedName",
        symbol: "alias_retries_keep_the_original_ordered_worker",
        path: "tests/retry_affinity.rs",
        source: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/codebase-memory-graph-consumption/repo/tests/retry_affinity.rs"
        )),
    },
];

pub(super) fn record(view: &RequestView, observations: &mut ModelObservations) {
    let results: Vec<Value> = view
        .messages
        .iter()
        .filter(|message| message.role == "tool")
        .filter_map(|message| {
            let content = message
                .content
                .split_once("\n\n[Decision anchor:")
                .map_or(message.content.as_str(), |(result, _)| result);
            serde_json::from_str(content).ok()
        })
        .collect();
    observations.memory_result_seen |= results
        .iter()
        .any(|result| selected(result, &SOURCES[0]) && selected(result, &SOURCES[2]));
    observations.code_refinement_seen |= results.iter().any(|result| {
        result.pointer("/results/0").is_some_and(|reference| {
            identity(reference, "qualified_name", &SOURCES[0])
                && reference
                    .pointer("/related_source_references/0/qualifiedName")
                    .and_then(Value::as_str)
                    == Some(SOURCES[1].symbol)
        })
    });
    observations.graph_trace_seen |= results.iter().any(|result| {
        result
            .pointer("/function/qualified_name")
            .and_then(Value::as_str)
            == Some(SOURCES[0].symbol)
            && result.get("direction").and_then(Value::as_str) == Some("inbound")
            && result.get("complete").and_then(Value::as_bool) == Some(true)
            && selected(result, &SOURCES[1])
            && result
                .pointer("/related_sources/0")
                .is_some_and(|reference| identity(reference, "qualified_name", &SOURCES[1]))
    });
    let count = verified_source_count(&results);
    observations.current_root_source_seen |= count > 0;
    // A conversation repeats earlier results. Count distinct source roles once.
    observations.current_root_source_results = observations.current_root_source_results.max(count);
}

fn identity(reference: &Value, symbol_field: &str, expected: &ExpectedSource) -> bool {
    reference.get(symbol_field).and_then(Value::as_str) == Some(expected.symbol)
        && reference.get("file_path").and_then(Value::as_str) == Some(expected.path)
}

fn selected(result: &Value, expected: &ExpectedSource) -> bool {
    result
        .pointer(expected.selector)
        .is_some_and(|reference| identity(reference, expected.symbol_field, expected))
}

fn verified_source_count(results: &[Value]) -> usize {
    SOURCES
        .iter()
        .filter(|expected| {
            results.iter().enumerate().any(|(index, result)| {
                identity(result, "qualified_name", expected)
                    && result.get("source").and_then(Value::as_str) == Some(expected.source)
                    && results[..index]
                        .iter()
                        .any(|prior| selected(prior, expected))
            })
        })
        .count()
}

#[cfg(test)]
#[path = "legacy_graph_observations_tests.rs"]
mod tests;
