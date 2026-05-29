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
        GateCondition::DependenciesResolved => artifact
            .relations
            .iter()
            .filter(|relation| relation.kind == RelationKind::Dependency)
            .all(|relation| signals.dependencies().is_landed(relation.target)),
        GateCondition::CiPassed => signals.ci().is_passed(),
        GateCondition::CiFailed => signals.ci().is_failed(),
        GateCondition::ReviewApproved => signals.review().is_approved(),
        GateCondition::ReviewChangesRequested => signals.review().has_changes_requested(),
    }
}
