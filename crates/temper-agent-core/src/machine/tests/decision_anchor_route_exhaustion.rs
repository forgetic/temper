// Route-specific forest exhaustion and final authority regression.

use super::*;
use crate::{
    EligibleLineageAdmission, EligibleWorkspaceTarget, InvocationTargetAdmission,
    LineageAdmissionOutcome, TargetAdmissionOutcome,
};

const ROUTE_TARGET: &str = "00000000-0000-4000-8000-000000000021";
const UNRELATED_TARGET: &str = "00000000-0000-4000-8000-000000000022";

mod reported_empty_callers {
    include!("decision_anchor_reported_empty_callers.rs");
}

fn source_admission(root: &str, kind: DecisionEvidenceKindV1) -> LineageAdmissionOutcome {
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::new(
            root.to_string(),
            DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(kind),
        )
        .expect("closed source admission"),
    )
}

fn trace_admission(root: &str) -> LineageAdmissionOutcome {
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::implementation_caller_traversal(
            root.to_string(),
            DecisionAnchorTargetKindV1::FunctionName,
        )
        .expect("closed trace admission"),
    )
}

fn workspace_target(value: &str) -> EligibleWorkspaceTarget {
    EligibleWorkspaceTarget::new(value.to_string()).expect("opaque workspace target")
}

fn recover_source(
    state: &mut DecisionAnchorState,
    id: &str,
    root: &str,
    kind: DecisionEvidenceKindV1,
    turn: usize,
    source_target: Option<&TargetAdmissionOutcome>,
) -> DecisionAnchorTransition {
    let source = source_call(id, kind);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &source,
            turn,
            Some(&source_admission(root, kind)),
        ),
        None,
    );
    state.on_tool_finished_with_source_target(
        id,
        "codebase_memory_get_code_snippet",
        &output_with_evidence(root, DecisionAnchorLineageStageV1::CarryForward, kind),
        source_target,
    )
}

fn recover_trace(
    state: &mut DecisionAnchorState,
    id: &str,
    root: &str,
    turn: usize,
    outcome: CallerDiscoveryOutcomeV1,
) -> DecisionAnchorTransition {
    let trace = call(id, "codebase_memory_trace_path");
    assert_eq!(
        state.on_tool_dispatched_with_admission(&trace, turn, Some(&trace_admission(root))),
        None,
    );
    state.on_tool_finished(
        id,
        "codebase_memory_trace_path",
        &output_with_caller_discovery(root, DecisionAnchorLineageStageV1::CarryForward, outcome),
    )
}

fn enter_two_root_recovery(state: &mut DecisionAnchorState) {
    for id in ["implementation-root", "focused-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let root = |binding| {
        output_with_focused_test_discovery(
            "codebase_memory_search_graph",
            binding,
            DecisionAnchorLineageStageV1::Root,
            FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
        )
    };
    let roots = [root(ROOT), root(OTHER_ROOT)];
    state.on_tool_batch_finished(&[
        ("implementation-root", "codebase_memory_search_graph", &roots[0]),
        ("focused-root", "codebase_memory_search_graph", &roots[1]),
    ]);
    for (turn, id) in [(1, "broad-one"), (2, "broad-two")] {
        state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success());
    }
}

fn exhaust_first_implementation_route(state: &mut DecisionAnchorState) {
    assert_eq!(
        recover_source(state, "first-implementation", ROOT, DecisionEvidenceKindV1::Implementation, 3, None),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        recover_trace(state, "first-empty-trace", ROOT, 4, CallerDiscoveryOutcomeV1::NoEligibleSelector),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
}

fn complete_second_implementation_route(
    state: &mut DecisionAnchorState,
    route_target: &TargetAdmissionOutcome,
) {
    assert_eq!(
        recover_source(state, "second-implementation", OTHER_ROOT, DecisionEvidenceKindV1::Implementation, 5, Some(route_target)),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        recover_trace(state, "second-trace", OTHER_ROOT, 6, CallerDiscoveryOutcomeV1::EligibleSelectorReturned),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        recover_source(state, "second-caller", OTHER_ROOT, DecisionEvidenceKindV1::Caller, 7, None),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
}

fn assert_post_read_authority(
    state: &mut DecisionAnchorState,
    route_target: TargetAdmissionOutcome,
) {
    let matching = InvocationTargetAdmission::Mutation(vec![route_target.clone()]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("before-read", "write"), 9, Some(&matching)),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
    let read = call("exact-route-read", "read");
    let read_target = InvocationTargetAdmission::Read(route_target);
    assert_eq!(state.on_tool_dispatched_with_targets(&read, 10, Some(&read_target)), None);
    state.on_tool_finished("exact-route-read", "read", &plain_success());
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("matching-write", "write"), 11, Some(&matching)),
        None,
    );
    let unrelated = InvocationTargetAdmission::Mutation(vec![TargetAdmissionOutcome::Eligible(
        workspace_target(UNRELATED_TARGET),
    )]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(&call("unrelated-write", "write"), 12, Some(&unrelated)),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
}

fn assert_exposed_action(
    state: &DecisionAnchorState,
    root: &str,
    kind: GraphRecoveryEvidenceKindV1,
) {
    assert_eq!(
        state.active_recovery_action(),
        Some((root.to_string(), GraphRecoveryActionV1::for_evidence(kind)))
    );
}

fn state_awaiting_final_focused_action() -> DecisionAnchorState {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    enter_two_root_recovery(&mut state);
    assert_exposed_action(&state, ROOT, GraphRecoveryEvidenceKindV1::Implementation);
    exhaust_first_implementation_route(&mut state);

    assert_exposed_action(&state, OTHER_ROOT, GraphRecoveryEvidenceKindV1::Implementation);
    recover_source(
        &mut state,
        "second-implementation",
        OTHER_ROOT,
        DecisionEvidenceKindV1::Implementation,
        5,
        None,
    );
    assert_exposed_action(&state, OTHER_ROOT, GraphRecoveryEvidenceKindV1::Trace);
    recover_trace(
        &mut state,
        "second-trace",
        OTHER_ROOT,
        6,
        CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    assert_exposed_action(&state, OTHER_ROOT, GraphRecoveryEvidenceKindV1::Caller);
    recover_source(
        &mut state,
        "second-caller",
        OTHER_ROOT,
        DecisionEvidenceKindV1::Caller,
        7,
        None,
    );
    assert_exposed_action(&state, ROOT, GraphRecoveryEvidenceKindV1::FocusedTest);
    state
}

#[test]
fn implementation_selection_reserves_the_only_independent_focused_route() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    for id in ["focused-capable-root", "implementation-only-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let focused_capable = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    let implementation_only = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::NoEligibleSelector,
    );
    state.on_tool_batch_finished(&[
        ("focused-capable-root", "codebase_memory_search_graph", &focused_capable),
        (
            "implementation-only-root",
            "codebase_memory_search_graph",
            &implementation_only,
        ),
    ]);
    for (turn, id) in [(1, "broad-one"), (2, "broad-two")] {
        state.on_tool_dispatched(&call(id, "codebase_memory_get_architecture"), turn);
        state.on_tool_finished(id, "codebase_memory_get_architecture", &plain_success());
    }

    assert_exposed_action(
        &state,
        OTHER_ROOT,
        GraphRecoveryEvidenceKindV1::Implementation,
    );
    recover_source(
        &mut state,
        "implementation",
        OTHER_ROOT,
        DecisionEvidenceKindV1::Implementation,
        3,
        None,
    );
    recover_trace(
        &mut state,
        "trace",
        OTHER_ROOT,
        4,
        CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    recover_source(
        &mut state,
        "caller",
        OTHER_ROOT,
        DecisionEvidenceKindV1::Caller,
        5,
        None,
    );
    assert_exposed_action(&state, ROOT, GraphRecoveryEvidenceKindV1::FocusedTest);
    assert_eq!(
        recover_source(
            &mut state,
            "focused-test",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
            6,
            None,
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );
}

#[test]
fn every_exposed_recovery_action_advances_to_cross_root_completion() {
    let mut state = state_awaiting_final_focused_action();
    assert_eq!(
        recover_source(
            &mut state,
            "final-focused",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
            8,
            None,
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );
}

#[test]
fn final_in_flight_allowance_exposes_no_unreachable_action() {
    let mut state = state_awaiting_final_focused_action();
    let focused = source_call("final-focused", DecisionEvidenceKindV1::FocusedTest);
    assert_eq!(
        state.on_tool_dispatched_with_admission(
            &focused,
            8,
            Some(&source_admission(ROOT, DecisionEvidenceKindV1::FocusedTest)),
        ),
        None,
    );
    assert_eq!(state.active_recovery_action(), None);
    assert_eq!(
        state.on_tool_finished(
            "final-focused",
            "codebase_memory_get_code_snippet",
            &output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                DecisionEvidenceKindV1::FocusedTest,
            ),
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );
}

#[test]
fn exhausted_implementation_root_retains_reachable_focused_route_and_exact_authority() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    enter_two_root_recovery(&mut state);
    exhaust_first_implementation_route(&mut state);
    let route_target = TargetAdmissionOutcome::Eligible(workspace_target(ROUTE_TARGET));
    complete_second_implementation_route(&mut state, &route_target);

    let focused = GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::FocusedTest);
    let recovery = state.recovery_details().expect("reachable focused-test route");
    assert_eq!(recovery.missing_evidence, [GraphRecoveryEvidenceKindV1::FocusedTest]);
    assert_eq!(recovery.compatible_actions, [focused]);
    assert_eq!(recovery.remaining_allowance, 1);
    assert_eq!(state.active_recovery_action(), Some((ROOT.to_string(), focused)));
    assert_eq!(
        recover_source(
            &mut state,
            "first-focused",
            ROOT,
            DecisionEvidenceKindV1::FocusedTest,
            8,
            None,
        ),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );
    assert_post_read_authority(&mut state, route_target);
}
