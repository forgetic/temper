//! Diagnostics for mutations the existing authorization gate already denied.

use crate::{InvocationTargetAdmission, TargetAdmissionOutcome, TargetAdmissionStatus};

use super::ToolCallDenial;

pub(super) fn blocked_mutation_denial(
    admission: Option<&InvocationTargetAdmission>,
) -> ToolCallDenial {
    let outcomes = match admission {
        Some(InvocationTargetAdmission::Ineligible(status)) => return status_denial(*status),
        Some(InvocationTargetAdmission::Mutation(outcomes))
        | Some(InvocationTargetAdmission::PatchCreation {
            existing: outcomes, ..
        }) => outcomes,
        _ => return ToolCallDenial::DecisionAnchorMutation,
    };
    // Keep the result deterministic for a batch with multiple invalid targets.
    // Unknown targets retain read-first guidance; they may simply be unread.
    for status in [
        TargetAdmissionStatus::MalformedTarget,
        TargetAdmissionStatus::CompetingTargets,
    ] {
        if outcomes.iter().any(|outcome| {
            matches!(outcome, TargetAdmissionOutcome::Ineligible(value) if *value == status)
        }) {
            return status_denial(status);
        }
    }
    ToolCallDenial::DecisionAnchorMutation
}

fn status_denial(status: TargetAdmissionStatus) -> ToolCallDenial {
    match status {
        TargetAdmissionStatus::MalformedTarget => ToolCallDenial::MalformedMutationTarget,
        TargetAdmissionStatus::CompetingTargets => ToolCallDenial::ConflictingMutationTargets,
        _ => ToolCallDenial::DecisionAnchorMutation,
    }
}
