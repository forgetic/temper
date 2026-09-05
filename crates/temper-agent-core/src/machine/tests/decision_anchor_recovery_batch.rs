// Root-local, immutable recovery-batch regressions.

use super::*;
use crate::{EligibleLineageAdmission, LineageAdmissionOutcome};

fn eligible_admission(
    root: &str,
    tool: GraphCorrelationToolV1,
    evidence: Option<DecisionEvidenceKindV1>,
) -> LineageAdmissionOutcome {
    let selector = match tool {
        GraphCorrelationToolV1::TracePath => DecisionAnchorTargetKindV1::FunctionName,
        GraphCorrelationToolV1::GetCodeSnippet => DecisionAnchorTargetKindV1::QualifiedName,
        GraphCorrelationToolV1::SearchCode => DecisionAnchorTargetKindV1::Pattern,
        GraphCorrelationToolV1::SearchGraph => DecisionAnchorTargetKindV1::GraphQuery,
    };
    LineageAdmissionOutcome::Eligible(
        EligibleLineageAdmission::new(root.to_string(), selector, tool, evidence)
            .expect("capable closed admission"),
    )
}

#[test]
fn immutable_recovery_snapshots_bound_denied_and_speculative_siblings() {
    let mut state = DecisionAnchorState::from_effects(&effects()).unwrap();
    install_consumable_root(&mut state);
    enter_budget_recovery(&mut state, 1);

    let calls = [
        source_call("cross-caller", DecisionEvidenceKindV1::Caller),
        source_call("implementation", DecisionEvidenceKindV1::Implementation),
        source_call("duplicate", DecisionEvidenceKindV1::Implementation),
    ];
    let denials = state.on_tool_batch_dispatched_with_admissions(
        &calls,
        3,
        &[
            Some(eligible_admission(
                OTHER_ROOT,
                GraphCorrelationToolV1::GetCodeSnippet,
                Some(DecisionEvidenceKindV1::Caller),
            )),
            Some(eligible_admission(
                ROOT,
                GraphCorrelationToolV1::GetCodeSnippet,
                Some(DecisionEvidenceKindV1::Implementation),
            )),
            Some(eligible_admission(
                ROOT,
                GraphCorrelationToolV1::GetCodeSnippet,
                Some(DecisionEvidenceKindV1::Implementation),
            )),
        ],
    );
    assert!(denials[0].is_some());
    assert_eq!(denials[1], None);
    assert!(denials[2].is_some());
    assert_eq!(
        finish_with_evidence(
            &mut state,
            "implementation",
            ROOT,
            DecisionEvidenceKindV1::Implementation,
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.recovery_details().unwrap().compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Trace,
        )],
    );

    let calls = [
        call("trace", "codebase_memory_trace_path"),
        source_call("same-batch-caller", DecisionEvidenceKindV1::Caller),
    ];
    let denials = state.on_tool_batch_dispatched_with_admissions(
        &calls,
        4,
        &[
            Some(eligible_admission(
                ROOT,
                GraphCorrelationToolV1::TracePath,
                None,
            )),
            Some(eligible_admission(
                ROOT,
                GraphCorrelationToolV1::GetCodeSnippet,
                Some(DecisionEvidenceKindV1::Caller),
            )),
        ],
    );
    assert_eq!(denials[0], None);
    assert!(denials[1].is_some());
    assert_eq!(
        state.on_tool_finished(
            "trace",
            "codebase_memory_trace_path",
            &output_with_caller_discovery(
                ROOT,
                DecisionAnchorLineageStageV1::CarryForward,
                CallerDiscoveryOutcomeV1::EligibleSelectorReturned,
            ),
        ),
        DecisionAnchorTransition::GapRecoveryNeeded,
    );
    assert_eq!(
        state.recovery_details().unwrap().compatible_actions,
        [GraphRecoveryActionV1::for_evidence(
            GraphRecoveryEvidenceKindV1::Caller,
        )],
        "denied siblings in a progressing batch do not consume its remaining recovery slots",
    );
    assert_eq!(
        state.recovery_details().unwrap().remaining_allowance,
        2,
    );
}
