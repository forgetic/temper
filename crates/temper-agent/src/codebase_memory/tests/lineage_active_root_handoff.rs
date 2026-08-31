#[test]
fn active_root_handoff_selects_the_first_admission_valid_provider_candidate() {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use temper_agent_core::{
        AgentCompletion, AgentEvent, AgentMachine, AgentRequest, ToolCallDenial,
        ToolFailureDiagnostic, SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
        SAFE_GRAPH_CORRELATION_DETAIL_KEY,
    };
    use temper_agent_io::{EngineTime, Machine};
    use temper_protocol_activity::{
        DecisionAnchorLineageV1, DecisionEvidenceKindV1, GraphExplorationClosedV1,
        GraphRecoveryActionV1, GraphRecoveryEvidenceKindV1,
        GraphRecoveryReferenceDispositionV1, MAX_GRAPH_RECOVERY_ALLOWANCE_V1,
    };
    use tongs::model::{
        AssistantMessage, ContentBlock, Message, StopReason, ToolCall, Usage, UserContent,
        UserMessage,
    };
    use tongs::tools::{ToolEffects, ToolOutput};

    fn projected_results(prefix: &str) -> serde_json::Value {
        serde_json::json!({
            "total": 13,
            "has_more": false,
            "results": (1..=13)
                .map(|rank| serde_json::json!({
                    "name": format!("{prefix}_{rank}"),
                    "qualified_name": format!("crate::{prefix}::{prefix}_{rank}"),
                    "label": "Function",
                    "file_path": "src/route.rs",
                }))
                .collect::<Vec<_>>()
        })
    }

    fn implementation_references(guidance: &str) -> Vec<String> {
        guidance
            .split([',', '.', ' ', ']'])
            .filter_map(|part| part.split_once('='))
            .filter(|(label, value)| {
                label.starts_with("implementation_candidate")
                    && value.starts_with("temper-recovery-selector:")
            })
            .map(|(_, value)| value.to_string())
            .collect()
    }

    fn source_input(reference: &str) -> serde_json::Value {
        serde_json::json!({
            "qualified_name": reference,
            "decision_evidence_kind": "implementation",
        })
    }

    fn assistant(calls: Vec<(&str, &str, serde_json::Value)>) -> AssistantMessage {
        AssistantMessage {
            content: calls
                .into_iter()
                .map(|(id, name, arguments)| {
                    ContentBlock::ToolCall(ToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        arguments,
                    })
                })
                .collect(),
            api: "test".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
        }
    }

    fn complete_llm(
        machine: &mut AgentMachine,
        message: AssistantMessage,
    ) -> Vec<AgentRequest> {
        let (operation_generation, batch_generation) = machine
            .active_generations()
            .expect("an active model operation");
        machine.on_completion(
            EngineTime::ZERO,
            AgentCompletion::LlmResponded {
                operation_generation,
                batch_generation,
                message,
            },
        )
    }

    fn complete_tool(
        machine: &mut AgentMachine,
        id: &str,
        output: ToolOutput,
        failure: Option<ToolFailureDiagnostic>,
    ) -> Vec<AgentRequest> {
        let (operation_generation, batch_generation) = machine
            .active_tool_generations(id)
            .expect("an active tool operation");
        machine.on_completion(
            EngineTime::ZERO,
            AgentCompletion::ToolFinished {
                operation_generation,
                batch_generation,
                id: id.to_string(),
                output,
                failure,
            },
        )
    }

    fn machine_output(
        correlation: &GraphCorrelationV1,
        lineage: &DecisionAnchorLineageV1,
    ) -> ToolOutput {
        ToolOutput {
            content: Vec::new(),
            details: Some(serde_json::json!({
                SAFE_GRAPH_CORRELATION_DETAIL_KEY: correlation,
                SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: lineage,
            })),
            is_error: false,
        }
    }

    fn active_handoff(requests: &[AgentRequest]) -> &str {
        requests
            .iter()
            .find_map(|request| match request {
                AgentRequest::CallLlm { messages, .. } => {
                    messages.iter().rev().find_map(|message| match message {
                        Message::User(message) => match &message.content {
                            UserContent::Text(text)
                                if text.contains("Active-root selector handoff") =>
                            {
                                Some(text.as_str())
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("the machine exposes one active-root handoff")
    }

    let workspace = tempfile::tempdir().unwrap();
    let context = crate::codebase_memory::tests::test_support::workspace_context(
        workspace.path(),
        &[("acme", "demo", "demo")],
    );
    let scope = Arc::new(
        crate::codebase_memory::scope::WorkspaceScope::from_context(&context, workspace.path())
            .unwrap(),
    );
    let registry = Arc::new(DecisionAnchorLineageRegistry::new(scope));

    let active_correlation = correlation(GraphCorrelationTargetKindV1::GraphQuery);
    let active_input = serde_json::json!({"query": "active routing implementation"});
    let active = registry
        .record_with_evidence_kind(
            &active_correlation,
            &active_input,
            Some(&structured_parts(projected_results("active"))),
            None,
        )
        .expect("valid projected active root");
    let sibling_correlation = correlation(GraphCorrelationTargetKindV1::GraphQuery);
    let sibling_input = serde_json::json!({"query": "sibling routing implementation"});
    let sibling = registry
        .record_with_evidence_kind(
            &sibling_correlation,
            &sibling_input,
            Some(&structured_parts(projected_results("sibling"))),
            None,
        )
        .expect("valid projected sibling root");

    let active_references = implementation_references(
        &registry
            .recovery_selector_guidance(&active)
            .expect("active projected candidates"),
    );
    let sibling_references = implementation_references(
        &registry
            .recovery_selector_guidance(&sibling)
            .expect("sibling projected candidates"),
    );
    assert_eq!(active_references.len(), 4);
    assert_eq!(sibling_references.len(), 4);

    let classified = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::active::active_4"}),
            Some(&structured_parts(serde_json::json!({
                "name": "active_4",
                "qualified_name": "crate::active::active_4",
                "source": "fn active_4() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .expect("rank 4 becomes the provider-classified implementation result");
    assert_eq!(classified.root_binding, active.root_binding);
    assert_eq!(
        classified.decision_evidence_kind,
        Some(DecisionEvidenceKindV1::Implementation)
    );

    registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::sibling::sibling_5"}),
            Some(&structured_parts(serde_json::json!({
                "name": "sibling_5",
                "qualified_name": "crate::sibling::sibling_5",
                "source": "fn sibling_5() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .expect("an implementation outside the sibling candidate window is classified");

    let action = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Implementation);
    assert!(
        registry
            .active_root_recovery_selector(&sibling.root_binding, action)
            .is_none(),
        "a projected root with no admission-valid retained candidate fails closed",
    );

    let effects = BTreeMap::from([
        (
            "codebase_memory_search_graph".to_string(),
            ToolEffects::read(),
        ),
        (
            "codebase_memory_get_code_snippet".to_string(),
            ToolEffects::read(),
        ),
    ]);
    let mut machine = AgentMachine::with_effects(
        vec![Message::User(UserMessage {
            content: UserContent::Text("repair routing".to_string()),
            timestamp: 0,
        })],
        10,
        effects,
    )
    .with_lineage_admission(registry.clone());
    let _ = machine.on_start(EngineTime::ZERO);

    let dispatched = complete_llm(
        &mut machine,
        assistant(vec![
            (
                "active-root",
                "codebase_memory_search_graph",
                active_input,
            ),
            (
                "sibling-root",
                "codebase_memory_search_graph",
                sibling_input,
            ),
        ]),
    );
    assert_eq!(
        dispatched
            .iter()
            .filter(|request| matches!(
                request,
                AgentRequest::RunTool {
                    denial: None,
                    rejection: None,
                    ..
                }
            ))
            .count(),
        2,
        "both projected roots are dispatched in one parallel read batch",
    );
    assert!(
        complete_tool(
            &mut machine,
            "sibling-root",
            machine_output(&sibling_correlation, &sibling),
            None,
        )
        .is_empty()
    );
    let selected = complete_tool(
        &mut machine,
        "active-root",
        machine_output(&active_correlation, &active),
        None,
    );
    let selected_handoff = active_handoff(&selected);
    assert_eq!(
        selected_handoff.matches(&active_references[3]).count(),
        1
    );
    assert!(selected_handoff.contains("codebase_memory_get_code_snippet"));
    assert!(selected_handoff.contains("selector field=qualified_name"));
    for hidden in active_references[..3].iter().chain(&sibling_references) {
        assert!(!selected_handoff.contains(hidden));
    }

    let denied = complete_llm(
        &mut machine,
        assistant(vec![(
            "active-distractor",
            "codebase_memory_get_code_snippet",
            source_input(&active_references[0]),
        )]),
    );
    let expected = GraphExplorationClosedV1::recoverable_without_actions(
        [
            GraphRecoveryEvidenceKindV1::Trace,
            GraphRecoveryEvidenceKindV1::Implementation,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ],
        MAX_GRAPH_RECOVERY_ALLOWANCE_V1,
    )
    .unwrap();
    assert!(denied.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool {
            call,
            denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
            rejection: None,
            ..
        } if call.id == "active-distractor" && details == &expected
    )));
    assert!(denied.iter().any(|request| matches!(
        request,
        AgentRequest::Emit(AgentEvent::ToolStart {
            id,
            recovery_reference_disposition: Some(
                GraphRecoveryReferenceDispositionV1::Rejected
            ),
            ..
        }) if id == "active-distractor"
    )));

    let resumed = complete_tool(
        &mut machine,
        "active-distractor",
        ToolOutput {
            content: Vec::new(),
            details: None,
            is_error: true,
        },
        Some(ToolFailureDiagnostic::graph_exploration(expected)),
    );
    let resumed_handoff = active_handoff(&resumed);
    assert_eq!(resumed_handoff.matches(&active_references[3]).count(), 1);
    for hidden in active_references[..3].iter().chain(&sibling_references) {
        assert!(!resumed_handoff.contains(hidden));
    }

    let exact_input = source_input(&active_references[3]);
    let admitted = complete_llm(
        &mut machine,
        assistant(vec![(
            "active-implementation",
            "codebase_memory_get_code_snippet",
            exact_input.clone(),
        )]),
    );
    assert!(admitted.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool {
            call,
            denial: None,
            rejection: None,
            ..
        } if call.id == "active-implementation"
    )));
    assert!(admitted.iter().any(|request| matches!(
        request,
        AgentRequest::Emit(AgentEvent::ToolStart {
            id,
            recovery_reference_disposition: Some(
                GraphRecoveryReferenceDispositionV1::Recognized
            ),
            ..
        }) if id == "active-implementation"
    )));

    let mut provider_input = exact_input;
    let expanded = registry
        .expand_recovery_selector(
            action.tool.public_name(),
            &mut provider_input,
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .unwrap()
        .expect("the admitted opaque handoff expands at provider dispatch");
    assert_eq!(provider_input["qualified_name"], "crate::active::active_4");
    let advanced = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &provider_input,
            Some(&structured_parts(serde_json::json!({
                "name": "active_4",
                "qualified_name": "crate::active::active_4",
                "source": "fn active_4() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .expect("the exact admitted handoff advances implementation evidence");
    registry
        .complete_candidate_reference(&expanded, false)
        .expect("the successful provider candidate is completed");
    assert_eq!(advanced.root_binding, active.root_binding);
    assert_eq!(
        advanced.decision_evidence_kind,
        Some(DecisionEvidenceKindV1::Implementation)
    );

    let advanced_requests = complete_tool(
        &mut machine,
        "active-implementation",
        machine_output(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &advanced,
        ),
        None,
    );
    assert!(advanced_requests.iter().any(|request| match request {
        AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| {
            matches!(message, Message::User(message)
                if matches!(&message.content, UserContent::Text(text)
                    if text.contains("accepted evidence=[implementation]")))
        }),
        _ => false,
    }));
}
