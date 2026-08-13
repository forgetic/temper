//! Typed provider-result lineage regressions.
//!
//! This test module is intentionally larger than the normal Rust-file target:
//! its data-driven cases cover every approved transformed provider shape and
//! every rejection boundary without exposing provider values outside the
//! wrapper-local lineage registry.

use super::super::lineage::*;
use crate::mcp::McpToolResultPart;
use serde_json::Value;
use temper_agent_core::{DecisionAnchorLineageStageV1, DecisionAnchorTargetKindV1};
use temper_protocol_activity::{
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
};

const MAX_RESULT_TARGETS: usize = 64;

mod tests {
    use super::*;

    fn text_parts(value: Value) -> Vec<McpToolResultPart> {
        vec![McpToolResultPart::Content(serde_json::json!({
            "type": "text",
            "text": value.to_string(),
        }))]
    }

    fn structured_parts(value: Value) -> Vec<McpToolResultPart> {
        vec![McpToolResultPart::StructuredContent(value)]
    }

    fn correlation(kind: GraphCorrelationTargetKindV1) -> GraphCorrelationV1 {
        let tool = match kind {
            GraphCorrelationTargetKindV1::GraphQuery
            | GraphCorrelationTargetKindV1::NamePattern
            | GraphCorrelationTargetKindV1::QualifiedNamePattern => {
                GraphCorrelationToolV1::SearchGraph
            }
            GraphCorrelationTargetKindV1::Pattern => GraphCorrelationToolV1::SearchCode,
            GraphCorrelationTargetKindV1::FunctionName => GraphCorrelationToolV1::TracePath,
            GraphCorrelationTargetKindV1::QualifiedName => GraphCorrelationToolV1::GetCodeSnippet,
        };
        GraphCorrelationV1::new(tool, kind, "declared target").unwrap()
    }

    #[test]
    fn qualified_symbols_carry_to_equivalent_function_and_qualified_selectors() {
        let mut lineages = DecisionAnchorLineages::default();
        let root_parts = text_parts(serde_json::json!([
            {"qualifiedName":"crate::engine::run","path":"private/src/lib.rs"}
        ]));
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&root_parts),
            )
            .unwrap();
        assert_eq!(root.stage, DecisionAnchorLineageStageV1::Root);
        assert_eq!(
            root.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );

        let function_parts = text_parts(serde_json::json!({"qualified_name":"crate::engine::run"}));
        let function = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "run"}),
                Some(&function_parts),
            )
            .unwrap();
        assert_eq!(function.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(function.root_binding, root.root_binding);

        let qualified_parts = text_parts(serde_json::json!({"function_name":"run"}));
        let qualified = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "crate::engine::run"}),
                Some(&qualified_parts),
            )
            .unwrap();
        assert_eq!(qualified.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(qualified.root_binding, root.root_binding);
    }

    #[test]
    fn each_selector_uses_its_own_target_kind_when_matching_provider_values() {
        let mut lineages = DecisionAnchorLineages::default();
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&structured_parts(
                    serde_json::json!({"qualified_name":"crate::engine::run"}),
                )),
            )
            .unwrap();

        let refinement = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::Pattern),
                &serde_json::json!({"pattern": "run"}),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(refinement.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(refinement.root_binding, root.root_binding);

        let trace = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "run"}),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(trace.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(trace.root_binding, root.root_binding);

        let source = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "run"}),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(source.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(source.root_binding, root.root_binding);
    }

    #[test]
    fn dashed_provider_namespace_components_remain_closed_qualified_identities() {
        let mut lineages = DecisionAnchorLineages::default();
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": "temper-v1-hash.src.temper-agent-core.machine.DecisionAnchorState"
                }))),
            )
            .unwrap();
        assert!(!root.result_target_kinds.is_empty());

        let carried = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "DecisionAnchorState"}),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(carried.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(carried.root_binding, root.root_binding);
    }

    #[test]
    fn provider_package_qualified_names_carry_across_all_selectors() {
        let mut lineages = DecisionAnchorLineages::default();
        let root_parts = text_parts(serde_json::json!({
            "results": [{
                "qualified_name": "delivery-router.route.worker_slot",
                "name": "worker_slot"
            }]
        }));
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&root_parts),
            )
            .unwrap();
        assert_eq!(
            root.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );

        let trace_parts = text_parts(serde_json::json!({
            "function": {
                "qualified_name": "delivery-router.route.worker_slot",
                "name": "worker_slot"
            },
            "callers": [{
                "qualified_name": "delivery-router.model.DeliveryAttempt",
                "name": "DeliveryAttempt"
            }]
        }));
        let trace = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "worker_slot"}),
                Some(&trace_parts),
            )
            .unwrap();
        assert_eq!(trace.stage, DecisionAnchorLineageStageV1::CarryForward);

        let source_parts = text_parts(serde_json::json!({}));
        let source = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({
                    "qualified_name": "delivery-router.model.DeliveryAttempt"
                }),
                Some(&source_parts),
            )
            .unwrap();
        assert_eq!(source.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(source.root_binding, root.root_binding);

        let short_source = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "DeliveryAttempt"}),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(
            short_source.stage,
            DecisionAnchorLineageStageV1::CarryForward,
            "a direct provider name remains valid for a source selector"
        );
        assert_eq!(short_source.root_binding, root.root_binding);
    }

    #[test]
    fn dotted_qualified_symbol_carries_across_provider_selectors() {
        let mut lineages = DecisionAnchorLineages::default();
        let root_parts = text_parts(serde_json::json!({"qualified_name":"crate.engine.run"}));
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&root_parts),
            )
            .unwrap();
        assert_eq!(
            root.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );

        let function_parts = text_parts(serde_json::json!({"qualified_name":"crate.engine.run"}));
        let function = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "crate.engine.run"}),
                Some(&function_parts),
            )
            .unwrap();
        assert_eq!(function.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(function.root_binding, root.root_binding);

        let source_parts = text_parts(serde_json::json!({}));
        let source = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "crate.engine.run"}),
                Some(&source_parts),
            )
            .unwrap();
        assert_eq!(source.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(source.root_binding, root.root_binding);
    }

    #[test]
    fn short_symbol_in_a_qualified_name_field_carries_to_a_pattern_selector() {
        let mut lineages = DecisionAnchorLineages::default();
        let root_parts = text_parts(serde_json::json!({"qualified_name":"worker_slot"}));
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&root_parts),
            )
            .unwrap();
        assert_eq!(root.stage, DecisionAnchorLineageStageV1::Root);
        assert_eq!(
            root.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );

        let refinement_parts = text_parts(serde_json::json!({"symbol":"worker_slot"}));
        let refinement = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::Pattern),
                &serde_json::json!({"pattern": "worker_slot"}),
                Some(&refinement_parts),
            )
            .unwrap();
        assert_eq!(refinement.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(refinement.root_binding, root.root_binding);

        let trace_parts = text_parts(serde_json::json!({"callers":["DeliveryAttempt"]}));
        let trace = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "worker_slot"}),
                Some(&trace_parts),
            )
            .unwrap();
        assert_eq!(trace.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(trace.root_binding, root.root_binding);

        let source_parts = text_parts(serde_json::json!({}));
        let source = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "DeliveryAttempt"}),
                Some(&source_parts),
            )
            .unwrap();
        assert_eq!(source.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(source.root_binding, root.root_binding);
    }

    #[test]
    fn approved_structured_result_parts_carry_nested_symbols_callers_sources_and_metadata() {
        let mut lineages = DecisionAnchorLineages::default();
        let root_parts = structured_parts(serde_json::json!({
            "results": [
                {"results": [{"symbol": "run"}]},
                {"callers": [{"qualifiedName": "crate::engine::caller"}]},
                {"related_source_references": [{"qualified_name": "crate::engine::source"}]},
                {"source_metadata": {
                    "next_target": {"qualifiedName": "crate::engine::behavior"},
                    "source": "PRIVATE-SOURCE"
                }}
            ]
        }));
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&root_parts),
            )
            .unwrap();
        assert_eq!(root.stage, DecisionAnchorLineageStageV1::Root);
        assert_eq!(
            root.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );

        for (kind, input) in [
            (
                GraphCorrelationTargetKindV1::FunctionName,
                serde_json::json!({"function_name": "run"}),
            ),
            (
                GraphCorrelationTargetKindV1::QualifiedName,
                serde_json::json!({"qualified_name": "crate::engine::caller"}),
            ),
            (
                GraphCorrelationTargetKindV1::QualifiedName,
                serde_json::json!({"qualified_name": "crate::engine::source"}),
            ),
            (
                GraphCorrelationTargetKindV1::QualifiedName,
                serde_json::json!({"qualified_name": "crate::engine::behavior"}),
            ),
        ] {
            let no_candidates = structured_parts(serde_json::json!({}));
            let carried = lineages
                .record(&correlation(kind), &input, Some(&no_candidates))
                .unwrap();
            assert_eq!(carried.stage, DecisionAnchorLineageStageV1::CarryForward);
            assert_eq!(carried.root_binding, root.root_binding);
        }

        let rendered = serde_json::to_string(&root).unwrap();
        for private in ["crate::engine", "PRIVATE-SOURCE"] {
            assert!(!rendered.contains(private));
        }
    }

    #[test]
    fn malformed_or_oversized_provider_records_do_not_create_carry_forwards() {
        let mut cases = vec![
            serde_json::json!({"qualified_name": ["not a string"]}),
            serde_json::json!({"name":"unbound_display_name"}),
            serde_json::json!({"unknown_target":"crate::engine::run"}),
            serde_json::json!({"callers": "not a list"}),
        ];
        cases.push(serde_json::json!({
            "results": (0..=MAX_RESULT_TARGETS)
                .map(|index| serde_json::json!({
                    "qualified_name": format!("crate::engine::run_{index}"),
                }))
                .collect::<Vec<_>>(),
        }));
        for result in cases {
            let mut lineages = DecisionAnchorLineages::default();
            let root_parts = text_parts(result);
            let root = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                    &serde_json::json!({"query": "start"}),
                    Some(&root_parts),
                )
                .unwrap();
            assert!(root.result_target_kinds.is_empty());
            let no_candidates = structured_parts(serde_json::json!({}));
            let later = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::FunctionName),
                    &serde_json::json!({"function_name": "run"}),
                    Some(&no_candidates),
                )
                .unwrap();
            assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
            assert_ne!(later.root_binding, root.root_binding);
        }
    }

    #[test]
    fn repeated_approved_representations_of_one_provider_identity_remain_consumable() {
        let mut lineages = DecisionAnchorLineages::default();
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&structured_parts(serde_json::json!({
                    "results": [
                        {
                            "qualified_name": "crate::engine::run",
                            "name": "run",
                            "symbols": ["crate::engine::run"],
                        }
                    ]
                }))),
            )
            .unwrap();
        assert!(!root.result_target_kinds.is_empty());

        let carried = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "run"}),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(carried.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(carried.root_binding, root.root_binding);
    }

    #[test]
    fn equivalent_text_and_structured_mirrors_create_one_typed_lineage() {
        let value = serde_json::json!({
            "results": [{"qualified_name": "crate.route.worker_slot"}]
        });
        let parts = vec![
            McpToolResultPart::Content(serde_json::json!({
                "type": "text",
                "text": value.to_string(),
            })),
            McpToolResultPart::StructuredContent(value),
        ];
        let mut lineages = DecisionAnchorLineages::default();
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&parts),
            )
            .unwrap();
        assert!(!root.result_target_kinds.is_empty());

        let later = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::Pattern),
                &serde_json::json!({"pattern": "worker_slot"}),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(later.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(later.root_binding, root.root_binding);
    }

    #[test]
    fn production_trace_shape_retains_caller_candidates() {
        let value = serde_json::json!({
            "function": "worker_slot",
            "direction": "both",
            "mode": "calls",
            "callees": [{
                "name": "affinity_topic",
                "qualified_name": "temper-v1-hash.src.model.DeliveryAttempt.affinity_topic",
                "hop": 1
            }],
            "callers": [{
                "name": "worker_for",
                "qualified_name": "temper-v1-hash.src.delivery.DeliveryRouter.worker_for",
                "hop": 1
            }]
        });
        let parts = vec![
            McpToolResultPart::Content(serde_json::json!({
                "type": "text",
                "text": value.to_string(),
            })),
            McpToolResultPart::StructuredContent(value),
        ];
        let mut lineages = DecisionAnchorLineages::default();
        let trace = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "worker_slot"}),
                Some(&parts),
            )
            .unwrap();
        assert!(!trace.result_target_kinds.is_empty());
    }

    #[test]
    fn production_source_shape_treats_numeric_caller_count_as_metadata() {
        let parts = structured_parts(serde_json::json!({
            "name": "DeliveryAttempt",
            "qualified_name": "temper-v1-hash.src.model.DeliveryAttempt",
            "label": "Struct",
            "file_path": "/private/workspace/src/model.rs",
            "source": "(source not available)",
            "callers": 0,
            "callees": 0
        }));
        let mut lineages = DecisionAnchorLineages::default();
        let source = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "DeliveryAttempt"}),
                Some(&parts),
            )
            .unwrap();
        assert!(!source.result_target_kinds.is_empty());
    }

    #[test]
    fn malformed_or_nontext_result_parts_cannot_create_carry_forwards() {
        let cases = [
            McpToolResultPart::Content(serde_json::json!({
                "text": r#"{\"symbol\":\"run\"}"#,
            })),
            McpToolResultPart::Content(serde_json::json!({
                "type": "text",
                "text": {"symbol": "run"},
            })),
            McpToolResultPart::Content(serde_json::json!({
                "type": "resource",
                "text": r#"{\"symbol\":\"run\"}"#,
            })),
            McpToolResultPart::StructuredContent(serde_json::json!([{"symbol": "run"}])),
        ];

        for part in cases {
            let mut lineages = DecisionAnchorLineages::default();
            let root_parts = vec![part];
            let root = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                    &serde_json::json!({"query": "start"}),
                    Some(&root_parts),
                )
                .unwrap();
            assert!(root.result_target_kinds.is_empty());

            let no_candidates = structured_parts(serde_json::json!({}));
            let later = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::FunctionName),
                    &serde_json::json!({"function_name": "run"}),
                    Some(&no_candidates),
                )
                .unwrap();
            assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
        }
    }

    #[test]
    fn exact_provider_qualified_identity_emits_portable_canonical_digests() {
        let mut lineages = DecisionAnchorLineages::default();
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": "crate.route.DeliveryRouter.worker_for"
                }))),
            )
            .unwrap();
        let root_binding = root.root_binding.clone();
        let carried = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({
                    "qualified_name": "crate.route.DeliveryRouter.worker_for"
                }),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(carried.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(carried.root_binding, root_binding);
        assert!(
            carried.canonical_target_digests.contains(
                &GraphCorrelationV1::target_digest("DeliveryRouter::worker_for").unwrap()
            )
        );
        let rendered = serde_json::to_string(&carried).unwrap();
        assert!(!rendered.contains("crate.route.DeliveryRouter.worker_for"));
    }

    #[test]
    fn ambiguous_qualified_identity_cannot_emit_canonical_equivalence() {
        let mut lineages = DecisionAnchorLineages::default();
        let first = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "first"}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": "crate.route.DeliveryRouter.worker_for"
                }))),
            )
            .unwrap();
        let first_root = first.root_binding.clone();
        let second = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "second"}),
                Some(&structured_parts(serde_json::json!({
                    "qualified_name": "crate.route.DeliveryRouter.worker_for"
                }))),
            )
            .unwrap();
        assert_ne!(first_root, second.root_binding);
        let later = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({
                    "qualified_name": "crate.route.DeliveryRouter.worker_for"
                }),
                Some(&structured_parts(serde_json::json!({}))),
            )
            .unwrap();
        assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
        assert_ne!(later.root_binding, first_root);
    }

    #[test]
    fn duplicate_cross_root_candidates_become_ineligible() {
        let mut lineages = DecisionAnchorLineages::default();
        let first_parts = structured_parts(serde_json::json!({"symbol": "shared"}));
        let first = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "first"}),
                Some(&first_parts),
            )
            .unwrap();
        let second_parts = structured_parts(serde_json::json!({"symbol": "shared"}));
        let second = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "second"}),
                Some(&second_parts),
            )
            .unwrap();
        assert_ne!(first.root_binding, second.root_binding);

        let no_candidates = structured_parts(serde_json::json!({}));
        let later = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "shared"}),
                Some(&no_candidates),
            )
            .unwrap();
        assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
        assert_ne!(later.root_binding, first.root_binding);
        assert_ne!(later.root_binding, second.root_binding);
    }
}
