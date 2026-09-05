// Production-shaped parallel projected-root selector handoff regression.

const ACTIVE_CANDIDATE_REFERENCE: &str =
    "temper-recovery-selector:00000000-0000-4000-8000-000000000010";
const ACTIVE_TRACE_REFERENCE: &str =
    "temper-recovery-selector:00000000-0000-4000-8000-000000000011";
const SIBLING_CANDIDATE_REFERENCE: &str =
    "temper-recovery-selector:00000000-0000-4000-8000-000000000012";
const SIBLING_ROOT: &str = "00000000-0000-4000-8000-000000000013";

struct ParallelProjectedAdmission;

impl LineageAdmissionResolver for ParallelProjectedAdmission {
    fn resolve(
        &self,
        _tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> LineageAdmissionOutcome {
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector)
    }

    fn resolve_for_active_root_with_recovery(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        active_root: Option<&str>,
    ) -> (
        LineageAdmissionOutcome,
        Option<GraphRecoveryReferenceDispositionV1>,
    ) {
        let reference = arguments.as_object().and_then(|arguments| {
            arguments.values().find_map(|value| {
                value.as_str().filter(|value| {
                    value.starts_with("temper-recovery-selector:")
                })
            })
        });
        let eligible = match tool_name {
            "codebase_memory_get_code_snippet"
                if active_root == Some(ACTIVE_ROOT)
                    && arguments["qualified_name"] == ACTIVE_CANDIDATE_REFERENCE
                    && arguments["decision_evidence_kind"] == "implementation" =>
            {
                EligibleLineageAdmission::new(
                    ACTIVE_ROOT.to_string(),
                    DecisionAnchorTargetKindV1::QualifiedName,
                    GraphCorrelationToolV1::GetCodeSnippet,
                    Some(DecisionEvidenceKindV1::Implementation),
                )
            }
            "codebase_memory_trace_path"
                if active_root == Some(ACTIVE_ROOT)
                    && arguments["function_name"] == ACTIVE_TRACE_REFERENCE =>
            {
                EligibleLineageAdmission::implementation_caller_traversal(
                    ACTIVE_ROOT.to_string(),
                    DecisionAnchorTargetKindV1::FunctionName,
                )
            }
            _ => None,
        };
        let disposition = reference.map(|_| {
            if eligible.is_some() {
                GraphRecoveryReferenceDispositionV1::Recognized
            } else {
                GraphRecoveryReferenceDispositionV1::Rejected
            }
        });
        (
            eligible.map_or_else(
                || LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector),
                LineageAdmissionOutcome::Eligible,
            ),
            disposition,
        )
    }

    fn recovery_reference_disposition(
        &self,
        _tool_name: &str,
        arguments: &serde_json::Value,
        _active_root: Option<&str>,
    ) -> Option<GraphRecoveryReferenceDispositionV1> {
        arguments
            .as_object()
            .is_some_and(|arguments| {
                arguments.values().any(|value| {
                    value.as_str().is_some_and(|value| {
                        value.starts_with("temper-recovery-selector:")
                    })
                })
            })
            .then_some(GraphRecoveryReferenceDispositionV1::Rejected)
    }

    fn active_root_recovery_selector(
        &self,
        active_root: &str,
        action: temper_protocol_activity::GraphRecoveryActionV1,
    ) -> Option<crate::OpaqueRecoverySelectorReference> {
        let reference = if active_root == ACTIVE_ROOT
            && action
                == temper_protocol_activity::GraphRecoveryActionV1::for_evidence(
                    temper_protocol_activity::GraphRecoveryEvidenceKindV1::Implementation,
                )
        {
            ACTIVE_CANDIDATE_REFERENCE
        } else if active_root == ACTIVE_ROOT
            && action
                == temper_protocol_activity::GraphRecoveryActionV1::for_evidence(
                    temper_protocol_activity::GraphRecoveryEvidenceKindV1::Trace,
                )
        {
            ACTIVE_TRACE_REFERENCE
        } else if active_root == SIBLING_ROOT {
            SIBLING_CANDIDATE_REFERENCE
        } else {
            return None;
        };
        crate::OpaqueRecoverySelectorReference::new(reference.to_string())
    }
}

#[test]
fn parallel_projected_roots_handoff_one_active_selector_and_preserve_its_allowance() {
    use temper_protocol_activity::{
        DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, GraphCorrelationTargetKindV1,
        GraphCorrelationV1, GraphExplorationClosedV1, GraphRecoveryEvidenceKindV1,
    };

    let catalog = catalog(&[
        "codebase_memory_search_graph",
        "codebase_memory_get_code_snippet",
        "codebase_memory_trace_path",
    ]);
    let mut machine = machine(catalog).with_lineage_admission(Arc::new(ParallelProjectedAdmission));
    let _ = machine.on_start(EngineTime::ZERO);

    let output = |root: &str,
                  stage: DecisionAnchorLineageStageV1,
                  evidence: Option<DecisionEvidenceKindV1>| {
        let lineage = evidence.map_or_else(
            || {
                DecisionAnchorLineageV1::new(
                    root.to_string(),
                    stage,
                    DecisionAnchorTargetKindV1::GraphQuery,
                    [
                        DecisionAnchorTargetKindV1::FunctionName,
                        DecisionAnchorTargetKindV1::QualifiedName,
                    ],
                )
                .unwrap()
            },
            |evidence| {
                DecisionAnchorLineageV1::new_with_decision_evidence_kind(
                    root.to_string(),
                    stage,
                    DecisionAnchorTargetKindV1::QualifiedName,
                    [
                        DecisionAnchorTargetKindV1::FunctionName,
                        DecisionAnchorTargetKindV1::QualifiedName,
                    ],
                    evidence,
                )
                .unwrap()
            },
        );
        let (tool, target_kind) = if evidence.is_some() {
            (
                GraphCorrelationToolV1::GetCodeSnippet,
                GraphCorrelationTargetKindV1::QualifiedName,
            )
        } else {
            (
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
            )
        };
        ToolOutput {
            content: Vec::new(),
            details: Some(serde_json::json!({
                SAFE_GRAPH_CORRELATION_DETAIL_KEY:
                    GraphCorrelationV1::new(tool, target_kind, "request").unwrap(),
                SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY: lineage,
            })),
            is_error: false,
        }
    };

    let _ = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![
                (
                    "broad-active",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"routing implementation and focused tests"}),
                ),
                (
                    "narrow-sibling",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query":"routing implementation"}),
                ),
            ],
        )),
    );
    assert!(complete(
        &mut machine,
        tool_finished(
            "narrow-sibling",
            output(SIBLING_ROOT, DecisionAnchorLineageStageV1::Root, None),
        ),
    )
    .is_empty());
    let selected = complete(
        &mut machine,
        tool_finished(
            "broad-active",
            output(ACTIVE_ROOT, DecisionAnchorLineageStageV1::Root, None),
        ),
    );
    let handoffs = selected
        .iter()
        .find_map(|request| match request {
            AgentRequest::CallLlm { messages, .. } => Some(
                messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::User(message) => match &message.content {
                            tongs::model::UserContent::Text(text)
                                if text.contains("Active-root selector handoff") =>
                            {
                                Some(text.as_str())
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("parallel settlement starts one guided model turn");
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].matches(ACTIVE_CANDIDATE_REFERENCE).count(), 1);
    assert!(handoffs[0].contains("codebase_memory_get_code_snippet"));
    assert!(handoffs[0].contains("selector field=qualified_name"));
    assert!(!handoffs[0].contains(SIBLING_CANDIDATE_REFERENCE));
    assert!(!handoffs[0].contains(ACTIVE_ROOT));
    assert!(!handoffs[0].contains(SIBLING_ROOT));

    let denied = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![
                (
                    "sibling-reference",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": SIBLING_CANDIDATE_REFERENCE,
                        "decision_evidence_kind": "implementation"
                    }),
                ),
                (
                    "wrong-purpose",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": ACTIVE_CANDIDATE_REFERENCE,
                        "decision_evidence_kind": "caller"
                    }),
                ),
                (
                    "wrong-field",
                    "codebase_memory_trace_path",
                    serde_json::json!({
                        "qualified_name": ACTIVE_TRACE_REFERENCE,
                        "direction": "inbound"
                    }),
                ),
            ],
        )),
    );
    let expected = GraphExplorationClosedV1::recoverable_without_actions(
        [
            GraphRecoveryEvidenceKindV1::Trace,
            GraphRecoveryEvidenceKindV1::Implementation,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ],
        temper_protocol_activity::MAX_GRAPH_RECOVERY_ALLOWANCE_V1,
    )
    .unwrap();
    let denied_ids = denied
        .iter()
        .filter_map(|request| match request {
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
                rejection: None,
                ..
            } if details == &expected => Some(call.id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(denied_ids, ["sibling-reference", "wrong-purpose"]);
    assert_eq!(
        denied
            .iter()
            .filter(|request| matches!(
                request,
                AgentRequest::Emit(AgentEvent::ToolStart {
                    recovery_reference_disposition: Some(
                        GraphRecoveryReferenceDispositionV1::Rejected
                    ),
                    ..
                })
            ))
            .count(),
        2,
    );

    assert!(complete(
        &mut machine,
        tool_failed(
            "wrong-purpose",
            tool_output("local reference denial", true),
            ToolFailureDiagnostic::graph_exploration(expected.clone()),
        ),
    )
    .is_empty());
    let wrong_field_dispatch = complete(
        &mut machine,
        tool_failed(
            "sibling-reference",
            tool_output("local reference denial", true),
            ToolFailureDiagnostic::graph_exploration(expected.clone()),
        ),
    );
    assert!(wrong_field_dispatch.iter().any(|request| matches!(
        request,
        AgentRequest::Emit(AgentEvent::ToolStart {
            id,
            recovery_reference_disposition: Some(
                GraphRecoveryReferenceDispositionV1::Rejected
            ),
            ..
        }) if id == "wrong-field"
    )));
    let after_rejections = complete(
        &mut machine,
        tool_failed(
            "wrong-field",
            tool_output("local reference denial", true),
            ToolFailureDiagnostic::graph_exploration(expected),
        ),
    );
    assert!(after_rejections.iter().any(|request| match request {
        AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| {
            matches!(message, Message::User(message)
                if matches!(&message.content, tongs::model::UserContent::Text(text)
                    if text.contains("Active-root selector handoff")
                        && text.contains(ACTIVE_CANDIDATE_REFERENCE)
                        && !text.contains(SIBLING_CANDIDATE_REFERENCE)))
        }),
        _ => false,
    }));

    let admitted = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "active-implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": ACTIVE_CANDIDATE_REFERENCE,
                    "decision_evidence_kind": "implementation"
                }),
            )],
        )),
    );
    assert!(admitted.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool { call, denial: None, rejection: None, .. }
            if call.id == "active-implementation"
    )));
    let advanced = complete(
        &mut machine,
        tool_finished(
            "active-implementation",
            output(
                ACTIVE_ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                Some(DecisionEvidenceKindV1::Implementation),
            ),
        ),
    );
    assert!(advanced.iter().any(|request| match request {
        AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| {
            matches!(message, Message::User(message)
                if matches!(&message.content, tongs::model::UserContent::Text(text)
                    if text.contains("accepted evidence=[implementation]")
                        && !text.contains("Active-root selector handoff")))
        }) && messages.iter().any(|message| {
            matches!(message, Message::User(message)
                if matches!(&message.content, tongs::model::UserContent::Text(text)
                    if text.contains("Active-root selector handoff")
                        && text.contains(ACTIVE_TRACE_REFERENCE)
                        && text.contains("selector field=function_name")))
        }),
        _ => false,
    }));
}
