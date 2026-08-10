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

    fn output_with_kinds(
        name: &str,
        root: &str,
        stage: DecisionAnchorLineageStageV1,
        result_target_kinds: &[DecisionAnchorTargetKindV1],
    ) -> ToolOutput {
        let (tool, kind) = correlation(name);
        let lineage = DecisionAnchorLineageV1::new(
            root.to_string(),
            stage,
            DecisionAnchorTargetKindV1::from_graph_correlation(kind),
            result_target_kinds.iter().copied(),
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

    fn output(name: &str, root: &str, stage: DecisionAnchorLineageStageV1) -> ToolOutput {
        output_with_kinds(
            name,
            root,
            stage,
            &[
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ],
        )
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

    fn finish_with_kinds(
        state: &mut DecisionAnchorState,
        id: &str,
        name: &str,
        root: &str,
        stage: DecisionAnchorLineageStageV1,
        result_target_kinds: &[DecisionAnchorTargetKindV1],
    ) -> DecisionAnchorTransition {
        state.on_tool_finished(
            id,
            name,
            &output_with_kinds(name, root, stage, result_target_kinds),
        )
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
    #[test]
    fn rejects_producer_turn_type_incompatible_and_cross_root_substitutions() {
        let mut producer_turn = DecisionAnchorState::from_effects(&effects()).unwrap();
        producer_turn.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut producer_turn,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        producer_turn.on_tool_dispatched(&call("same-turn", "codebase_memory_trace_path"), 0);
        assert_eq!(
            finish_with_kinds(
                &mut producer_turn,
                "same-turn",
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                &[DecisionAnchorTargetKindV1::QualifiedName],
            ),
            DecisionAnchorTransition::Unchanged
        );
        assert!(producer_turn.blocks_mutation("write"));

        let mut incompatible = DecisionAnchorState::from_effects(&effects()).unwrap();
        incompatible.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        assert_eq!(
            finish_with_kinds(
                &mut incompatible,
                "root",
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::Root,
                &[DecisionAnchorTargetKindV1::FunctionName],
            ),
            DecisionAnchorTransition::Unchanged
        );
        incompatible.on_tool_dispatched(&call("pattern", "codebase_memory_search_code"), 1);
        assert_eq!(
            finish_with_kinds(
                &mut incompatible,
                "pattern",
                "codebase_memory_search_code",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                &[DecisionAnchorTargetKindV1::FunctionName],
            ),
            DecisionAnchorTransition::RecoveryNeeded
        );
        assert!(incompatible.blocks_mutation("write"));

        let mut cross_root = DecisionAnchorState::from_effects(&effects()).unwrap();
        cross_root.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut cross_root,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        cross_root.on_tool_dispatched(&call("other", "codebase_memory_trace_path"), 1);
        finish(
            &mut cross_root,
            "other",
            "codebase_memory_trace_path",
            OTHER_ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        assert!(cross_root.blocks_mutation("write"));
    }

    #[test]
    fn unconsumable_roots_have_two_recovery_attempts_then_stay_blocked() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        for (turn, id, expected) in [
            (0, "root", DecisionAnchorTransition::RecoveryNeeded),
            (1, "recovery-one", DecisionAnchorTransition::RecoveryNeeded),
            (
                2,
                "recovery-two",
                DecisionAnchorTransition::RecoveryExhausted,
            ),
        ] {
            state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), turn);
            assert_eq!(
                finish_with_kinds(
                    &mut state,
                    id,
                    "codebase_memory_search_graph",
                    ROOT,
                    DecisionAnchorLineageStageV1::Root,
                    &[],
                ),
                expected
            );
            assert!(state.blocks_mutation("write"));
        }
    }

    #[test]
    fn failed_or_malformed_graph_results_create_no_anchor_or_mutation_block() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("failed", "codebase_memory_search_graph"), 0);
        let failed = ToolOutput {
            content: Vec::new(),
            details: None,
            is_error: true,
        };
        assert_eq!(
            state.on_tool_finished("failed", "codebase_memory_search_graph", &failed),
            DecisionAnchorTransition::Unchanged
        );
        assert!(!state.blocks_mutation("write"));

        state.on_tool_dispatched(&call("malformed", "codebase_memory_search_graph"), 1);
        let malformed = ToolOutput {
            content: Vec::new(),
            details: Some(serde_json::json!({
                SAFE_GRAPH_CORRELATION_DETAIL_KEY: {"version": 99},
            })),
            is_error: false,
        };
        assert_eq!(
            state.on_tool_finished("malformed", "codebase_memory_search_graph", &malformed),
            DecisionAnchorTransition::Unchanged
        );
        assert!(!state.blocks_mutation("write"));
    }
}
