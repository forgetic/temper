use super::super::tool::graph_correlation;
use serde_json::json;
use temper_protocol_activity::{
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
    MAX_GRAPH_CORRELATION_TARGET_BYTES,
};

#[test]
fn codebase_memory_correlation_extracts_only_closed_complete_structured_targets() {
    const SECRET: &str = "Authorization: Bearer WRAPPER-GRAPH-SECRET";
    let cases = [
        (
            "codebase_memory_search_graph",
            json!({"query": format!("  activity   {SECRET}  ")}),
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::GraphQuery,
            format!("activity {SECRET}"),
        ),
        (
            "codebase_memory_search_graph",
            json!({"name_pattern": "Normalizer::tool_finished"}),
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::NamePattern,
            "Normalizer::tool_finished".to_string(),
        ),
        (
            "codebase_memory_search_graph",
            json!({"qn_pattern": "crate::activity::.*"}),
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::QualifiedNamePattern,
            "crate::activity::.*".to_string(),
        ),
        (
            "codebase_memory_search_code",
            json!({"pattern": "ToolFinishedV1"}),
            GraphCorrelationToolV1::SearchCode,
            GraphCorrelationTargetKindV1::Pattern,
            "ToolFinishedV1".to_string(),
        ),
        (
            "codebase_memory_trace_path",
            json!({"function_name": "normalize_activity"}),
            GraphCorrelationToolV1::TracePath,
            GraphCorrelationTargetKindV1::FunctionName,
            "normalize_activity".to_string(),
        ),
        (
            "codebase_memory_get_code_snippet",
            json!({"qualified_name": "crate::activity::normalize"}),
            GraphCorrelationToolV1::GetCodeSnippet,
            GraphCorrelationTargetKindV1::QualifiedName,
            "crate::activity::normalize".to_string(),
        ),
    ];

    for (public_name, input, tool, target_kind, declared_target) in cases {
        let correlation = graph_correlation(public_name, &input).expect("closed target extracted");
        assert_eq!(correlation.tool, tool);
        assert_eq!(correlation.target_kind, target_kind);
        assert_eq!(
            correlation.target_digest,
            GraphCorrelationV1::target_digest(&declared_target).expect("complete target")
        );
        let rendered = serde_json::to_string(&correlation).expect("correlation serializes");
        assert!(!rendered.contains(SECRET));
        assert!(!rendered.contains(&declared_target));
    }

    for (public_name, input) in [
        (
            "codebase_memory_search_code",
            json!({"query": "not allowlisted"}),
        ),
        (
            "codebase_memory_search_code",
            json!({"pattern": ["not", "a string"]}),
        ),
        (
            "codebase_memory_search_graph",
            json!({"query": "one", "name_pattern": "ambiguous"}),
        ),
        (
            "codebase_memory_search_graph",
            json!({"query": "bad\ncontrol"}),
        ),
        (
            "codebase_memory_trace_path",
            json!({"function_name": "x".repeat(MAX_GRAPH_CORRELATION_TARGET_BYTES + 1)}),
        ),
        (
            "codebase_memory_get_architecture",
            json!({"query": "not targeted"}),
        ),
    ] {
        assert_eq!(graph_correlation(public_name, &input), None);
    }
}
