//! Exact provider-selector promotion within the retained decision forest.

use crate::LineageAdmissionOutcome;

use super::*;

impl DecisionAnchorState {
    pub(super) fn promote_unique_forest_root(
        &mut self,
        admissions: &[Option<LineageAdmissionOutcome>],
    ) -> bool {
        let selections = admissions
            .iter()
            .filter_map(|outcome| match outcome {
                Some(LineageAdmissionOutcome::Eligible(admission))
                    if admission.selects_forest_root() =>
                {
                    Some(admission)
                }
                Some(LineageAdmissionOutcome::Eligible(_))
                | Some(LineageAdmissionOutcome::Ineligible(_))
                | None => None,
            })
            .filter_map(|admission| {
                Some((
                    self.root_matching_admission(admission)?,
                    DecisionGap::recovery_action_for_admission(admission)?,
                ))
            })
            .collect::<Vec<_>>();
        let [(root, action)] = selections.as_slice() else {
            return selections.is_empty();
        };
        self.promote_forest_root(root, [*action])
    }

    fn promote_forest_root(
        &mut self,
        root: &str,
        actions: impl IntoIterator<Item = GraphRecoveryActionV1>,
    ) -> bool {
        let actions = actions.into_iter().collect::<BTreeSet<_>>();
        let Some(route) = actions
            .iter()
            .next()
            .map(|action| match action.evidence_kind {
                GraphRecoveryEvidenceKindV1::FocusedTest => RecoveryRoute::FocusedTest,
                GraphRecoveryEvidenceKindV1::Trace
                | GraphRecoveryEvidenceKindV1::Implementation
                | GraphRecoveryEvidenceKindV1::Caller => RecoveryRoute::Implementation,
            })
        else {
            return false;
        };
        if actions.iter().any(|action| {
            let candidate_route = match action.evidence_kind {
                GraphRecoveryEvidenceKindV1::FocusedTest => RecoveryRoute::FocusedTest,
                GraphRecoveryEvidenceKindV1::Trace
                | GraphRecoveryEvidenceKindV1::Implementation
                | GraphRecoveryEvidenceKindV1::Caller => RecoveryRoute::Implementation,
            };
            candidate_route != route
        }) {
            return false;
        }
        let admissible = match self.phase.as_ref() {
            Some(AnchorPhase::Root(anchors)) | Some(AnchorPhase::Trail(anchors)) => {
                anchors.roots.get(root).is_some_and(|anchor| {
                    let compatible = anchor.evidence.compatible_actions(anchor, route);
                    actions.iter().all(|action| compatible.contains(action))
                })
            }
            Some(AnchorPhase::Recovery(recovery)) => {
                recovery.anchors.roots.get(root).is_some_and(|anchor| {
                    let compatible = anchor.evidence.compatible_actions(anchor, route);
                    actions.iter().all(|action| compatible.contains(action))
                })
            }
            Some(AnchorPhase::GapRecovery(recovery)) => {
                !recovery.exhausted_roots.contains(root)
                    && recovery.remaining > 0
                    && recovery.anchors.roots.get(root).is_some_and(|anchor| {
                        let compatible = anchor.evidence.compatible_actions(anchor, route);
                        actions.iter().all(|action| compatible.contains(action))
                    })
            }
            Some(
                AnchorPhase::EnabledComplete(_)
                | AnchorPhase::EnabledIncomplete(_)
                | AnchorPhase::ProviderUnavailable,
            )
            | None => false,
        };
        if !admissible {
            return false;
        }
        let phase = self
            .phase
            .take()
            .expect("an admissible forest is installed");
        let promoted = match phase {
            AnchorPhase::Root(anchors)
            | AnchorPhase::Trail(anchors)
            | AnchorPhase::Recovery(Recovery { anchors, .. }) => {
                let remaining_pivots = anchors.roots.len().saturating_sub(1);
                GapRecovery {
                    anchors,
                    active_root: root.to_string(),
                    route,
                    remaining: MAX_DECISION_GAP_RECOVERY_CALLS,
                    exhausted_roots: BTreeSet::new(),
                    remaining_pivots,
                }
            }
            AnchorPhase::GapRecovery(mut recovery) => {
                recovery.active_root = root.to_string();
                recovery.route = route;
                recovery
            }
            AnchorPhase::EnabledComplete(_)
            | AnchorPhase::EnabledIncomplete(_)
            | AnchorPhase::ProviderUnavailable => {
                unreachable!("terminal phases are not admissible")
            }
        };
        self.phase = Some(AnchorPhase::GapRecovery(promoted));
        self.exploration = ExplorationStatus::GapRecovery;
        true
    }
}
