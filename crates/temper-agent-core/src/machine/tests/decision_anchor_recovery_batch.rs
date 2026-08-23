// Root-local, immutable recovery-batch regressions.

use super::*;
use crate::{
    EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionResolver,
    LineageAdmissionStatus,
};

struct RecoveryAdmissionFixture;

impl LineageAdmissionResolver for RecoveryAdmissionFixture {
    fn resolve(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> LineageAdmissionOutcome {
        let tool = GraphCorrelationToolV1::from_public_name(tool_name);
        match tool {
            Some(GraphCorrelationToolV1::TracePath) => eligible_admission(
                ROOT,
                GraphCorrelationToolV1::TracePath,
                None,
            )
            .expect("trace fixture admission"),
            Some(GraphCorrelationToolV1::GetCodeSnippet) => {
                let root = if arguments.get("qualified_name").and_then(serde_json::Value::as_str)
                    == Some("active")
                {
                    ROOT
                } else {
                    OTHER_ROOT
                };
                let evidence = arguments
                    .get("decision_evidence_kind")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok());
                eligible_admission(root, GraphCorrelationToolV1::GetCodeSnippet, evidence)
                    .unwrap_or(LineageAdmissionOutcome::Ineligible(
                        LineageAdmissionStatus::MalformedSelector,
                    ))
            }
            Some(GraphCorrelationToolV1::SearchGraph) => LineageAdmissionOutcome::Ineligible(
                LineageAdmissionStatus::BroadSelector,
            ),
            Some(GraphCorrelationToolV1::SearchCode) => LineageAdmissionOutcome::Ineligible(
                LineageAdmissionStatus::IncapableSelection,
            ),
            None => LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnsupportedTool),
        }
    }
}

fn eligible_admission(
    root: &str,
    tool: GraphCorrelationToolV1,
    evidence: Option<DecisionEvidenceKindV1>,
) -> Option<LineageAdmissionOutcome> {
    let selector = match tool {
        GraphCorrelationToolV1::TracePath => DecisionAnchorTargetKindV1::FunctionName,
        GraphCorrelationToolV1::GetCodeSnippet => DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::SearchCode => DecisionAnchorTargetKindV1::Pattern,
        GraphCorrelationToolV1::SearchGraph => DecisionAnchorTargetKindV1::GraphQuery,
    };
    Some(LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::new(root.to_string(), selector, tool, evidence)
            .expect("capable closed admission"),
    ))
}

#[test]
fn recovery_batch_uses_one_root_local_immutable_snapshot() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    assert!(state.blocks_mutation("write"));
    let calls = [
        source_call("cross-caller", DecisionEvidenceKindV1::Caller),
        call("trace", "codebase_memory_trace_path"),
        source_call("cross-test", DecisionEvidenceKindV1::FocusedTest),
    ];
    let admissions = [
        eligible_admission(
            OTHER_ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Caller),
        ),
        eligible_admission(ROOT, GraphCorrelationToolV1::TracePath, None),
        eligible_admission(
            OTHER_ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::FocusedTest),
        ),
    ];
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions(&calls, 3, &admissions),
        [
            recovery_graph_denial(all_missing(), 4),
            None,
            recovery_graph_denial(all_missing(), 4),
        ],
        "cross-root siblings are denied from the same pre-batch allowance snapshot",
    );

    let trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let denied_caller = output_with_evidence(
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Caller,
    );
    let denied_test = output_with_evidence(
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("cross-test", "codebase_memory_get_code_snippet", &denied_test),
            ("trace", "codebase_memory_trace_path", &trace),
            (
                "cross-caller",
                "codebase_memory_get_code_snippet",
                &denied_caller,
            ),
        ]),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let details = state.recovery_details().expect("root-local recovery details");
    assert_eq!(
        details.missing_evidence,
        [
            GraphRecoveryEvidenceKindV1::Implementation,
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ]
    );
    assert_eq!(details.remaining_allowance, 3);
    assert_eq!(details.compatible_actions.len(), 3);

    let source_calls = [
        source_call("implementation", DecisionEvidenceKindV1::Implementation),
        source_call("caller", DecisionEvidenceKindV1::Caller),
        source_call("test", DecisionEvidenceKindV1::FocusedTest),
    ];
    let source_admissions = [
        eligible_admission(
            ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Implementation),
        ),
        eligible_admission(
            ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Caller),
        ),
        eligible_admission(
            ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::FocusedTest),
        ),
    ];
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions(
            &source_calls,
            4,
            &source_admissions,
        ),
        [None, None, None],
    );
    let implementation = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Implementation,
    );
    let caller = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::Caller,
    );
    let test = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("test", "codebase_memory_get_code_snippet", &test),
            (
                "implementation",
                "codebase_memory_get_code_snippet",
                &implementation,
            ),
            ("caller", "codebase_memory_get_code_snippet", &caller),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert_eq!(state.on_tool_dispatched(&call("mutation", "write"), 5), None);
    assert_eq!(
        state.on_tool_dispatched(&call("closed", "codebase_memory_trace_path"), 5),
        completed_graph_denial(),
    );
}

#[test]
fn immutable_recovery_admission_denies_ineligible_duplicates_and_trace_dependents() {
    use crate::LineageAdmissionStatus::{
        AmbiguousSelector, BroadSelector, IncapableSelection, MalformedSelector,
        UnknownSelector, UnsupportedTool,
    };

    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    let calls = [
        call("trace", "codebase_memory_trace_path"),
        call("duplicate-trace", "codebase_memory_trace_path"),
        source_call("same-root-source", DecisionEvidenceKindV1::Implementation),
        call("ambiguous", "codebase_memory_trace_path"),
        call("malformed", "codebase_memory_trace_path"),
        call("broad", "codebase_memory_search_graph"),
        call("unsupported", "codebase_memory_get_architecture"),
        source_call("incapable", DecisionEvidenceKindV1::Caller),
        call("unknown", "codebase_memory_trace_path"),
    ];
    let admissions = [
        eligible_admission(ROOT, GraphCorrelationToolV1::TracePath, None),
        eligible_admission(ROOT, GraphCorrelationToolV1::TracePath, None),
        eligible_admission(
            ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Implementation),
        ),
        Some(LineageAdmissionOutcome::Ineligible(AmbiguousSelector)),
        Some(LineageAdmissionOutcome::Ineligible(MalformedSelector)),
        Some(LineageAdmissionOutcome::Ineligible(BroadSelector)),
        Some(LineageAdmissionOutcome::Ineligible(UnsupportedTool)),
        Some(LineageAdmissionOutcome::Ineligible(IncapableSelection)),
        Some(LineageAdmissionOutcome::Ineligible(UnknownSelector)),
    ];
    let denials = state.on_tool_batch_dispatched_with_admissions(&calls, 3, &admissions);
    assert_eq!(denials[0], None);
    for denial in &denials[1..] {
        assert_eq!(*denial, recovery_graph_denial(all_missing(), 4));
    }

    assert_eq!(
        state.on_tool_finished(
            "trace",
            "codebase_memory_trace_path",
            &output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state
            .recovery_details()
            .expect("trace settlement keeps recovery actionable")
            .remaining_allowance,
        3,
    );
}

#[test]
fn machine_executes_only_the_snapshot_compatible_root_call() {
    let mut machine = AgentMachine::with_effects(vec![user("repair")], 12, effects())
        .with_lineage_admission(Arc::new(RecoveryAdmissionFixture));
    let _ = machine.on_start(EngineTime::ZERO);

    let _ = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[
            ("active-root", "codebase_memory_search_graph"),
            ("sibling-root", "codebase_memory_search_graph"),
        ])),
    );
    assert!(
        complete(
            &mut machine,
            tool_finished(
                "sibling-root",
                output(
                    "codebase_memory_search_graph",
                    OTHER_ROOT,
                    DecisionAnchorLineageStageV1::Root,
                ),
            ),
        )
        .is_empty()
    );
    let _ = complete(
        &mut machine,
        tool_finished(
            "active-root",
            output(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::Root,
            ),
        ),
    );
    for id in ["broad-one", "broad-two"] {
        let _ = complete(
            &mut machine,
            llm_responded(assistant_tool_calls(&[(
                id,
                "codebase_memory_get_architecture",
            )])),
        );
        let _ = complete(&mut machine, tool_finished(id, plain_success()));
    }

    let mut recovery = assistant_tool_calls(&[
        ("cross-caller", "codebase_memory_get_code_snippet"),
        ("trace", "codebase_memory_trace_path"),
        ("cross-test", "codebase_memory_get_code_snippet"),
    ]);
    for block in &mut recovery.content {
        let tongs::model::ContentBlock::ToolCall(call) = block else {
            continue;
        };
        call.arguments = match call.id.as_str() {
            "trace" => serde_json::json!({"function_name": "active"}),
            "cross-caller" => serde_json::json!({
                "qualified_name": "cross",
                "decision_evidence_kind": "caller",
            }),
            "cross-test" => serde_json::json!({
                "qualified_name": "cross",
                "decision_evidence_kind": "focused_test",
            }),
            _ => unreachable!(),
        };
    }
    let requests = complete(&mut machine, llm_responded(recovery));
    let executable = requests
        .iter()
        .filter_map(|request| match request {
            AgentRequest::RunTool {
                call,
                denial: None,
                ..
            } => Some(call.id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(executable, ["trace"]);

    assert!(
        complete(
            &mut machine,
            tool_finished("cross-test", plain_success()),
        )
        .is_empty()
    );
    assert!(
        complete(
            &mut machine,
            tool_finished("cross-caller", plain_success()),
        )
        .is_empty()
    );
    let after_trace = complete(
        &mut machine,
        tool_finished(
            "trace",
            output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
    );
    assert!(message_containing(
        &after_trace,
        "missing evidence: [implementation, caller, focused_test]"
    ));
    assert!(message_containing(&after_trace, "remaining allowance: 3"));

    let mut sources = assistant_tool_calls(&[
        ("implementation", "codebase_memory_get_code_snippet"),
        ("caller", "codebase_memory_get_code_snippet"),
        ("test", "codebase_memory_get_code_snippet"),
    ]);
    for block in &mut sources.content {
        let tongs::model::ContentBlock::ToolCall(call) = block else {
            continue;
        };
        call.arguments = serde_json::json!({
            "qualified_name": "active",
            "decision_evidence_kind": match call.id.as_str() {
                "implementation" => "implementation",
                "caller" => "caller",
                "test" => "focused_test",
                _ => unreachable!(),
            },
        });
    }
    let requests = complete(&mut machine, llm_responded(sources));
    assert_eq!(
        requests
            .iter()
            .filter(|request| matches!(
                request,
                AgentRequest::RunTool { denial: None, .. }
            ))
            .count(),
        3,
    );
    assert!(
        complete(
            &mut machine,
            tool_finished(
                "test",
                output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionEvidenceKindV1::FocusedTest,
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
    let converged = complete(
        &mut machine,
        tool_finished(
            "implementation",
            output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::Implementation,
            ),
        ),
    );
    assert_eq!(message_count(&converged, DECISION_ANCHOR_CONVERGENCE_MESSAGE), 1);

    let mutation = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[("mutation", "write")])),
    );
    assert!(mutation.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool {
            call,
            denial: None,
            ..
        } if call.id == "mutation"
    )));
    let _ = complete(&mut machine, tool_finished("mutation", plain_success()));
    let closed = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[("closed", "codebase_memory_trace_path")])),
    );
    assert!(closed.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool {
            call,
            denial: Some(ToolCallDenial::GraphExplorationClosed(_)),
            ..
        } if call.id == "closed"
    )));
}

