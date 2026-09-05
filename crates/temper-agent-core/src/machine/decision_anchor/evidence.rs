//! Typed evidence completion and bounded gap-recovery lifecycle.

use super::*;

impl DecisionAnchorState {
    pub(super) fn settle_open_candidate_recovery(
        &mut self,
        anchors: AnchorForest,
        finished: &[FinishedCodebaseCall<'_>],
        active_root: Option<&str>,
    ) -> Result<AnchorForest, DecisionAnchorTransition> {
        let disposition = finished.iter().find_map(|finished| {
            (finished.call.admitted_root.as_deref() == active_root)
                .then(|| candidate_recovery_disposition(finished.name, finished.output))
                .flatten()
        });
        match disposition {
            Some(CandidateRecoveryDisposition::RetryAvailable) => {
                self.phase = Some(AnchorPhase::Trail(anchors));
                Err(DecisionAnchorTransition::Unchanged)
            }
            Some(CandidateRecoveryDisposition::Exhausted) => {
                let evidence = anchors.active_evidence();
                Err(self.enter_incomplete_enabled(evidence))
            }
            None => Ok(anchors),
        }
    }

    pub(super) fn enter_gap_recovery(&mut self, anchors: AnchorForest) -> DecisionAnchorTransition {
        debug_assert!(!anchors.has_complete_evidence());
        let mut anchors = anchors;
        let Some((active_root, route)) = anchors.recovery_selection() else {
            return self.enter_incomplete_enabled(anchors.active_evidence());
        };
        anchors.mark_parallel_recovery(&active_root, route);
        let remaining_pivots = anchors.roots.len().saturating_sub(1);
        self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
            anchors,
            active_root,
            route,
            remaining: MAX_DECISION_GAP_RECOVERY_CALLS,
            exhausted_roots: BTreeSet::new(),
            remaining_pivots,
        }));
        self.exploration = ExplorationStatus::GapRecovery;
        DecisionAnchorTransition::GapRecoveryNeeded
    }

    fn pivot_gap_recovery_or_exhaust(
        &mut self,
        mut anchors: AnchorForest,
        exhausted_root: String,
        mut exhausted_roots: BTreeSet<String>,
        remaining_pivots: usize,
    ) -> DecisionAnchorTransition {
        let exhausted_evidence = anchors
            .roots
            .get(&exhausted_root)
            .map(|root| root.evidence.clone())
            .unwrap_or_default();
        exhausted_roots.insert(exhausted_root);
        let next = (remaining_pivots > 0)
            .then(|| anchors.recovery_selection_excluding(&exhausted_roots))
            .flatten();
        let Some((active_root, route)) = next else {
            return self.enter_incomplete_enabled(exhausted_evidence);
        };
        anchors.mark_parallel_recovery(&active_root, route);

        self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
            anchors,
            active_root,
            route,
            remaining: MAX_DECISION_GAP_RECOVERY_CALLS,
            exhausted_roots,
            remaining_pivots: remaining_pivots.saturating_sub(1),
        }));
        self.exploration = ExplorationStatus::GapRecovery;
        DecisionAnchorTransition::GapRecoveryNeeded
    }

    pub(super) fn enter_provider_unavailable(&mut self) -> DecisionAnchorTransition {
        self.source_authorities.clear();
        self.pending_exact_reads.clear();
        self.exact_read_authorities.clear();
        self.phase = Some(AnchorPhase::ProviderUnavailable);
        self.exploration = ExplorationStatus::ProviderUnavailable;
        DecisionAnchorTransition::ProviderUnavailableFallback
    }

    pub(super) fn enter_incomplete_enabled(
        &mut self,
        evidence: SourceEvidence,
    ) -> DecisionAnchorTransition {
        self.phase = Some(AnchorPhase::EnabledIncomplete(evidence));
        self.exploration = ExplorationStatus::EnabledIncomplete;
        DecisionAnchorTransition::EnabledEvidenceIncomplete
    }

    pub(super) fn advance_gap_recovery(
        &mut self,
        recovery: GapRecovery,
        finished: &[FinishedCodebaseCall<'_>],
    ) -> DecisionAnchorTransition {
        let GapRecovery {
            mut anchors,
            active_root,
            route,
            remaining,
            exhausted_roots,
            remaining_pivots,
        } = recovery;
        let Some(active) = anchors.roots.get(&active_root) else {
            return self.pivot_gap_recovery_or_exhaust(
                anchors,
                active_root,
                exhausted_roots,
                remaining_pivots,
            );
        };
        let compatible = finished
            .iter()
            .filter_map(|finished| {
                if finished.call.admitted_root.as_deref() != Some(active_root.as_str()) {
                    return None;
                }
                let output = anchor_output(finished.name, finished.output)?;
                (output.lineage.root_binding == active_root
                    && active.accepts(&finished.call, &output.lineage))
                .then_some((
                    finished.id,
                    finished.call.clone(),
                    output,
                    finished.source_target,
                ))
            })
            .collect::<Vec<_>>();
        let had_trace = active.evidence.has_trace();
        let had_caller_selector = active.evidence.caller_selector_available;
        let had_focused_test_selector = active.evidence.focused_test_selector_available;
        let had_implementation = active
            .evidence
            .decision_kinds
            .contains(&DecisionEvidenceKindV1::Implementation);
        let parallel_recovery = active.evidence.trace_before_implementation;
        let batch_trace = (route == RecoveryRoute::Implementation)
            .then(|| {
                compatible
                    .iter()
                    .filter(|(_, call, output, _)| {
                        call.recovery_gap == Some(DecisionGap::Trace)
                            && output.tool == GraphCorrelationToolV1::TracePath
                            && output.lineage.caller_discovery.is_some()
                    })
                    .min_by_key(|(_, call, _, _)| call.turn)
                    .map(|(id, call, output, _)| (*id, call.turn, output.lineage.caller_discovery))
            })
            .flatten();
        let decision_kinds = compatible
            .iter()
            .filter_map(|(id, call, output, _)| match call.recovery_gap {
                Some(DecisionGap::Evidence(expected))
                    if output.tool == GraphCorrelationToolV1::GetCodeSnippet
                        && output.lineage.decision_evidence_kind == Some(expected)
                        && match expected {
                            DecisionEvidenceKindV1::Implementation => {
                                route == RecoveryRoute::Implementation
                            }
                            DecisionEvidenceKindV1::Caller => {
                                route == RecoveryRoute::Implementation
                                    && had_trace
                                    && (had_caller_selector || !call.admission_checked)
                                    && (had_implementation
                                        || parallel_recovery
                                        || !call.admission_checked)
                            }
                            DecisionEvidenceKindV1::FocusedTest => {
                                (route == RecoveryRoute::FocusedTest
                                    || route == RecoveryRoute::Implementation
                                        && parallel_recovery
                                        && had_trace)
                                    && (had_focused_test_selector || !call.admission_checked)
                            }
                        } =>
                {
                    Some((*id, expected))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let accepted_kinds = decision_kinds
            .iter()
            .map(|(_, kind)| *kind)
            .collect::<BTreeSet<_>>();
        let accepted_sources = compatible
            .iter()
            .filter_map(|(id, call, output, source_target)| {
                let expected = output.lineage.decision_evidence_kind?;
                (output.tool == GraphCorrelationToolV1::GetCodeSnippet
                    && decision_kinds.contains(&(*id, expected)))
                .then_some((*id, call, expected, *source_target))
            })
            .collect::<Vec<_>>();
        for (id, call, expected, source_target) in accepted_sources {
            self.record_source_authority(&active_root, call, source_target);
            self.mark_accepted(id, AcceptedEvidence::from(expected));
        }
        let active = anchors
            .roots
            .get_mut(&active_root)
            .expect("the active recovery root remains installed");
        if let Some((id, turn, outcome)) = batch_trace {
            active.evidence.record_trace(turn);
            let correction_available = compatible
                .iter()
                .find(|(candidate_id, _, _, _)| candidate_id == &id)
                .is_some_and(|(_, _, output, _)| {
                    output.lineage.implementation_correction_available
                });
            active
                .evidence
                .record_implementation_correction_availability(correction_available);
            active.evidence.record_caller_discovery(outcome);
            self.mark_accepted(id, AcceptedEvidence::Trace);
        }
        for (id, call, output, _) in &compatible {
            if call.recovery_gap != Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
            {
                continue;
            }
            match output.tool {
                GraphCorrelationToolV1::TracePath => {
                    active.evidence.record_focused_test_traversal(call.turn)
                }
                GraphCorrelationToolV1::SearchGraph => {
                    active.evidence.record_focused_test_fallback(call.turn)
                }
                GraphCorrelationToolV1::SearchCode | GraphCorrelationToolV1::GetCodeSnippet => {
                    continue;
                }
            }
            self.mark_accepted(id, AcceptedEvidence::FocusedTestRoute);
            active
                .evidence
                .record_focused_test_discovery(output.tool, output.lineage.focused_test_discovery);
        }
        active.evidence.record_decision_kinds(accepted_kinds);
        let active_evidence = active.evidence.clone();

        if anchors.has_complete_evidence() {
            self.phase = Some(AnchorPhase::EnabledComplete(anchors));
            self.exploration = ExplorationStatus::EnabledComplete;
            return DecisionAnchorTransition::EnabledEvidenceComplete;
        }

        let candidate_recovery = finished.iter().find_map(|finished| {
            (finished.call.admitted_root.as_deref() == Some(active_root.as_str()))
                .then(|| candidate_recovery_disposition(finished.name, finished.output))
                .flatten()
        });
        match candidate_recovery {
            Some(CandidateRecoveryDisposition::RetryAvailable) => {
                self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
                    anchors,
                    active_root,
                    route,
                    remaining: remaining
                        .saturating_add(1)
                        .min(MAX_DECISION_GAP_RECOVERY_CALLS),
                    exhausted_roots,
                    remaining_pivots,
                }));
                self.exploration = ExplorationStatus::GapRecovery;
                return DecisionAnchorTransition::GapRecoveryNeeded;
            }
            Some(CandidateRecoveryDisposition::Exhausted) => {
                return self.enter_incomplete_enabled(active_evidence);
            }
            None => {}
        }

        if compatible.iter().any(|(_, call, output, _)| {
            route == RecoveryRoute::FocusedTest
                && call.recovery_gap
                    == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                && output.tool == GraphCorrelationToolV1::GetCodeSnippet
                && output.lineage.decision_evidence_kind
                    != Some(DecisionEvidenceKindV1::FocusedTest)
        }) {
            return self.pivot_gap_recovery_or_exhaust(
                anchors,
                active_root,
                exhausted_roots,
                remaining_pivots,
            );
        }

        if finished.iter().any(|finished| {
            trusted_unavailable_provider_output(finished.name, finished.output)
                && finished.call.admitted_root.as_deref() == Some(active_root.as_str())
                && finished
                    .call
                    .recovery_gap
                    .is_some_and(|gap| active_evidence.needs(gap))
        }) {
            return self.enter_provider_unavailable();
        }

        let active = anchors
            .roots
            .get(&active_root)
            .expect("the active recovery root remains installed");
        let active_route_complete = active_evidence.route_is_complete(route);
        let has_path = !active_evidence.compatible_actions(active, route).is_empty();
        if remaining == 0 || (!has_path && !active_route_complete) {
            return self.pivot_gap_recovery_or_exhaust(
                anchors,
                active_root,
                exhausted_roots,
                remaining_pivots,
            );
        }

        let (next_root, next_route) = if active_route_complete {
            let Some((next_root, next_route)) =
                anchors.recovery_selection_excluding(&exhausted_roots)
            else {
                return self.enter_incomplete_enabled(active_evidence);
            };
            (next_root, next_route)
        } else {
            (active_root, route)
        };
        anchors.mark_parallel_recovery(&next_root, next_route);
        self.phase = Some(AnchorPhase::GapRecovery(GapRecovery {
            anchors,
            active_root: next_root,
            route: next_route,
            remaining,
            exhausted_roots,
            remaining_pivots,
        }));
        self.exploration = ExplorationStatus::GapRecovery;
        DecisionAnchorTransition::GapRecoveryNeeded
    }
}
