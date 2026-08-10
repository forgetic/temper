//! Deterministic decision-anchor policy regressions.

mod tests {
    use super::super::super::decision_anchor::*;
    use crate::machine::{
        SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY, SAFE_GRAPH_CORRELATION_DETAIL_KEY,
    };
    use std::collections::BTreeMap;
    use temper_protocol_activity::{
        GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
    };
    use tongs::{
        model::ToolCall,
        tools::{ToolEffects, ToolOutput},
    };

    const ROOT: &str = "00000000-0000-4000-8000-000000000001";
    const OTHER_ROOT: &str = "00000000-0000-4000-8000-000000000002";

    fn effects() -> BTreeMap<String, ToolEffects> {
        [
            ("codebase_memory_search_graph", ToolEffects::read()),
            ("codebase_memory_search_code", ToolEffects::read()),
            ("codebase_memory_trace_path", ToolEffects::read()),
            ("codebase_memory_get_code_snippet", ToolEffects::read()),
            ("write", ToolEffects::write()),
        ]
        .into_iter()
        .map(|(name, effect)| (name.to_string(), effect))
        .collect()
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({"target": "not used by the policy"}),
        }
    }

    fn correlation(name: &str) -> (GraphCorrelationToolV1, GraphCorrelationTargetKindV1) {
        match name {
            "codebase_memory_search_graph" => (
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
            ),
            "codebase_memory_search_code" => (
                GraphCorrelationToolV1::SearchCode,
                GraphCorrelationTargetKindV1::Pattern,
            ),
            "codebase_memory_trace_path" => (
                GraphCorrelationToolV1::TracePath,
                GraphCorrelationTargetKindV1::FunctionName,
            ),
            "codebase_memory_get_code_snippet" => (
                GraphCorrelationToolV1::GetCodeSnippet,
                GraphCorrelationTargetKindV1::QualifiedName,
            ),
            _ => panic!("unsupported test tool"),
        }
    }

    fn output(name: &str, root: &str, stage: DecisionAnchorLineageStageV1) -> ToolOutput {
        let (tool, kind) = correlation(name);
        let lineage = DecisionAnchorLineageV1::new(
            root.to_string(),
            stage,
            DecisionAnchorTargetKindV1::from_graph_correlation(kind),
            [
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ],
        )
        .unwrap();
        ToolOutput {
            content: Vec::new(),
            details: Some(serde_json::json!({
                SAFE_GRAPH_CORRELATION_DETAIL_KEY: GraphCorrelationV1::new(tool, kind, "request").unwrap(),
                SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: lineage,
            })),
            is_error: false,
        }
    }

    fn finish(
        state: &mut DecisionAnchorState,
        id: &str,
        name: &str,
        root: &str,
        stage: DecisionAnchorLineageStageV1,
    ) {
        state.on_tool_finished(id, name, &output(name, root, stage));
    }

    #[test]
    fn blocks_until_a_later_root_bound_trace_and_two_source_reads_complete() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        assert!(state.blocks_mutation("write"));

        state.on_tool_dispatched(&call("refine", "codebase_memory_search_code"), 1);
        finish(
            &mut state,
            "refine",
            "codebase_memory_search_code",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
        finish(
            &mut state,
            "trace",
            "codebase_memory_trace_path",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        state.on_tool_dispatched(&call("source", "codebase_memory_get_code_snippet"), 3);
        finish(
            &mut state,
            "source",
            "codebase_memory_get_code_snippet",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        assert!(
            state.blocks_mutation("write"),
            "one source read is incomplete"
        );
        state.on_tool_dispatched(&call("test", "codebase_memory_get_code_snippet"), 4);
        finish(
            &mut state,
            "test",
            "codebase_memory_get_code_snippet",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );

        assert!(!state.blocks_mutation("write"));
    }

    #[test]
    fn unknown_malformed_mixed_root_or_unsupported_lineage_cannot_consume_a_root() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );

        let cases = [
            serde_json::json!({
                "version": 2,
                "root_binding": ROOT,
                "stage": "carry_forward",
                "target_kind": "pattern"
            }),
            serde_json::json!({
                "version": 1,
                "root_binding": "not-an-opaque-root",
                "stage": "carry_forward",
                "target_kind": "pattern"
            }),
            serde_json::json!({
                "version": 1,
                "root_binding": ROOT,
                "stage": "carry_forward",
                "target_kind": "pattern",
                "result_target_kinds": ["function_name", "function_name"]
            }),
            serde_json::json!({
                "version": 1,
                "root_binding": ROOT,
                "stage": "carry_forward",
                "target_kind": "pattern",
                "result_target_kinds": ["graph_query"]
            }),
            serde_json::json!({
                "version": 1,
                "root_binding": ROOT,
                "stage": "carry_forward",
                "target_kind": "unknown"
            }),
        ];
        for (index, lineage) in cases.into_iter().enumerate() {
            let id = format!("invalid-{index}");
            state.on_tool_dispatched(&call(&id, "codebase_memory_search_code"), index + 1);
            let (tool, kind) = correlation("codebase_memory_search_code");
            let output = ToolOutput {
                content: Vec::new(),
                details: Some(serde_json::json!({
                    SAFE_GRAPH_CORRELATION_DETAIL_KEY: GraphCorrelationV1::new(tool, kind, "request").unwrap(),
                    SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: lineage,
                })),
                is_error: false,
            };
            state.on_tool_finished(&id, "codebase_memory_search_code", &output);
            assert!(state.blocks_mutation("write"));
        }

        state.on_tool_dispatched(&call("mixed", "codebase_memory_search_code"), 5);
        finish(
            &mut state,
            "mixed",
            "codebase_memory_search_code",
            OTHER_ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        assert!(state.blocks_mutation("write"));
    }

    #[test]
    fn contract_is_canonical_and_never_serializes_raw_provider_values_or_digests() {
        let raw = "Authorization: Bearer decision-anchor-secret";
        let lineage = DecisionAnchorLineageV1::new(
            ROOT.to_string(),
            DecisionAnchorLineageStageV1::Root,
            DecisionAnchorTargetKindV1::GraphQuery,
            [
                DecisionAnchorTargetKindV1::QualifiedName,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ],
        )
        .unwrap();
        assert!(lineage.is_valid());
        assert_eq!(
            lineage.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );
        let serialized = serde_json::to_string(&lineage).unwrap();
        assert!(!serialized.contains(raw));
        assert!(!serialized.contains("sha256"));
    }
}
