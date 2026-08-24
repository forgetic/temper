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
        let Some(tool) = GraphCorrelationToolV1::from_public_name(tool_name) else {
            return LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnsupportedTool);
        };
        match tool {
            GraphCorrelationToolV1::TracePath => {
                eligible_admission(ROOT, GraphCorrelationToolV1::TracePath, None).unwrap()
            }
            GraphCorrelationToolV1::GetCodeSnippet => {
                let root = if arguments
                    .get("qualified_name")
                    .and_then(serde_json::Value::as_str)
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
                eligible_admission(root, GraphCorrelationToolV1::GetCodeSnippet, evidence).unwrap()
            }
            GraphCorrelationToolV1::SearchGraph => {
                LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::BroadSelector)
            }
            GraphCorrelationToolV1::SearchCode => {
                LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection)
            }
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
fn immutable_recovery_batch_admits_only_the_next_staged_root_action() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    let calls = [
        source_call("cross-caller", DecisionEvidenceKindV1::Caller),
        source_call("implementation", DecisionEvidenceKindV1::Implementation),
        source_call("duplicate-implementation", DecisionEvidenceKindV1::Implementation),
        source_call("cross-test", DecisionEvidenceKindV1::FocusedTest),
    ];
    let admissions = [
        eligible_admission(
            OTHER_ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Caller),
        ),
        eligible_admission(
            ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Implementation),
        ),
        eligible_admission(
            ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::Implementation),
        ),
        eligible_admission(
            OTHER_ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::FocusedTest),
        ),
    ];
    let denial = recovery_graph_denial(all_missing(), 4);
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions(&calls, 3, &admissions),
        [denial.clone(), None, denial.clone(), denial],
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let details = state.recovery_details().unwrap();
    assert_eq!(details.remaining_allowance, 3);
    assert_eq!(
        details.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Trace,
        )]
    );

    let trace = call("trace", "codebase_memory_trace_path");
    let speculative_caller = source_call("speculative-caller", DecisionEvidenceKindV1::Caller);
    let missing_after_implementation = [
        GraphRecoveryEvidenceKindV1::Trace,
        GraphRecoveryEvidenceKindV1::Caller,
        GraphRecoveryEvidenceKindV1::FocusedTest,
    ];
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions(
            &[trace, speculative_caller],
            4,
            &[
                eligible_admission(ROOT, GraphCorrelationToolV1::TracePath, None),
                eligible_admission(
                    ROOT,
                    GraphCorrelationToolV1::GetCodeSnippet,
                    Some(DecisionEvidenceKindV1::Caller),
                ),
            ],
        ),
        [
            None,
            recovery_graph_denial(missing_after_implementation, 3),
        ],
        "the trace result cannot make its same-batch caller source eligible",
    );
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
    assert_eq!(state.recovery_details().unwrap().remaining_allowance, 2);

    for (turn, id, kind, expected) in [
        (
            5,
            "caller",
            DecisionEvidenceKindV1::Caller,
            DecisionAnchorTransition::GapRecoveryNeeded,
        ),
        (
            6,
            "test",
            DecisionEvidenceKindV1::FocusedTest,
            DecisionAnchorTransition::Converged,
        ),
    ] {
        let admission = eligible_admission(
            ROOT,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(kind),
        );
        assert_eq!(
            state.on_tool_dispatched_with_admission(
                &source_call(id, kind),
                turn,
                admission.as_ref(),
            ),
            None,
        );
        assert_eq!(finish_with_evidence(&mut state, id, ROOT, kind), expected);
    }
    assert_eq!(state.on_tool_dispatched(&call("mutation", "write"), 7), None);
}

#[test]
fn machine_executes_only_each_snapshot_compatible_staged_call() {
    let mut machine = AgentMachine::with_effects(vec![user("repair")], 12, effects())
        .with_lineage_admission(Arc::new(RecoveryAdmissionFixture));
    let _ = machine.on_start(EngineTime::ZERO);
    let _ = complete(
        &mut machine,
        llm_responded(assistant_tool_calls(&[("root", "codebase_memory_search_graph")])),
    );
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

    let mut first = assistant_tool_calls(&[
        ("cross-caller", "codebase_memory_get_code_snippet"),
        ("implementation", "codebase_memory_get_code_snippet"),
        ("cross-test", "codebase_memory_get_code_snippet"),
    ]);
    for block in &mut first.content {
        let tongs::model::ContentBlock::ToolCall(call) = block else {
            continue;
        };
        call.arguments = serde_json::json!({
            "qualified_name": if call.id == "implementation" { "active" } else { "cross" },
            "decision_evidence_kind": match call.id.as_str() {
                "implementation" => "implementation",
                "cross-caller" => "caller",
                "cross-test" => "focused_test",
                _ => unreachable!(),
            },
        });
    }
    let requests = complete(&mut machine, llm_responded(first));
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
    assert_eq!(executable, ["implementation"]);
    assert!(complete(&mut machine, tool_finished("cross-caller", plain_success())).is_empty());
    assert!(complete(&mut machine, tool_finished("cross-test", plain_success())).is_empty());
    let _ = complete(
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

    let mut trace = assistant_tool_calls(&[("trace", "codebase_memory_trace_path")]);
    let tongs::model::ContentBlock::ToolCall(trace_call) = &mut trace.content[0] else {
        unreachable!();
    };
    trace_call.arguments = serde_json::json!({"function_name": "active"});
    let requests = complete(&mut machine, llm_responded(trace));
    assert_eq!(run_tools(&requests), ["trace"]);
    let _ = complete(
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

    for (id, kind) in [
        ("caller", DecisionEvidenceKindV1::Caller),
        ("test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        let mut source = assistant_tool_calls(&[(id, "codebase_memory_get_code_snippet")]);
        let tongs::model::ContentBlock::ToolCall(call) = &mut source.content[0] else {
            unreachable!();
        };
        call.arguments = serde_json::json!({
            "qualified_name": "active",
            "decision_evidence_kind": kind,
        });
        let requests = complete(&mut machine, llm_responded(source));
        assert_eq!(run_tools(&requests), [id]);
        let settled = complete(
            &mut machine,
            tool_finished(
                id,
                output_with_evidence(ROOT, DecisionAnchorLineageStageV1::CarryForward, kind),
            ),
        );
        if kind == DecisionEvidenceKindV1::FocusedTest {
            assert!(message_containing(&settled, DECISION_ANCHOR_CONVERGENCE_MESSAGE));
        }
    }
}
