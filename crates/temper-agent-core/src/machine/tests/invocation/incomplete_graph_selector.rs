use crate::EligibleLineageAdmission;
use temper_protocol_activity::{
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, GraphCorrelationToolV1,
    GraphRecoveryReferenceDispositionV1,
};

const ACTIVE_ROOT: &str = "00000000-0000-4000-8000-000000000001";
const ACTIVE_IMPLEMENTATION: &str = "returned-implementation";
const UNKNOWN_SELECTOR: &str = "schema-valid-but-never-returned";
const RECOVERY_REFERENCE: &str =
    "temper-recovery-selector:00000000-0000-4000-8000-000000000002";

struct RootAwareTraversalAdmission;

impl LineageAdmissionResolver for RootAwareTraversalAdmission {
    fn resolve(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> LineageAdmissionOutcome {
        let eligible = match tool_name {
            "codebase_memory_get_code_snippet"
                if arguments["qualified_name"] == ACTIVE_IMPLEMENTATION =>
            {
                EligibleLineageAdmission::new(
                    ACTIVE_ROOT.to_string(),
                    DecisionAnchorTargetKindV1::QualifiedName,
                    GraphCorrelationToolV1::GetCodeSnippet,
                    Some(DecisionEvidenceKindV1::Implementation),
                )
            }
            "codebase_memory_trace_path"
                if arguments["function_name"] == ACTIVE_IMPLEMENTATION
                    || arguments["function_name"] == RECOVERY_REFERENCE =>
            {
                EligibleLineageAdmission::implementation_caller_traversal(
                    ACTIVE_ROOT.to_string(),
                    DecisionAnchorTargetKindV1::FunctionName,
                )
            }
            _ => None,
        };
        eligible.map_or_else(
            || LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector),
            LineageAdmissionOutcome::Eligible,
        )
    }

    fn resolve_for_active_root_with_recovery(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        _active_root: Option<&str>,
    ) -> (
        LineageAdmissionOutcome,
        Option<GraphRecoveryReferenceDispositionV1>,
    ) {
        let outcome = self.resolve(tool_name, arguments);
        let disposition = (tool_name == "codebase_memory_trace_path").then(|| {
            match arguments.get("function_name").and_then(serde_json::Value::as_str) {
                Some(RECOVERY_REFERENCE) => GraphRecoveryReferenceDispositionV1::Recognized,
                Some(value) if value.starts_with("temper-recovery-selector:") => {
                    GraphRecoveryReferenceDispositionV1::Rejected
                }
                Some(_) | None => GraphRecoveryReferenceDispositionV1::Missing,
            }
        });
        (outcome, disposition)
    }

    fn trace_recovery_selector(
        &self,
        active_root: &str,
    ) -> Option<crate::OpaqueRecoverySelectorReference> {
        (active_root == ACTIVE_ROOT)
            .then(|| crate::OpaqueRecoverySelectorReference::new(RECOVERY_REFERENCE.to_string()))
            .flatten()
    }
}

#[test]
fn staged_incomplete_trace_returns_an_invokable_opaque_selector_then_accepts_it() {
    use temper_protocol_activity::{
        DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
        DecisionEvidenceKindV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1,
        GraphCorrelationV1, GraphExplorationClosedV1, GraphRecoveryEvidenceKindV1,
    };

    let catalog = catalog(&[
        "codebase_memory_search_graph",
        "codebase_memory_get_code_snippet",
        "codebase_memory_trace_path",
    ]);
    assert!(
        crate::arguments_match(
            catalog
                .schema("codebase_memory_trace_path")
                .expect("trace schema"),
            &serde_json::json!({"direction":"inbound"}),
        ),
        "the regression schema deliberately admits a direction-only traversal"
    );
    let mut machine =
        machine(catalog).with_lineage_admission(Arc::new(RootAwareTraversalAdmission));
    let _ = machine.on_start(EngineTime::ZERO);

    let output = |tool: GraphCorrelationToolV1,
                  target_kind: GraphCorrelationTargetKindV1,
                  stage: DecisionAnchorLineageStageV1,
                  evidence: Option<DecisionEvidenceKindV1>| {
        let result_kinds = [
            DecisionAnchorTargetKindV1::FunctionName,
            DecisionAnchorTargetKindV1::QualifiedName,
        ];
        let lineage = evidence.map_or_else(
            || {
                DecisionAnchorLineageV1::new(
                    "00000000-0000-4000-8000-000000000001".to_string(),
                    stage,
                    DecisionAnchorTargetKindV1::from_graph_correlation(target_kind),
                    result_kinds,
                )
                .unwrap()
            },
            |evidence| {
                DecisionAnchorLineageV1::new_with_decision_evidence_kind(
                    "00000000-0000-4000-8000-000000000001".to_string(),
                    stage,
                    DecisionAnchorTargetKindV1::from_graph_correlation(target_kind),
                    result_kinds,
                    evidence,
                )
                .unwrap()
            },
        );
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
            vec![(
                "root",
                "codebase_memory_search_graph",
                serde_json::json!({"query":"selected implementation"}),
            )],
        )),
    );
    let _ = complete(
        &mut machine,
        tool_finished(
            "root",
            output(
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
                DecisionAnchorLineageStageV1::Root,
                None,
            ),
        ),
    );

    let _ = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name":"returned-implementation",
                    "decision_evidence_kind":"implementation"
                }),
            )],
        )),
    );
    let _ = complete(
        &mut machine,
        tool_finished(
            "implementation",
            output(
                GraphCorrelationToolV1::GetCodeSnippet,
                GraphCorrelationTargetKindV1::QualifiedName,
                DecisionAnchorLineageStageV1::CarryForward,
                Some(DecisionEvidenceKindV1::Implementation),
            ),
        ),
    );

    let denied = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "incomplete-trace",
                "codebase_memory_trace_path",
                serde_json::json!({"direction":"inbound"}),
            )],
        )),
    );
    assert!(denied.iter().any(|request| matches!(
        request,
        AgentRequest::Emit(AgentEvent::ToolStart {
            id,
            recovery_reference_disposition: Some(
                GraphRecoveryReferenceDispositionV1::Missing
            ),
            ..
        }) if id == "incomplete-trace"
    )));
    let expected = GraphExplorationClosedV1::recoverable_without_actions(
        [
            GraphRecoveryEvidenceKindV1::Trace,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ],
        4,
    )
    .unwrap();
    let details = denied
        .iter()
        .find_map(|request| match request {
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
                rejection: None,
                ..
            } if call.id == "incomplete-trace" => Some(details.clone()),
            _ => None,
        })
        .expect("incomplete traversal is denied before provider dispatch");
    assert_eq!(details, expected);
    let scrubbed = denied.iter().find_map(|request| match request {
        AgentRequest::Emit(AgentEvent::AssistantMessage { content }) => {
            content.iter().find_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call),
                _ => None,
            })
        }
        _ => None,
    });
    let scrubbed = scrubbed.expect("scrubbed assistant call");
    assert_eq!(scrubbed.name, REJECTED_TOOL_NAME);
    assert_eq!(scrubbed.arguments, serde_json::json!({}));

    let next_turn = complete(
        &mut machine,
        tool_failed(
            "incomplete-trace",
            tool_output("local traversal denial", true),
            ToolFailureDiagnostic::graph_exploration(details),
        ),
    );
    assert!(next_turn.iter().any(|request| match request {
        AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| {
            matches!(message, Message::User(message)
                if matches!(&message.content, tongs::model::UserContent::Text(text)
                    if text.contains("required next call=[codebase_memory_trace_path")
                        && text.contains(RECOVERY_REFERENCE)
                        && text.contains("\"direction\":\"inbound\"")))
        }),
        _ => false,
    }));

    let second_denied = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "wrong-field-trace",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "qualified_name": ACTIVE_IMPLEMENTATION,
                    "direction": "inbound"
                }),
            )],
        )),
    );
    let second_details = second_denied
        .iter()
        .find_map(|request| match request {
            AgentRequest::RunTool {
                call,
                denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
                rejection: None,
                ..
            } if call.id == "wrong-field-trace" => Some(details.clone()),
            _ => None,
        })
        .expect("repeated malformed traversal remains a local decision denial");
    assert_eq!(second_details, expected);
    let second_next_turn = complete(
        &mut machine,
        tool_failed(
            "wrong-field-trace",
            tool_output("local traversal denial", true),
            ToolFailureDiagnostic::graph_exploration(second_details),
        ),
    );
    assert!(second_next_turn.iter().any(|request| match request {
        AgentRequest::CallLlm { messages, .. } => messages.iter().any(|message| {
            matches!(message, Message::User(message)
                if matches!(&message.content, tongs::model::UserContent::Text(text)
                    if text.contains(RECOVERY_REFERENCE)))
        }),
        _ => false,
    }));

    let recovered = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "complete-trace",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": RECOVERY_REFERENCE,
                    "direction":"inbound"
                }),
            )],
        )),
    );
    assert!(recovered.iter().any(|request| matches!(
        request,
        AgentRequest::Emit(AgentEvent::ToolStart {
            id,
            recovery_reference_disposition: Some(
                GraphRecoveryReferenceDispositionV1::Recognized
            ),
            ..
        }) if id == "complete-trace"
    )));
    assert!(recovered.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool { call, denial: None, rejection: None, .. }
            if call.id == "complete-trace"
                && call.name == "codebase_memory_trace_path"
                && call.arguments["function_name"] == RECOVERY_REFERENCE
    )));
}

#[test]
fn staged_unknown_trace_is_denied_before_dispatch_then_accepts_active_selector() {
    use temper_protocol_activity::{
        DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, GraphCorrelationTargetKindV1,
        GraphCorrelationV1, GraphExplorationClosedV1, GraphRecoveryEvidenceKindV1,
    };

    let catalog = catalog(&[
        "codebase_memory_search_graph",
        "codebase_memory_get_code_snippet",
        "codebase_memory_trace_path",
    ]);
    assert!(crate::arguments_match(
        catalog
            .schema("codebase_memory_trace_path")
            .expect("trace schema"),
        &serde_json::json!({
            "function_name": UNKNOWN_SELECTOR,
            "direction": "inbound"
        }),
    ));
    let mut machine = machine(catalog)
        .with_lineage_admission(Arc::new(RootAwareTraversalAdmission));
    let _ = machine.on_start(EngineTime::ZERO);

    let output = |root: &str,
                  tool: GraphCorrelationToolV1,
                  target_kind: GraphCorrelationTargetKindV1,
                  stage: DecisionAnchorLineageStageV1,
                  evidence: Option<DecisionEvidenceKindV1>| {
        let result_kinds = [
            DecisionAnchorTargetKindV1::FunctionName,
            DecisionAnchorTargetKindV1::QualifiedName,
        ];
        let lineage = evidence.map_or_else(
            || {
                DecisionAnchorLineageV1::new(
                    root.to_string(),
                    stage,
                    DecisionAnchorTargetKindV1::from_graph_correlation(target_kind),
                    result_kinds,
                )
                .unwrap()
            },
            |evidence| {
                DecisionAnchorLineageV1::new_with_decision_evidence_kind(
                    root.to_string(),
                    stage,
                    DecisionAnchorTargetKindV1::from_graph_correlation(target_kind),
                    result_kinds,
                    evidence,
                )
                .unwrap()
            },
        );
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
            vec![(
                "active-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query":"selected implementation"}),
            )],
        )),
    );
    let _ = complete(
        &mut machine,
        tool_finished(
            "active-root",
            output(
                ACTIVE_ROOT,
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
                DecisionAnchorLineageStageV1::Root,
                None,
            ),
        ),
    );
    let _ = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "implementation",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": ACTIVE_IMPLEMENTATION,
                    "decision_evidence_kind": "implementation"
                }),
            )],
        )),
    );
    let _ = complete(
        &mut machine,
        tool_finished(
            "implementation",
            output(
                ACTIVE_ROOT,
                GraphCorrelationToolV1::GetCodeSnippet,
                GraphCorrelationTargetKindV1::QualifiedName,
                DecisionAnchorLineageStageV1::CarryForward,
                Some(DecisionEvidenceKindV1::Implementation),
            ),
        ),
    );

    let denied = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "non-returned-trace",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": UNKNOWN_SELECTOR,
                    "direction": "inbound"
                }),
            )],
        )),
    );
    let expected = GraphExplorationClosedV1::recoverable_without_actions(
        [
            GraphRecoveryEvidenceKindV1::Trace,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ],
        4,
    )
    .unwrap();
    assert!(denied.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool {
            call,
            denial: Some(ToolCallDenial::GraphExplorationClosed(Some(details))),
            rejection: None,
            ..
        } if call.id == "non-returned-trace" && details == &expected
    )));

    let _ = complete(
        &mut machine,
        tool_failed(
            "non-returned-trace",
            tool_output("local traversal denial", true),
            ToolFailureDiagnostic::graph_exploration(expected),
        ),
    );
    let recovered = complete(
        &mut machine,
        llm_responded(assistant(
            "openai-responses",
            vec![(
                "active-trace",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": ACTIVE_IMPLEMENTATION,
                    "direction": "inbound"
                }),
            )],
        )),
    );
    assert!(recovered.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool { call, denial: None, rejection: None, .. }
            if call.id == "active-trace"
                && call.arguments["function_name"] == ACTIVE_IMPLEMENTATION
    )));
}
