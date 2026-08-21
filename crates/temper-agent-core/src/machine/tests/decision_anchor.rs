//! Deterministic decision-anchor policy regressions.

mod tests {
    use super::super::super::decision_anchor::*;
    use crate::machine::tests::common::{
        assistant_tool_calls, calls_llm, complete, final_stop, llm_responded, run_tools,
        tool_finished, user,
    };
    use crate::machine::{
        AgentMachine, AgentRequest, SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
        SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY, ToolCallDenial,
        ToolStartPresentation,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use temper_agent_io::{EngineTime, Machine};
    use temper_protocol_activity::{
        DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
        DecisionEvidenceKindV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1,
        GraphCorrelationV1, GraphExplorationClosedReasonV1, GraphExplorationClosedV1,
        GraphRecoveryEvidenceKindV1,
    };
    use tongs::{
        model::{ContentBlock, Message, ToolCall, UserContent},
        tools::{ToolEffects, ToolOutput},
    };

    const ROOT: &str = "00000000-0000-4000-8000-000000000001";
    const OTHER_ROOT: &str = "00000000-0000-4000-8000-000000000002";

    mod root_forest {
        include!("decision_anchor_root_forest.rs");
    }

    mod convergence {
        include!("decision_anchor_convergence.rs");
    }

    mod recovery {
        include!("decision_anchor_recovery.rs");
    }

    mod routing_recovery {
        include!("decision_anchor_routing_recovery.rs");
    }

    mod evidence {
        include!("decision_anchor_evidence.rs");
    }

    fn all_missing() -> [GraphRecoveryEvidenceKindV1; 4] {
        [
            GraphRecoveryEvidenceKindV1::Trace,
            GraphRecoveryEvidenceKindV1::Implementation,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ]
    }

    fn completed_graph_denial() -> Option<ToolCallDenial> {
        Some(ToolCallDenial::GraphExplorationClosed(Some(
            GraphExplorationClosedV1::completed(),
        )))
    }

    fn recovery_graph_denial(
        missing: impl IntoIterator<Item = GraphRecoveryEvidenceKindV1>,
        remaining: u8,
    ) -> Option<ToolCallDenial> {
        Some(ToolCallDenial::GraphExplorationClosed(
            GraphExplorationClosedV1::recoverable(missing, remaining),
        ))
    }

    fn exhausted_graph_denial(
        missing: impl IntoIterator<Item = GraphRecoveryEvidenceKindV1>,
    ) -> Option<ToolCallDenial> {
        Some(ToolCallDenial::GraphExplorationClosed(
            GraphExplorationClosedV1::exhausted(missing),
        ))
    }

    fn legacy_graph_denial() -> Option<ToolCallDenial> {
        Some(ToolCallDenial::GraphExplorationClosed(None))
    }

    fn effects() -> BTreeMap<String, ToolEffects> {
        [
            ("codebase_memory_search_graph", ToolEffects::read()),
            ("codebase_memory_search_code", ToolEffects::read()),
            ("codebase_memory_trace_path", ToolEffects::read()),
            ("codebase_memory_get_code_snippet", ToolEffects::read()),
            ("codebase_memory_get_architecture", ToolEffects::read()),
            ("read", ToolEffects::read()),
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

    fn source_call(id: &str, kind: DecisionEvidenceKindV1) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "codebase_memory_get_code_snippet".to_string(),
            arguments: serde_json::json!({
                "qualified_name": "not used by the policy",
                "decision_evidence_kind": kind,
            }),
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

    fn output_with_evidence(
        root: &str,
        stage: DecisionAnchorLineageStageV1,
        evidence_kind: DecisionEvidenceKindV1,
    ) -> ToolOutput {
        let (tool, kind) = correlation("codebase_memory_get_code_snippet");
        let lineage = DecisionAnchorLineageV1::new_with_decision_evidence_kind(
            root.to_string(),
            stage,
            DecisionAnchorTargetKindV1::from_graph_correlation(kind),
            [
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ],
            evidence_kind,
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

    fn failure_output(category: &str) -> ToolOutput {
        ToolOutput {
            content: Vec::new(),
            details: Some(serde_json::json!({
                SAFE_TOOL_FAILURE_DETAIL_KEY: {
                    "source": "codebase_memory",
                    "category": category,
                },
            })),
            is_error: true,
        }
    }

    fn plain_success() -> ToolOutput {
        ToolOutput {
            content: Vec::new(),
            details: None,
            is_error: false,
        }
    }

    fn message_count(requests: &[AgentRequest], expected: &str) -> usize {
        requests
            .iter()
            .filter_map(|request| match request {
                AgentRequest::CallLlm { messages, .. } => Some(messages),
                _ => None,
            })
            .flatten()
            .filter(|message| {
                matches!(
                    message,
                    Message::User(user)
                        if matches!(&user.content, UserContent::Text(text) if text == expected)
                )
            })
            .count()
    }

    fn message_containing(requests: &[AgentRequest], expected: &str) -> bool {
        requests
            .iter()
            .filter_map(|request| match request {
                AgentRequest::CallLlm { messages, .. } => Some(messages),
                _ => None,
            })
            .flatten()
            .any(|message| {
                matches!(
                    message,
                    Message::User(user)
                        if matches!(&user.content, UserContent::Text(text) if text.contains(expected))
                )
            })
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

    fn finish_with_evidence(
        state: &mut DecisionAnchorState,
        id: &str,
        root: &str,
        kind: DecisionEvidenceKindV1,
    ) -> DecisionAnchorTransition {
        state.on_tool_finished(
            id,
            "codebase_memory_get_code_snippet",
            &output_with_evidence(root, DecisionAnchorLineageStageV1::CarryForward, kind),
        )
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
    fn parallel_trace_and_source_batch_uses_dispatch_order_after_reverse_completion() {
        let mut machine = AgentMachine::with_effects(vec![user("repair")], 10, effects())
            .with_arg_preview(Arc::new(|_, arguments| ToolStartPresentation {
                arg_preview: Some(arguments.to_string()),
                diagnostic_arguments: None,
            }));
        let _ = machine.on_start(EngineTime::ZERO);

        let root_requests = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(
                "root",
                "codebase_memory_search_graph",
            )])),
        );
        assert_eq!(run_tools(&root_requests), ["root"]);
        let _ = complete(
            &mut machine,
            tool_finished(
                "root",
                output(
                    "codebase_memory_search_graph",
                    ROOT,
                    DecisionAnchorLineageStageV1::Root,
                ),
            ),
        );

        let refine_requests = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(
                "refine",
                "codebase_memory_search_code",
            )])),
        );
        assert_eq!(run_tools(&refine_requests), ["refine"]);
        let _ = complete(
            &mut machine,
            tool_finished(
                "refine",
                output(
                    "codebase_memory_search_code",
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                ),
            ),
        );

        let batch_requests = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[
                ("trace", "codebase_memory_trace_path"),
                ("implementation", "codebase_memory_get_code_snippet"),
                ("caller", "codebase_memory_get_code_snippet"),
                ("behavior", "codebase_memory_get_code_snippet"),
            ])),
        );
        assert_eq!(
            run_tools(&batch_requests),
            ["trace", "implementation", "caller", "behavior"],
            "the independent reads retain their parallel dispatch batch"
        );

        // Reverse transport completion still evaluates the original trace-first dispatch order.
        assert!(
            complete(
                &mut machine,
                tool_finished(
                    "implementation",
                    output_with_evidence(
                        ROOT,
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionEvidenceKindV1::Implementation,
                    ),
                ),
            )
            .is_empty()
        );
        assert!(
            complete(
                &mut machine,
                tool_finished(
                    "trace",
                    output(
                        "codebase_memory_trace_path",
                        ROOT,
                        DecisionAnchorLineageStageV1::CarryForward,
                    ),
                ),
            )
            .is_empty()
        );
        assert!(
            complete(
                &mut machine,
                tool_finished(
                    "caller",
                    output_with_evidence(
                        ROOT,
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionEvidenceKindV1::Caller,
                    ),
                ),
            )
            .is_empty()
        );
        let final_source = complete(
            &mut machine,
            tool_finished(
                "behavior",
                output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionEvidenceKindV1::FocusedTest,
                ),
            ),
        );
        assert_eq!(
            calls_llm(&final_source),
            1,
            "the drained batch advances once after every completion"
        );
        assert_eq!(
            message_count(&final_source, DECISION_ANCHOR_CONVERGENCE_MESSAGE),
            1,
            "completion queues one fixed convergence instruction"
        );

        let denied_graph = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(
                "post-completion-graph",
                "codebase_memory_search_graph",
            )])),
        );
        assert!(denied_graph.iter().any(|request| {
            matches!(
                request,
                AgentRequest::RunTool {
                    call,
                    denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
                    ..
                } if call.id == "post-completion-graph"
                    && details.reason == GraphExplorationClosedReasonV1::Completed
            )
        }));
        assert!(denied_graph.iter().any(|request| {
            matches!(
                request,
                AgentRequest::Emit(crate::machine::AgentEvent::ToolStart {
                    id,
                    arg_preview: None,
                    ..
                }) if id == "post-completion-graph"
            )
        }));
        let after_denial = complete(
            &mut machine,
            tool_finished(
                "post-completion-graph",
                ToolOutput {
                    content: vec![tongs::model::ContentBlock::Text(
                        tongs::model::TextContent {
                            text: CODEBASE_MEMORY_EXPLORATION_CLOSED_MESSAGE.to_string(),
                            text_signature: None,
                        },
                    )],
                    details: None,
                    is_error: true,
                },
            ),
        );
        assert_eq!(calls_llm(&after_denial), 1);

        let mutation_requests = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[("mutation", "write")])),
        );
        assert!(mutation_requests.iter().any(|request| {
            matches!(
                request,
                AgentRequest::RunTool {
                    call,
                    denial: None,
                    ..
                } if call.id == "mutation"
            )
        }));
    }

    #[test]
    fn expected_unavailable_source_releases_conventional_fallback() {
        let mut fallback = DecisionAnchorState::from_effects(&effects()).unwrap();
        fallback.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut fallback,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        fallback.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
        finish(
            &mut fallback,
            "trace",
            "codebase_memory_trace_path",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        fallback.on_tool_dispatched(
            &source_call("source", DecisionEvidenceKindV1::Implementation),
            2,
        );
        assert_eq!(
            fallback.on_tool_finished(
                "source",
                "codebase_memory_get_code_snippet",
                &failure_output("transport"),
            ),
            DecisionAnchorTransition::Unchanged
        );
        assert!(
            !fallback.blocks_mutation("write"),
            "the unavailable expected source read must permit conventional fallback"
        );
        assert_eq!(
            fallback.on_tool_dispatched(
                &source_call("retry", DecisionEvidenceKindV1::Implementation),
                3,
            ),
            legacy_graph_denial(),
            "fallback must not immediately reopen graph exploration"
        );

        let mut unrelated = DecisionAnchorState::from_effects(&effects()).unwrap();
        unrelated.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut unrelated,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        unrelated.on_tool_dispatched(&call("unrelated", "codebase_memory_search_graph"), 1);
        unrelated.on_tool_finished(
            "unrelated",
            "codebase_memory_search_graph",
            &failure_output("transport"),
        );
        assert!(
            unrelated.blocks_mutation("write"),
            "an unrelated provider outage cannot bypass an active anchor"
        );
    }

    #[test]
    fn exploration_closure_cannot_release_incomplete_anchor_authority() {
        let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
        state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            "root",
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
        );
        state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
        finish(
            &mut state,
            "trace",
            "codebase_memory_trace_path",
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
        );
        state.on_tool_dispatched(
            &source_call("closed-source", DecisionEvidenceKindV1::Implementation),
            2,
        );

        assert_eq!(
            state.on_tool_finished(
                "closed-source",
                "codebase_memory_get_code_snippet",
                &failure_output("graph_lifecycle_denial"),
            ),
            DecisionAnchorTransition::RecoveryNeeded
        );
        assert!(
            state.blocks_mutation("write"),
            "closure cannot manufacture the missing source/test evidence"
        );
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
}
