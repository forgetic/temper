//! Exact provider-selector promotion within the retained decision forest.

use crate::LineageAdmissionOutcome;

use super::*;

pub(super) struct ForestRootSelection {
    pub(super) active_root: String,
    pub(super) route: RecoveryRoute,
}

impl DecisionAnchorState {
    pub(super) fn promote_unique_forest_root(
        &mut self,
        admissions: &[Option<LineageAdmissionOutcome>],
    ) -> Result<Option<ForestRootSelection>, ()> {
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
            return if selections.is_empty() {
                Ok(None)
            } else {
                Err(())
            };
        };
        let persist = !matches!(
            self.phase.as_ref(),
            Some(AnchorPhase::Root(_) | AnchorPhase::Trail(_))
        ) || self.exploration != ExplorationStatus::Open;
        let route = self
            .promote_forest_root(root, [*action], persist)
            .ok_or(())?;
        Ok(Some(ForestRootSelection {
            active_root: root.clone(),
            route,
        }))
    }

    fn promote_forest_root(
        &mut self,
        root: &str,
        actions: impl IntoIterator<Item = GraphRecoveryActionV1>,
        persist: bool,
    ) -> Option<RecoveryRoute> {
        let actions = actions.into_iter().collect::<BTreeSet<_>>();
        let route = actions
            .iter()
            .next()
            .map(|action| match action.evidence_kind {
                GraphRecoveryEvidenceKindV1::FocusedTest => RecoveryRoute::FocusedTest,
                GraphRecoveryEvidenceKindV1::Trace
                | GraphRecoveryEvidenceKindV1::Implementation
                | GraphRecoveryEvidenceKindV1::Caller => RecoveryRoute::Implementation,
            })?;
        if actions.iter().any(|action| {
            let candidate_route = match action.evidence_kind {
                GraphRecoveryEvidenceKindV1::FocusedTest => RecoveryRoute::FocusedTest,
                GraphRecoveryEvidenceKindV1::Trace
                | GraphRecoveryEvidenceKindV1::Implementation
                | GraphRecoveryEvidenceKindV1::Caller => RecoveryRoute::Implementation,
            };
            candidate_route != route
        }) {
            return None;
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
                !recovery
                    .exhausted_routes
                    .contains(&(root.to_string(), route))
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
            return None;
        }
        if !persist {
            return Some(route);
        }
        let phase = self
            .phase
            .take()
            .expect("an admissible forest is installed");
        let mut promoted = match phase {
            AnchorPhase::Root(anchors)
            | AnchorPhase::Trail(anchors)
            | AnchorPhase::Recovery(Recovery { anchors, .. }) => {
                let remaining_pivots = anchors.roots.len().saturating_sub(1);
                GapRecovery {
                    anchors,
                    active_root: root.to_string(),
                    route,
                    remaining: MAX_DECISION_GAP_RECOVERY_CALLS,
                    exhausted_routes: BTreeSet::new(),
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
        promoted
            .anchors
            .mark_parallel_recovery(root, promoted.route);
        self.phase = Some(AnchorPhase::GapRecovery(promoted));
        self.exploration = ExplorationStatus::GapRecovery;
        Some(route)
    }
}
