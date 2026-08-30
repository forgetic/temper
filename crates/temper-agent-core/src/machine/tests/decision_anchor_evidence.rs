// Typed evidence ordering and mutation-gating regressions.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionStatus};

fn source_admission(kind: DecisionEvidenceKindV1) -> LineageAdmissionOutcome {
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::new(
            ROOT.to_string(),
            DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(kind),
        )
        .unwrap(),
    )
}

#[test]
fn over_returned_later_kinds_cannot_bypass_the_implementation_route() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    state.take_model_guidance();

    let implementation = source_call("implementation", DecisionEvidenceKindV1::Implementation);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &implementation,
            1,
            Some(&source_admission(DecisionEvidenceKindV1::Implementation)),
        ),
        None,
    );
    finish_with_evidence(
        &mut state,
        "implementation",
        ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    state.take_model_guidance();

    for (id, kind) in [
        ("early-caller", DecisionEvidenceKindV1::Caller),
        ("early-test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        let attempted = source_call(id, kind);
        assert!(
            state
                .on_tool_dispatched_with_admission(
                    &attempted,
                    2,
                    Some(&LineageAdmissionOutcome::Ineligible(
                        LineageAdmissionStatus::IncapableSelection,
                    )),
                )
                .is_some(),
            "{kind:?} cannot bypass the inbound caller stage",
        );
        let guidance = state.take_model_guidance();
        assert!(guidance.iter().all(|message| {
            message.contains("trace_path/function_name/trace")
                || message.contains("stop without a product")
        }));
    }
    assert!(state.blocks_mutation("write"));
}

#[test]
fn empty_selected_implementation_caller_traversal_stops_incomplete() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    state.on_tool_finished(
        "root",
        "codebase_memory_search_graph",
        &output_with_kinds(
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
            &[
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ],
        ),
    );
    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    finish_with_evidence(
        &mut state,
        "implementation",
        ROOT,
        DecisionEvidenceKindV1::Implementation,
    );
    state.on_tool_dispatched(&call("empty-callers", "codebase_memory_trace_path"), 2);
    assert_eq!(
        state.on_tool_finished(
            "empty-callers",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::NoEligibleSelector,
            ),
        ),
        DecisionAnchorTransition::EnabledEvidenceIncomplete,
    );
    assert_eq!(
        state.on_tool_dispatched(&call("retry", "codebase_memory_trace_path"), 3),
        exhausted_graph_denial([
            GraphRecoveryEvidenceKindV1::Caller,
            GraphRecoveryEvidenceKindV1::FocusedTest,
        ]),
    );
}

#[test]
fn one_batch_admits_only_the_first_provider_derived_stage() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    let calls = [
        source_call("implementation", DecisionEvidenceKindV1::Implementation),
        call("trace", "codebase_memory_trace_path"),
        source_call("caller", DecisionEvidenceKindV1::Caller),
    ];
    let denials = state.on_tool_batch_dispatched_with_admissions(
        &calls,
        1,
        &[
            Some(source_admission(DecisionEvidenceKindV1::Implementation)),
            Some(LineageAdmissionOutcome::Ineligible(
                LineageAdmissionStatus::IncapableSelection,
            )),
            Some(LineageAdmissionOutcome::Ineligible(
                LineageAdmissionStatus::IncapableSelection,
            )),
        ],
    );
    assert_eq!(denials[0], None);
    assert!(denials[1].is_some());
    assert!(denials[2].is_some());
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::Unchanged,
    );
    assert_eq!(state.recovery_details(), None);
}

include!("decision_anchor_staged_guidance.rs");
