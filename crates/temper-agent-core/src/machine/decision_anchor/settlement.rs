//! Deterministic settlement of complete graph batches.

use super::*;

impl DecisionAnchorState {
    pub(in crate::machine) fn on_tool_batch_finished_with_targets(
        &mut self,
        completed: &[SettledToolCall<'_>],
    ) -> DecisionAnchorTransition {
        self.batch_progress.clear();
        if !completed.is_empty() {
            self.settled_batches = self.settled_batches.saturating_add(1);
        }
        self.settle_exact_reads(completed);
        let finished = completed
            .iter()
            .filter_map(|(id, name, output, source_target, _)| {
                let call_key = GraphCorrelationV1::target_digest(id)?;
                self.calls
                    .remove(&call_key)
                    .map(|call| FinishedCodebaseCall {
                        id,
                        call,
                        name,
                        output,
                        source_target: *source_target,
                    })
            })
            .collect::<Vec<_>>();
        if finished.is_empty() {
            return DecisionAnchorTransition::Unchanged;
        }

        let prior_phase = self.phase.take();
        let transition = if prior_phase.is_none() {
            match AnchorForest::from_finished(&finished, None) {
                Some(anchors) => self.install_roots(anchors, 0, false),
                None if finished.iter().any(|finished| {
                    trusted_unavailable_provider_output(finished.name, finished.output)
                }) =>
                {
                    self.enter_provider_unavailable()
                }
                None if successful_graph_batch(&finished) => self.record_non_progress(None),
                None => DecisionAnchorTransition::Unchanged,
            }
        } else {
            match prior_phase {
                None => unreachable!("the empty phase was handled above"),
                Some(AnchorPhase::Root(anchors)) | Some(AnchorPhase::Trail(anchors)) => {
                    if anchors.has_complete_evidence() {
                        self.phase = Some(AnchorPhase::EnabledComplete(anchors));
                        if self.exploration == ExplorationStatus::Open {
                            self.exploration = ExplorationStatus::EnabledComplete;
                            DecisionAnchorTransition::EnabledEvidenceComplete
                        } else {
                            DecisionAnchorTransition::Unchanged
                        }
                    } else {
                        self.advance_batch_or_recover(anchors, &finished, 1)
                    }
                }
                Some(AnchorPhase::Recovery(recovery)) => {
                    let replacement_roots = if recovery.anchors.is_consumable() {
                        None
                    } else {
                        AnchorForest::from_finished(
                            &finished,
                            Some(recovery.anchors.latest_produced_turn),
                        )
                    };
                    if let Some(anchors) = replacement_roots {
                        self.install_roots(anchors, recovery.attempts.saturating_add(1), true)
                    } else {
                        self.advance_batch_or_recover(
                            recovery.anchors,
                            &finished,
                            recovery.attempts.saturating_add(1),
                        )
                    }
                }
                Some(AnchorPhase::GapRecovery(recovery)) => {
                    self.advance_gap_recovery(recovery, &finished)
                }
                Some(AnchorPhase::EnabledComplete(anchors)) => {
                    self.settle_enabled_implementation_correction(anchors, &finished)
                }
                Some(AnchorPhase::EnabledIncomplete(evidence)) => {
                    self.phase = Some(AnchorPhase::EnabledIncomplete(evidence));
                    self.exploration = ExplorationStatus::EnabledIncomplete;
                    DecisionAnchorTransition::Unchanged
                }
                Some(AnchorPhase::ProviderUnavailable) => {
                    self.phase = Some(AnchorPhase::ProviderUnavailable);
                    self.exploration = ExplorationStatus::ProviderUnavailable;
                    DecisionAnchorTransition::Unchanged
                }
            }
        };
        self.queue_finished_guidance(&finished);
        transition
    }
}
