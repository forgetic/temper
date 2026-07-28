use super::signals::GateSignals;
use crate::classify::ClassifiedArtifact;
use crate::relation::RelationKind;
use crate::validated::GateCondition;
use std::collections::HashSet;

pub(super) fn gate_condition_satisfied(
    condition: &GateCondition,
    artifact: &ClassifiedArtifact,
    labels: &HashSet<&str>,
    signals: &GateSignals,
) -> bool {
    match condition {
        GateCondition::LabelPresent(label) => labels.contains(label.as_str()),
        GateCondition::StateEquals { dimension, state } => artifact
            .states
            .get(dimension)
            .is_some_and(|states| states.contains(state)),
        GateCondition::DependenciesResolved => {
            let blocked = artifact
                .states
                .values()
                .flatten()
                .any(|state| state.as_str() == "blocked");
            let mut dependencies = artifact
                .relations
                .iter()
                .filter(|relation| relation.kind == RelationKind::Dependency)
                .peekable();
            // A blocked dependency gate is a fail-closed lifecycle boundary,
            // not a vacuous predicate. Missing relation projection must never
            // make blocked work eligible for queue automation or a direct
            // unblock. Other dependency gates can be optional (for example, a
            // root implementation PR has no predecessor and may land).
            (!blocked || dependencies.peek().is_some())
                && dependencies.all(|relation| signals.dependencies().is_landed(&relation.target))
        }
        GateCondition::CiPassed => signals.ci().is_passed(),
        GateCondition::CiFailed => signals.ci().is_failed(),
        GateCondition::CiRecoveryRequired => signals.ci().is_recovery_required(),
        GateCondition::ReviewApproved => signals.review().is_approved(),
        GateCondition::ReviewChangesRequested => signals.review().has_changes_requested(),
        GateCondition::ExactHeadValidation => signals.exact_head_validation(),
    }
}
