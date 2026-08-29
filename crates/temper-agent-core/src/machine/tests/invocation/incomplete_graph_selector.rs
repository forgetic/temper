#[test]
fn staged_incomplete_trace_is_a_local_decision_denial_then_accepts_exact_selector() {
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
    let mut machine = machine(catalog);
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
                    if text.contains("trace_path/function_name/trace/selector=implementation_evidence_result")
                        && text.contains("direction=inbound")))
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
                    "function_name":"returned-implementation",
                    "direction":"inbound"
                }),
            )],
        )),
    );
    assert!(recovered.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool { call, denial: None, rejection: None, .. }
            if call.id == "complete-trace"
                && call.name == "codebase_memory_trace_path"
                && call.arguments["function_name"] == "returned-implementation"
    )));
}
