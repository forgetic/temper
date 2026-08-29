// Typed evidence completion and mutation-gating regressions.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionStatus};

#[test]
fn over_returned_later_kinds_require_their_staged_provider_routes() {
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
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ],
        ),
    );

    state.on_tool_dispatched(
        &source_call("implementation", DecisionEvidenceKindV1::Implementation),
        1,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::Unchanged,
    );

    for (turn, id, kind, expected) in [
        (
            2,
            "over-returned-caller",
            DecisionEvidenceKindV1::Caller,
            DecisionAnchorTransition::Unchanged,
        ),
        (
            3,
            "over-returned-test",
            DecisionEvidenceKindV1::FocusedTest,
            DecisionAnchorTransition::GapRecoveryNeeded,
        ),
    ] {
        state.on_tool_dispatched(&source_call(id, kind), turn);
        assert_eq!(finish_with_evidence(&mut state, id, ROOT, kind), expected);
    }
    assert!(state.blocks_mutation("write"));

    state.on_tool_dispatched(&call("implementation-callers", "codebase_memory_trace_path"), 4);
    assert_eq!(
        state.on_tool_finished(
            "implementation-callers",
            "codebase_memory_trace_path",
            &output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    state.on_tool_dispatched(&source_call("exact-caller", DecisionEvidenceKindV1::Caller), 5);
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "exact-caller",
            ROOT,
            DecisionEvidenceKindV1::Caller,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );

    assert_eq!(
        finish_semantic_test_search(
            &mut state,
            "semantic-test-search",
            ROOT,
            6,
            FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    state.on_tool_dispatched(
        &source_call("exact-test", DecisionEvidenceKindV1::FocusedTest),
        7,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "exact-test",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::Converged,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("mutation", "write"), 8),
        Some(ToolCallDenial::DecisionAnchorMutation)
    );
}

#[test]
fn empty_selected_implementation_caller_traversal_releases_bounded_fallback() {
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
        DecisionAnchorTransition::ConventionalFallbackReleased,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("retry", "codebase_memory_trace_path"), 3),
        conventional_fallback_graph_denial(),
    );
}

#[test]
fn one_batch_cannot_collapse_implementation_traversal_and_later_sources() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );

    let implementation_call =
        source_call("implementation", DecisionEvidenceKindV1::Implementation);
    let trace_call = call("trace", "codebase_memory_trace_path");
    let caller_call = source_call("caller", DecisionEvidenceKindV1::Caller);
    let behavior_call = source_call("behavior", DecisionEvidenceKindV1::FocusedTest);
    let implementation_admission = EligibleLineageAdmission::new(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::GetCodeSnippet,
        Some(DecisionEvidenceKindV1::Implementation),
    )
    .expect("implementation admission");
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions(
            &[
                implementation_call,
                trace_call,
                caller_call,
                behavior_call,
            ],
            1,
            &[
                Some(LineageAdmissionOutcome::Eligible(implementation_admission)),
                Some(LineageAdmissionOutcome::Ineligible(
                    LineageAdmissionStatus::IncapableSelection,
                )),
                Some(LineageAdmissionOutcome::Ineligible(
                    LineageAdmissionStatus::IncapableSelection,
                )),
                Some(LineageAdmissionOutcome::Ineligible(
                    LineageAdmissionStatus::IncapableSelection,
                )),
            ],
        ),
        [
            None,
            recovery_graph_denial(all_missing(), 4),
            recovery_graph_denial(all_missing(), 4),
            recovery_graph_denial(all_missing(), 4),
        ],
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "implementation",
                "codebase_memory_get_code_snippet",
                &output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionEvidenceKindV1::Implementation,
                ),
            ),
            (
                "trace",
                "codebase_memory_trace_path",
                &output(
                    "codebase_memory_trace_path",
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                ),
            ),
            (
                "caller",
                "codebase_memory_get_code_snippet",
                &output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionEvidenceKindV1::Caller,
                ),
            ),
            (
                "behavior",
                "codebase_memory_get_code_snippet",
                &output_with_evidence(
                    ROOT,
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionEvidenceKindV1::FocusedTest,
                ),
            ),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(state.blocks_mutation("write"));
}

include!("decision_anchor_staged_guidance.rs");

#[test]
fn root_producer_and_same_turn_dependents_stay_ineligible() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 0);
    state.on_tool_dispatched(&call("source-one", "codebase_memory_get_code_snippet"), 0);
    state.on_tool_dispatched(&call("source-two", "codebase_memory_get_code_snippet"), 0);
    let root = output(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    let trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let source_one = output(
        "codebase_memory_get_code_snippet",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let source_two = output(
        "codebase_memory_get_code_snippet",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    state.on_tool_batch_finished(&[
        (
            "source-two",
            "codebase_memory_get_code_snippet",
            &source_two,
        ),
        ("trace", "codebase_memory_trace_path", &trace),
        ("root", "codebase_memory_search_graph", &root),
        (
            "source-one",
            "codebase_memory_get_code_snippet",
            &source_one,
        ),
    ]);
    assert!(
        state.blocks_mutation("write"),
        "the root must be consumed by a later model turn"
    );
}
