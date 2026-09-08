//! New files require a completed decision, not fictitious read authority.

use super::*;

impl DecisionAnchorState {
    pub(super) fn patch_creation_authorized(
        &self,
        existing: &[TargetAdmissionOutcome],
        has_creations: bool,
    ) -> bool {
        if !has_creations {
            return false;
        }
        let admission = InvocationTargetAdmission::Mutation(existing.to_vec());
        match self.phase.as_ref() {
            Some(AnchorPhase::EnabledComplete(anchors)) => {
                let correction_pending = anchors.implementation_root().is_some_and(|(_, root)| {
                    root.evidence.implementation_correction_available
                        && !root.evidence.implementation_authority_corrected
                        && !self.implementation_correction_inspection_completed
                });
                anchors.has_complete_evidence()
                    && !correction_pending
                    && (existing.is_empty()
                        || self.mutation_targets_authorized(anchors, Some(&admission)))
            }
            Some(AnchorPhase::ProviderUnavailable) => {
                existing.is_empty()
                    || self.conventional_mutation_targets_authorized(Some(&admission))
            }
            _ => false,
        }
    }
}
