// Routing benchmark ordering regression for decision-evidence convergence.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome, LineageAdmissionStatus};

#[test]
fn caller_source_after_cross_root_focused_test_keeps_active_root_incomplete() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("routing-root", "codebase_memory_search_graph"), 0);
    state.on_tool_dispatched(&call("test-root", "codebase_memory_search_graph"), 0);
    let routing_root = output(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    let test_root = output(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "routing-root",
                "codebase_memory_search_graph",
                &routing_root,
            ),
            ("test-root", "codebase_memory_search_graph", &test_root),
        ]),
        DecisionAnchorTransition::Unchanged,
    );

    state.on_tool_dispatched(&call("implementation", "codebase_memory_search_code"), 1);
    state.on_tool_dispatched(&call("caller-trace", "codebase_memory_trace_path"), 1);
    state.on_tool_dispatched(
        &source_call("focused-test", DecisionEvidenceKindV1::FocusedTest),
        1,
    );
    let implementation = output(
        "codebase_memory_search_code",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let caller_trace = output(
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    let focused_test = output_with_evidence(
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionEvidenceKindV1::FocusedTest,
    );
    assert_eq!(
        state.on_tool_batch_finished(&[
            (
                "implementation",
                "codebase_memory_search_code",
                &implementation,
            ),
            (
                "caller-trace",
                "codebase_memory_trace_path",
                &caller_trace,
            ),
            (
                "focused-test",
                "codebase_memory_get_code_snippet",
                &focused_test,
            ),
        ]),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(state.blocks_mutation("write"));

    state.on_tool_dispatched(&call("duplicate", "codebase_memory_search_code"), 2);
    assert_eq!(
        state.on_tool_finished(
            "duplicate",
            "codebase_memory_search_code",
            &implementation,
        ),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(state.blocks_mutation("write"));

    state.on_tool_dispatched(&source_call("caller", DecisionEvidenceKindV1::Caller), 3);
    assert_eq!(
        finish_with_evidence(&mut state, "caller", ROOT, DecisionEvidenceKindV1::Caller),
        DecisionAnchorTransition::Unchanged,
    );
    assert!(
        state.blocks_mutation("write"),
        "the sibling root's focused-test result cannot complete routing-root evidence"
    );

    state.on_tool_dispatched(
        &source_call("routing-test", DecisionEvidenceKindV1::FocusedTest),
        4,
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
    assert!(!state.blocks_mutation("write"));
    assert_eq!(
        state.on_tool_dispatched(&call("closed", "codebase_memory_search_graph"), 5),
        completed_graph_denial(),
    );
    assert_eq!(state.on_tool_dispatched(&call("mutation", "write"), 5), None);
}

#[test]
fn focused_test_recovery_traverses_from_consumed_caller_then_reads_returned_test() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    state.on_tool_dispatched(&call("root", "codebase_memory_search_graph"), 0);
    finish(
        &mut state,
        "root",
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
    );
    state.on_tool_dispatched(&call("trace", "codebase_memory_trace_path"), 1);
    finish(
        &mut state,
        "trace",
        "codebase_memory_trace_path",
        ROOT,
        DecisionAnchorLineageStageV1::CarryForward,
    );
    for (turn, id, kind) in [
        (2, "implementation", DecisionEvidenceKindV1::Implementation),
        (3, "caller", DecisionEvidenceKindV1::Caller),
    ] {
        state.on_tool_dispatched(&source_call(id, kind), turn);
        finish_with_evidence(&mut state, id, ROOT, kind);
    }
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

    let details = state.recovery_details().expect("focused-test recovery menu");
    assert_eq!(details.missing_evidence, [GraphRecoveryEvidenceKindV1::FocusedTest]);
    assert!(
        details
            .compatible_actions
            .contains(&GraphRecoveryActionV1::focused_test_traversal())
    );
    assert!(details.compatible_actions.contains(&GraphRecoveryActionV1::for_evidence(
        GraphRecoveryEvidenceKindV1::FocusedTest,
    )));

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

    let mut traversal = call("caller-to-test", "codebase_memory_trace_path");
    traversal.arguments = serde_json::json!({
        "function_name": "provider-returned-caller",
        "mode": "calls",
        "direction": "inbound",
        "include_tests": true,
    });
    let traversal_admission = EligibleLineageAdmission::focused_test_traversal(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::FunctionName,
    )
    .expect("closed focused-test traversal admission");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &traversal,
            7,
            Some(&LineageAdmissionOutcome::Eligible(traversal_admission)),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "caller-to-test",
            "codebase_memory_trace_path",
            &output(
                "codebase_memory_trace_path",
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    let after_traversal = state.recovery_details().expect("exact test read remains");
    assert!(!after_traversal
        .compatible_actions
        .contains(&GraphRecoveryActionV1::focused_test_traversal()));

    let test_admission = EligibleLineageAdmission::new(
        ROOT.to_string(),
        DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::GetCodeSnippet,
        Some(DecisionEvidenceKindV1::FocusedTest),
    )
    .expect("returned focused-test admission");
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &source_call("exact-test", DecisionEvidenceKindV1::FocusedTest),
            8,
            Some(&LineageAdmissionOutcome::Eligible(test_admission)),
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
    assert!(!state.blocks_mutation("write"));
}
