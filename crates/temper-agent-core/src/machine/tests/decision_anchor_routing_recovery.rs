// Routing benchmark ordering regression for decision-evidence convergence.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionStatus};

mod focused_test_source {
    include!("decision_anchor_focused_test_source.rs");
}

#[test]
fn cross_root_focused_test_cannot_complete_the_staged_implementation_root() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    for (id, root) in [("routing-root", ROOT), ("test-root", OTHER_ROOT)] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
        finish(
            &mut state,
            id,
            "codebase_memory_search_graph",
            root,
            DecisionAnchorLineageStageV1::Root,
        );
    }
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
    state.on_tool_dispatched(&call("caller-trace", "codebase_memory_trace_path"), 2);
    finish(
        &mut state,
        "caller-trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    state.on_tool_dispatched(
        &source_call("cross-test", DecisionEvidenceKindV1::FocusedTest),
        3,
    );
    finish_with_evidence(
        &mut state,
        "cross-test",
        OTHER_ROOT,
        DecisionEvidenceKindV1::FocusedTest,
    );
    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 4);
    assert_eq!(
        finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(state.blocks_mutation("write"));

    state.on_tool_dispatched(
        &source_call("routing-test", DecisionEvidenceKindV1::FocusedTest),
        5,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "routing-test",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::Converged,
    );
    assert_eq!(
        state.on_tool_dispatched(&call("mutation", "write"), 6),
        Some(ToolCallDenial::DecisionAnchorMutation)
    );
}

#[test]
fn focused_test_recovery_traverses_from_consumed_caller_then_reads_returned_test() {
    let mut state = focused_test_recovery_state();

    let details = state.recovery_details().expect("focused-test recovery menu");
    assert_eq!(details.missing_evidence, [GraphRecoveryEvidenceKindV1::FocusedTest]);
    assert!(
        details
            .compatible_actions
            .contains(&GraphRecoveryActionV1::focused_test_traversal())
    );

    let speculative = source_call("speculative", DecisionEvidenceKindV1::FocusedTest);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &speculative,
            6,
            Some(&LineageAdmissionOutcome::Ineligible(
                LineageAdmissionStatus::IncapableSelection,
            )),
        ),
        recovery_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest], 4),
    );

    let traversal = focused_test_traversal_call("caller-to-test");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &traversal,
            7,
            Some(&LineageAdmissionOutcome::Eligible(traversal_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "caller-to-test",
            "codebase_memory_trace_path",
            &output_with_focused_test_discovery(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let after_traversal = state.recovery_details().expect("exact test read remains");
    assert_eq!(after_traversal.remaining_allowance, 3);
    assert_eq!(
        after_traversal.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::FocusedTest,
        )]
    );

    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &source_call("exact-test", DecisionEvidenceKindV1::FocusedTest),
            8,
            Some(&LineageAdmissionOutcome::Eligible(exact_test_admission())),
        ),
        None,
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
}

#[test]
fn empty_traversal_enables_one_semantic_fallback_then_one_exact_test_source() {
    let mut state = focused_test_recovery_state();
    let traversal = focused_test_traversal_call("empty-traversal");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &traversal,
            6,
            Some(&LineageAdmissionOutcome::Eligible(traversal_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "empty-traversal",
            "codebase_memory_trace_path",
            &output_with_focused_test_discovery(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let details = state.recovery_details().expect("semantic fallback menu");
    assert_eq!(details.remaining_allowance, 3);
    assert_eq!(
        details.compatible_actions,
        [GraphRecoveryActionV1::focused_test_semantic_fallback()]
    );

    let fallback = semantic_fallback_call("semantic-fallback");
    let duplicate = semantic_fallback_call("duplicate-fallback");
    let admissions = [
        Some(LineageAdmissionOutcome::Eligible(fallback_admission())),
        Some(LineageAdmissionOutcome::Eligible(fallback_admission())),
    ];
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions(
            &[fallback, duplicate],
            7,
            &admissions,
        ),
        [
            None,
            recovery_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest], 3),
        ],
        "only one fallback is admitted from the immutable snapshot",
    );
    assert_eq!(
        state.on_tool_finished(
            "semantic-fallback",
            "codebase_memory_search_graph",
            &output_with_focused_test_discovery(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let details = state.recovery_details().expect("exact fallback test read");
    assert_eq!(details.remaining_allowance, 2);
    assert_eq!(
        details.compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::FocusedTest,
        )]
    );

    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &source_call("fallback-test", DecisionEvidenceKindV1::FocusedTest),
            8,
            Some(&LineageAdmissionOutcome::Eligible(exact_test_admission())),
        ),
        None,
    );
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "fallback-test",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
        ),
        DecisionAnchorTransition::Converged,
    );
    assert_eq!(
        state.on_tool_dispatched(&call("mutation", "write"), 9),
        Some(ToolCallDenial::DecisionAnchorMutation)
    );
}

#[test]
fn empty_traversal_and_empty_fallback_exhaust_without_retry_path() {
    let mut state = focused_test_recovery_state();
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &focused_test_traversal_call("empty-traversal"),
            6,
            Some(&LineageAdmissionOutcome::Eligible(traversal_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "empty-traversal",
            "codebase_memory_trace_path",
            &output_with_focused_test_discovery(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &semantic_fallback_call("empty-fallback"),
            7,
            Some(&LineageAdmissionOutcome::Eligible(fallback_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "empty-fallback",
            "codebase_memory_search_graph",
            &output_with_focused_test_discovery(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
            ),
        ),
        DecisionAnchorTransition::RecoveryExhausted,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&semantic_fallback_call("retry"), 8),
        exhausted_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest]),
    );
}

#[test]
fn fallback_with_typed_targets_but_no_exact_selector_exhausts_without_escape() {
    let mut state = focused_test_recovery_state();
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &focused_test_traversal_call("empty-traversal"),
            6,
            Some(&LineageAdmissionOutcome::Eligible(traversal_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "empty-traversal",
            "codebase_memory_trace_path",
            &output_with_focused_test_discovery(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &semantic_fallback_call("malformed-fallback"),
            7,
            Some(&LineageAdmissionOutcome::Eligible(fallback_admission())),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "malformed-fallback",
            "codebase_memory_search_graph",
            &output_with_kinds(
                "codebase_memory_search_graph",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                &[
                    DecisionAnchorTargetKindV1::Pattern,
                    DecisionAnchorTargetKindV1::FunctionName,
                    DecisionAnchorTargetKindV1::QualifiedName,
                ],
            ),
        ),
        DecisionAnchorTransition::RecoveryExhausted,
    );
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&semantic_fallback_call("independent-retry"), 8),
        exhausted_graph_denial([GraphRecoveryEvidenceKindV1::FocusedTest]),
    );
}

#[test]
fn multi_root_over_returned_candidates_recover_on_the_selected_root() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("routing-root", "codebase_memory_search_graph"), 0);
    state.on_tool_dispatched(&call("behavior-root", "codebase_memory_search_graph"), 0);
    let routing = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    let behavior = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            ("routing-root", "codebase_memory_search_graph", &routing),
            ("behavior-root", "codebase_memory_search_graph", &behavior),
        ]),
        DecisionAnchorTransition::Unchanged,
    );

    for (turn, id, expected) in [
        (1, "sibling-source-one", DecisionAnchorTransition::Unchanged),
        (
            2,
            "sibling-source-two",
            DecisionAnchorTransition::GapRecoveryNeeded,
        ),
    ] {
        state.on_tool_dispatched(
            &source_call(id, DecisionEvidenceKindV1::FocusedTest),
            turn,
        );
        assert_eq!(
            finish_with_evidence(
                &mut state,
                id,
                OTHER_ROOT,
                DecisionEvidenceKindV1::FocusedTest,
            ),
            expected,
        );
    }
    assert!(state.blocks_mutation("write"));
    assert_eq!(
        state.recovery_details().unwrap().compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Trace,
        )],
    );

    state.on_tool_dispatched(&call("selected-trace", "codebase_memory_trace_path"), 3);
    assert_eq!(
        state.on_tool_finished(
            "selected-trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );

    for (id, kind) in [
        ("implementation", DecisionEvidenceKindV1::Implementation),
        ("caller", DecisionEvidenceKindV1::Caller),
        ("focused-test", DecisionEvidenceKindV1::FocusedTest),
    ] {
        assert_eq!(
            state.on_tool_dispatched(&source_call(id, kind), 4),
            None,
            "{id} must be admitted from the immutable post-trace snapshot",
        );
    }
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
    let focused_test = output_with_evidence(
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "implementation",
                "codebase_memory_get_code_snippet",
                &implementation,
            ),
            ("caller", "codebase_memory_get_code_snippet", &caller),
            (
                "focused-test",
                "codebase_memory_get_code_snippet",
                &focused_test,
            ),
        ]),
        DecisionAnchorTransition::Converged,
    );
    assert!(state.blocks_mutation("write"));
}

fn focused_test_recovery_state() -> DecisionAnchorState {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    state.on_tool_finished(
        "root",
        "codebase_memory_search_graph",
        &output_with_focused_test_discovery(
            "codebase_memory_search_graph",
            ROOT,
            DecisionAnchorLineageStageV1::Root,
            FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
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
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 2);
    finish(
        &mut state,
        "trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 3);
    finish_with_evidence(
        &mut state,
        "caller",
        ROOT,
        DecisionEvidenceKindV1::Caller,
    );
    for (turn, id, expected) in [
        (4, "broad-one", DecisionAnchorTransition::Unchanged),
        (5, "broad-two", DecisionAnchorTransition::GapRecoveryNeeded),
    ] {
        state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        assert_eq!(
            state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success()),
            expected,
        );
    }
    state
}

fn focused_test_traversal_call(id: &str) -> ToolCall {
    let mut traversal = call(id, "codebase_memory_trace_path");
    traversal.arguments = serde_json::json!({
        "function_name": "provider-returned-caller",
        "mode": "calls",
        "direction": "inbound",
        "include_tests": true,
    });
    traversal
}

fn semantic_fallback_call(id: &str) -> ToolCall {
    let mut fallback = call(id, "codebase_memory_search_graph");
    fallback.arguments = serde_json::json!({
        "query": "behavioral request affinity regression",
    });
    fallback
}

fn traversal_admission() -> EligibleLineageAdmission {
    EligibleLineageAdmission::focused_test_traversal(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::FunctionName,
    )
    .expect("closed focused-test traversal admission")
}

fn fallback_admission() -> EligibleLineageAdmission {
    EligibleLineageAdmission::focused_test_semantic_fallback(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::GraphQuery,
    )
    .expect("closed semantic fallback admission")
}

fn exact_test_admission() -> EligibleLineageAdmission {
    EligibleLineageAdmission::new(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::GetCodeSnippet,
        Some(DecisionEvidenceKindV1::FocusedTest),
    )
    .expect("returned focused-test admission")
}
