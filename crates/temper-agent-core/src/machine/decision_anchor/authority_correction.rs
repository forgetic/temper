//! One pre-mutation replacement of provisional implementation authority.

use super::*;

impl DecisionAnchorState {
    pub(super) fn settle_enabled_implementation_correction(
        &mut self,
        mut anchors: AnchorForest,
        finished: &[FinishedCodebaseCall<'_>],
    ) -> DecisionAnchorTransition {
        let correction = finished.iter().find(|finished| {
            anchor_output(finished.name, finished.output).is_some_and(|output| {
                output.lineage.implementation_authority_corrected
                    && output.lineage.decision_evidence_kind
                        == Some(DecisionEvidenceKindV1::Implementation)
                    && finished.call.admitted_root.as_deref()
                        == Some(output.lineage.root_binding.as_str())
            })
        });
        if let Some(finished) = correction {
            if let Some(output) = anchor_output(finished.name, finished.output) {
                let root = output.lineage.root_binding;
                let eligible = anchors.roots.get(&root).is_some_and(|anchor| {
                    anchor.evidence.implementation_correction_available
                        && !anchor.evidence.implementation_authority_corrected
                });
                if eligible && !self.implementation_authority_exercised {
                    self.replace_implementation_source_authority(
                        &root,
                        &finished.call,
                        finished.source_target,
                    );
                    if let Some(anchor) = anchors.roots.get_mut(&root) {
                        anchor.evidence.mark_implementation_authority_corrected();
                    }
                    self.mark_accepted(finished.id, AcceptedEvidence::ImplementationCorrection);
                }
            }
        }
        self.phase = Some(AnchorPhase::EnabledComplete(anchors));
        self.exploration = ExplorationStatus::EnabledComplete;
        DecisionAnchorTransition::Unchanged
    }
}
