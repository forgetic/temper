// Exact-read authority after provider-selected forest promotion.

use super::*;
use crate::{
    EligibleLineageAdmission, EligibleWorkspaceTarget, InvocationTargetAdmission,
    LineageAdmissionOutcome, TargetAdmissionOutcome,
};

const TARGET_A: &str = "00000000-0000-4000-8000-000000000011";
const TARGET_B: &str = "00000000-0000-4000-8000-000000000012";

fn exact_target(value: &str) -> EligibleWorkspaceTarget {
    EligibleWorkspaceTarget::new(value.to_string()).expect("opaque workspace target")
}

fn source_admission(
    root: &str,
    kind: DecisionEvidenceKindV1,
    selects_forest_root: bool,
) -> LineageAdmissionOutcome {
    let admission = EligibleLineageAdmission::new(
        root.to_string(),
        DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::GetCodeSnippet,
        Some(kind),
    )
    .expect("closed source admission");
    LineageAdmissionOutcome::Eligible(if selects_forest_root {
        admission.with_forest_root_selection()
    } else {
        admission
    })
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

fn install_parallel_roots(state: &mut DecisionAnchorState) {
    for id in ["implementation-root", "focused-root"] {
        state.on_tool_dispatched(&call(id, "codebase_memory_search_graph"), 0);
    }
    let implementation = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    let focused = output_with_focused_test_discovery(
        "codebase_memory_search_graph",
        OTHER_ROOT,
        DecisionAnchorLineageStageV1::Root,
        FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned,
    );
    state.on_tool_batch_finished(&[
        (
            "implementation-root",
            "codebase_memory_search_graph",
            &implementation,
        ),
        (
            "focused-root",
            "codebase_memory_search_graph",
            &focused,
        ),
    ]);
}

#[test]
fn promoted_forest_root_preserves_parallel_recovery_exact_read_authority() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_parallel_roots(&mut state);

    for (turn, id, selects_forest_root, transition) in [
        (1, "sibling-focused", true, DecisionAnchorTransition::Unchanged),
        (
            2,
            "sibling-focused-repeat",
            false,
            DecisionAnchorTransition::GapRecoveryNeeded,
        ),
    ] {
        let focused = source_call(id, DecisionEvidenceKindV1::FocusedTest);
        assert_eq!(
            state.on_tool_dispatched_with_admission(
                &focused,
                turn,
                Some(&source_admission(
                    OTHER_ROOT,
                    DecisionEvidenceKindV1::FocusedTest,
                    selects_forest_root,
                )),
            ),
            None,
        );
        assert_eq!(
            finish_with_evidence(
                &mut state,
                id,
                OTHER_ROOT,
                DecisionEvidenceKindV1::FocusedTest,
            ),
            transition,
        );
    }

    let trace = call("routing-trace", "codebase_memory_trace_path");
    assert_eq!(
        state.on_tool_dispatched_with_admission(&trace, 3, Some(&trace_admission(ROOT))),
        None,
    );
    assert_eq!(
        state.on_tool_finished(
            "routing-trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );

    let kinds = [
        DecisionEvidenceKindV1::Implementation,
        DecisionEvidenceKindV1::Caller,
        DecisionEvidenceKindV1::FocusedTest,
    ];
    let ids = ["implementation", "caller", "focused"];
    let calls = kinds
        .iter()
        .zip(ids)
        .map(|(kind, id)| source_call(id, *kind))
        .collect::<Vec<_>>();
    let admissions = kinds
        .iter()
        .map(|kind| Some(source_admission(ROOT, *kind, false)))
        .collect::<Vec<_>>();
    assert_eq!(
        state.on_tool_batch_dispatched_with_admissions(&calls, 4, &admissions),
        [None, None, None],
    );

    let outputs = kinds
        .iter()
        .map(|kind| {
            output_with_evidence(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                *kind,
            )
        })
        .collect::<Vec<_>>();
    let source_target = TargetAdmissionOutcome::Eligible(exact_target(TARGET_A));
    assert_eq!(
        state.on_tool_batch_finished_with_targets(&[
            (
                ids[0],
                "codebase_memory_get_code_snippet",
                &outputs[0],
                Some(&source_target),
                true,
            ),
            (
                ids[1],
                "codebase_memory_get_code_snippet",
                &outputs[1],
                None,
                true,
            ),
            (
                ids[2],
                "codebase_memory_get_code_snippet",
                &outputs[2],
                None,
                true,
            ),
        ]),
        DecisionAnchorTransition::EnabledEvidenceComplete,
    );

    let mutation_target = InvocationTargetAdmission::Mutation(vec![
        TargetAdmissionOutcome::Eligible(exact_target(TARGET_A)),
    ]);
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("mutation-before-read", "write"),
            5,
            Some(&mutation_target),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );

    let read_target = InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(
        exact_target(TARGET_A),
    ));
    let read = call("post-source-read", "read");
    assert_eq!(
        state.on_tool_dispatched_with_targets(&read, 6, Some(&read_target)),
        None,
    );
    state.on_tool_finished("post-source-read", "read", &plain_success());
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("matching-mutation", "write"),
            7,
            Some(&mutation_target),
        ),
        None,
    );
    assert_eq!(
        state.on_tool_dispatched_with_targets(
            &call("cross-target-mutation", "write"),
            8,
            Some(&InvocationTargetAdmission::Mutation(vec![
                TargetAdmissionOutcome::Eligible(exact_target(TARGET_B)),
            ])),
        ),
        Some(ToolCallDenial::DecisionAnchorMutation),
    );
}
