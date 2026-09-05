fn run_active_root_handoff(reverse_completion: bool, exhaust_active_candidate: bool) {
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

    const OPAQUE_REFERENCE_PREFIX: &str = "temper-recovery-selector:";

    fn projected_results(prefix: &str) -> serde_json::Value {
        serde_json::json!({
            "total": 13,
            "has_more": false,
            "results": (1..=13)
                .map(|rank| {
                    if rank <= 2 {
                        serde_json::json!({
                            "label": "Module",
                            "file_path": format!("src/{prefix}_{rank}.rs"),
                        })
                    } else {
                        let name = if (prefix == "active" && rank == 6)
                            || (prefix == "sibling" && rank == 3)
                        {
                            "worker_slot".to_string()
                        } else {
                            format!("{prefix}_{rank}")
                        };
                        let qualified_name = if name == "worker_slot" {
                            name.clone()
                        } else {
                            format!("crate::{prefix}::{name}")
                        };
                        serde_json::json!({
                            "name": name,
                            "qualified_name": qualified_name,
                            "label": "Function",
                            "file_path": "src/route.rs",
                        })
                    }
                })
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

    fn handoff_reference(handoff: &str) -> &str {
        handoff
            .split_once("\"qualified_name\":\"")
            .and_then(|(_, value)| value.split_once('\"'))
            .map(|(reference, _)| reference)
            .filter(|reference| reference.starts_with(OPAQUE_REFERENCE_PREFIX))
            .expect("the handoff contains one opaque qualified-name reference")
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
    let active_references = implementation_references(
        &registry
            .recovery_selector_guidance(&active)
            .expect("active projected candidates"),
    );
    assert_eq!(active_references.len(), 4);

    let classified = registry
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "worker_slot"}),
            Some(&structured_parts(serde_json::json!({
                "name": "worker_slot",
                "qualified_name": "worker_slot",
                "source": "fn worker_slot() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .expect("provider rank 6 becomes the classified implementation result");
    assert_eq!(classified.root_binding, active.root_binding);
    assert_eq!(
        classified.decision_evidence_kind,
        Some(DecisionEvidenceKindV1::Implementation)
    );

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
    let sibling_references = implementation_references(
        &registry
            .recovery_selector_guidance(&sibling)
            .expect("sibling projected candidates"),
    );
    assert_eq!(sibling_references.len(), 4);

    let action = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Implementation);
    assert!(
        !registry
            .active_root_recovery_selectors(&sibling.root_binding, action)
            .is_empty(),
        "a sibling candidate remains available but cannot authorize the active root",
    );
    let selected_candidates =
        registry.active_root_recovery_selectors(&active.root_binding, action);
    let selected_candidate = selected_candidates
        .first()
        .expect("the active root retains its rank-6 reference after sibling registration")
        .as_public_selector()
        .to_string();
    assert_eq!(selected_candidate, active_references[3]);
    if exhaust_active_candidate {
        let mut provider_input = source_input(&selected_candidate);
        let expanded = registry
            .expand_recovery_selector(
                action.tool.public_name(),
                &mut provider_input,
                Some(DecisionEvidenceKindV1::Implementation),
            )
            .unwrap()
            .expect("the active candidate can be closed for the no-valid case");
        registry
            .complete_candidate_reference(&expanded, true)
            .expect("the selected active candidate is closed");
        assert!(
            registry
                .active_root_recovery_selectors(&active.root_binding, action)
                .is_empty(),
            "incapable active alternates remain fail closed",
        );
    }

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
    let selected = if reverse_completion {
        assert!(
            complete_tool(
                &mut machine,
                "sibling-root",
                machine_output(&sibling_correlation, &sibling),
                None,
            )
            .is_empty()
        );
        complete_tool(
            &mut machine,
            "active-root",
            machine_output(&active_correlation, &active),
            None,
        )
    } else {
        assert!(
            complete_tool(
                &mut machine,
                "active-root",
                machine_output(&active_correlation, &active),
                None,
            )
            .is_empty()
        );
        complete_tool(
            &mut machine,
            "sibling-root",
            machine_output(&sibling_correlation, &sibling),
            None,
        )
    };
    if exhaust_active_candidate {
        assert!(selected.iter().all(|request| match request {
            AgentRequest::CallLlm { messages, .. } => messages.iter().all(|message| {
                !matches!(message, Message::User(message)
                    if matches!(&message.content, UserContent::Text(text)
                        if text.contains("Active-root selector handoff")))
            }),
            _ => true,
        }));
        return;
    }
    let selected_handoff = active_handoff(&selected);
    let selected_reference = handoff_reference(selected_handoff).to_string();
    assert_eq!(selected_reference, selected_candidate);
    assert_eq!(selected_reference, active_references[3]);
    assert_eq!(selected_handoff.matches(&selected_reference).count(), 1);
    assert_eq!(
        selected_handoff.matches(OPAQUE_REFERENCE_PREFIX).count(),
        1
    );
    assert!(selected_handoff.contains("codebase_memory_get_code_snippet"));
    assert!(selected_handoff.contains("selector field=qualified_name"));
    for hidden in active_references
        .iter()
        .filter(|reference| *reference != &selected_reference)
        .chain(&sibling_references)
    {
        assert!(!selected_handoff.contains(hidden));
    }
    for private in ["worker_slot", "sibling_3", "total", "has_more"] {
        assert!(!selected_handoff.contains(private));
    }

    let denied = complete_llm(
        &mut machine,
        assistant(vec![
            (
                "active-raw",
                "codebase_memory_get_code_snippet",
                source_input("worker_slot"),
            ),
            (
                "active-distractor",
                "codebase_memory_get_code_snippet",
                source_input("temper-recovery-selector:distractor"),
            ),
            (
                "active-alternate",
                "codebase_memory_get_code_snippet",
                source_input(&active_references[0]),
            ),
            (
                "sibling-attempt",
                "codebase_memory_get_code_snippet",
                source_input(&sibling_references[0]),
            ),
        ]),
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
    for id in [
        "active-raw",
        "active-distractor",
        "active-alternate",
        "sibling-attempt",
    ] {
        assert!(denied.iter().any(|request| matches!(
            request,
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
                rejection: None,
                ..
            } if call.id == id && details == &expected
        )));
    }
    for id in ["active-distractor", "active-alternate", "sibling-attempt"] {
        assert!(denied.iter().any(|request| matches!(
            request,
            AgentRequest::Emit(AgentEvent::ToolStart {
                id: emitted_id,
                recovery_reference_disposition: Some(
                    GraphRecoveryReferenceDispositionV1::Rejected
                ),
                ..
            }) if emitted_id == id
        )));
    }

    let mut resumed = Vec::new();
    for id in [
        "active-raw",
        "active-distractor",
        "active-alternate",
        "sibling-attempt",
    ] {
        resumed.extend(complete_tool(
            &mut machine,
            id,
            ToolOutput {
                content: Vec::new(),
                details: None,
                is_error: true,
            },
            Some(ToolFailureDiagnostic::graph_exploration(expected.clone())),
        ));
    }
    let resumed_handoff = active_handoff(&resumed);
    assert_eq!(
        handoff_reference(resumed_handoff),
        selected_reference.as_str()
    );
    assert_eq!(resumed_handoff.matches(&selected_reference).count(), 1);
    for hidden in active_references
        .iter()
        .filter(|reference| *reference != &selected_reference)
        .chain(&sibling_references)
    {
        assert!(!resumed_handoff.contains(hidden));
    }

    let exact_input = source_input(&selected_reference);
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
    assert_eq!(provider_input["qualified_name"], "worker_slot");
    let advanced = registry
        .record_with_expanded_recovery(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &provider_input,
            Some(&structured_parts(serde_json::json!({
                "name": "worker_slot",
                "qualified_name": "worker_slot",
                "source": "fn worker_slot() {}",
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
            Some(&expanded),
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

#[test]
fn active_root_handoff_selects_the_first_presented_admission_valid_provider_candidate() {
    run_active_root_handoff(false, false);
}

#[test]
fn active_root_handoff_is_stable_when_parallel_roots_complete_in_reverse() {
    run_active_root_handoff(true, false);
}

#[test]
fn active_root_without_a_valid_presented_candidate_does_not_borrow_from_its_sibling() {
    run_active_root_handoff(true, true);
}
