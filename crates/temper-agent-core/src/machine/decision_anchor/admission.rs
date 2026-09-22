//! Immutable, root-local pre-provider recovery admission.

use crate::{EligibleLineageAdmission, LineageAdmissionOutcome};
use sha2::{Digest as _, Sha256};

use super::*;

mod forest_selection;

use forest_selection::ForestRootSelection;

const MAX_REJECTED_RECOVERY_TUPLES: usize = 64;

struct RecoveryAdmissionSnapshot {
    active_root: String,
    missing: BTreeSet<DecisionGap>,
    compatible: BTreeSet<GraphRecoveryActionV1>,
    pending: BTreeSet<DecisionGap>,
    remaining: u8,
    denial: ToolCallDenial,
}

impl DecisionAnchorState {
    #[cfg(test)]
    pub(in crate::machine) fn on_tool_dispatched(
        &mut self,
        call: &ToolCall,
        turn: usize,
    ) -> Option<ToolCallDenial> {
        self.on_tool_dispatched_with_admission(call, turn, None)
    }

    #[cfg(test)]
    pub(in crate::machine) fn on_tool_dispatched_with_admission(
        &mut self,
        call: &ToolCall,
        turn: usize,
        admission: Option<&LineageAdmissionOutcome>,
    ) -> Option<ToolCallDenial> {
        self.on_tool_batch_dispatched_with_admissions(
            std::slice::from_ref(call),
            turn,
            &[admission.cloned()],
        )
        .pop()
        .flatten()
    }

    /// Evaluates every sibling against one immutable recovery snapshot. Only
    /// admitted calls are retained for settlement. An ineligible ordinary
    /// selector attempt at the current compatible action is bounded as
    /// non-progress; rejected opaque references remain fail-closed without
    /// spending an unattempted valid active-root action.
    #[cfg(test)]
    pub(in crate::machine) fn on_tool_batch_dispatched_with_admissions(
        &mut self,
        calls: &[ToolCall],
        turn: usize,
        admissions: &[Option<LineageAdmissionOutcome>],
    ) -> Vec<Option<ToolCallDenial>> {
        self.on_tool_batch_dispatched_with_admissions_and_targets(
            calls,
            turn,
            admissions,
            &vec![None; calls.len()],
        )
    }

    /// Adds canonical ordinary target facts to the immutable graph-recovery
    /// snapshot. Both admission vectors are wrapper-owned and content-free.
    #[cfg(test)]
    pub(in crate::machine) fn on_tool_batch_dispatched_with_admissions_and_targets(
        &mut self,
        calls: &[ToolCall],
        turn: usize,
        admissions: &[Option<LineageAdmissionOutcome>],
        invocation_targets: &[Option<InvocationTargetAdmission>],
    ) -> Vec<Option<ToolCallDenial>> {
        self.on_tool_batch_dispatched_with_closed_inputs(
            calls,
            turn,
            admissions,
            invocation_targets,
            &vec![None; calls.len()],
            &vec![None; calls.len()],
        )
    }

    pub(in crate::machine) fn on_tool_batch_dispatched_with_closed_inputs(
        &mut self,
        calls: &[ToolCall],
        turn: usize,
        admissions: &[Option<LineageAdmissionOutcome>],
        invocation_targets: &[Option<InvocationTargetAdmission>],
        incomplete_graph_selectors: &[Option<GraphCorrelationToolV1>],
        recovery_reference_dispositions: &[Option<
            temper_protocol_activity::GraphRecoveryReferenceDispositionV1,
        >],
    ) -> Vec<Option<ToolCallDenial>> {
        debug_assert_eq!(calls.len(), admissions.len());
        debug_assert_eq!(calls.len(), invocation_targets.len());
        debug_assert_eq!(calls.len(), incomplete_graph_selectors.len());
        debug_assert_eq!(calls.len(), recovery_reference_dispositions.len());
        let forest_selection = self.promote_unique_forest_root(admissions);
        let forest_selection_conflict = forest_selection.is_err();
        let forest_selection = forest_selection.ok().flatten();
        let snapshot = self.staged_admission_snapshot(forest_selection.as_ref());
        let batch_has_exact_read_or_creation = invocation_targets.iter().any(|target| {
            matches!(
                target,
                Some(
                    InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(_))
                        | InvocationTargetAdmission::PatchCreation { .. }
                )
            )
        });
        let batch_has_correction_preview = admissions.iter().any(|admission| {
            matches!(
                admission,
                Some(LineageAdmissionOutcome::Eligible(admission))
                    if admission.is_implementation_authority_correction()
                        && admission.is_implementation_candidate_preview()
            )
        });
        let mut correction_commit_selected = false;
        let mut selected = BTreeSet::new();
        let mut admitted_actions = Vec::new();
        let mut admitted_count = 0u8;
        let mut local_rejections = Vec::new();
        let mut local_readiness_deferrals = 0usize;
        let mut local_readiness_exhaustions = 0usize;
        let mut denials = Vec::with_capacity(calls.len());

        for (
            (((call, admission), invocation_target), incomplete_graph_selector),
            recovery_reference_disposition,
        ) in calls
            .iter()
            .zip(admissions)
            .zip(invocation_targets)
            .zip(incomplete_graph_selectors)
            .zip(recovery_reference_dispositions)
        {
            let order = self.next_call_order;
            self.next_call_order = self.next_call_order.saturating_add(1);
            let mut denial = None;
            if (call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX)
                && call.name != "codebase_memory_check_index_coverage")
                || incomplete_graph_selector.is_some()
            {
                let traversal_not_ready = matches!(
                    admission,
                    Some(LineageAdmissionOutcome::Ineligible(
                        crate::LineageAdmissionStatus::TraversalNotReady
                    ))
                );
                let traversal_readiness_exhausted = matches!(
                    admission,
                    Some(LineageAdmissionOutcome::Ineligible(
                        crate::LineageAdmissionStatus::TraversalReadinessExhausted
                    ))
                );
                let eligible = match admission.as_ref() {
                    Some(LineageAdmissionOutcome::Eligible(admission)) => Some(admission),
                    Some(LineageAdmissionOutcome::Ineligible(_)) | None => None,
                };
                let requested_gap = incomplete_graph_selector
                    .is_some_and(|tool| tool == GraphCorrelationToolV1::TracePath)
                    .then_some(DecisionGap::Trace)
                    .or_else(|| DecisionGap::from_call(call));
                let requested_action = DecisionGap::recovery_action_for_call(call).or_else(|| {
                    requested_gap
                        .map(|gap| GraphRecoveryActionV1::for_evidence(gap.recovery_kind()))
                });
                let recovery_gap = eligible
                    .and_then(DecisionGap::from_admission)
                    .or_else(|| admission.is_none().then_some(requested_gap).flatten());
                let recovery_action = eligible
                    .and_then(DecisionGap::recovery_action_for_admission)
                    .or_else(|| admission.is_none().then_some(requested_action).flatten());
                let is_traversal = call.name == GraphCorrelationToolV1::TracePath.public_name();
                let has_traversal_selector = is_traversal
                    && call
                        .arguments
                        .get("function_name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|selector| !selector.is_empty());
                let admitted_root = eligible
                    .and_then(|admission| self.root_matching_admission(admission))
                    // Direct state tests and non-codebase-memory compositions
                    // retain their legacy untyped-call path. A schema-valid
                    // traversal selector must instead have root admission.
                    .or_else(|| {
                        (admission.is_none()
                            && incomplete_graph_selector.is_none()
                            && !has_traversal_selector)
                            .then(|| {
                                snapshot
                                    .as_ref()
                                    .map(|snapshot| snapshot.active_root.clone())
                            })
                            .flatten()
                    });
                let call_key = GraphCorrelationV1::target_digest(&call.id);
                let tuple_identity = snapshot.as_ref().and_then(|snapshot| {
                    requested_action.and_then(|action| {
                        RecoveryTupleIdentity::for_call(call, action, &snapshot.active_root)
                    })
                });
                let already_rejected = tuple_identity
                    .is_some_and(|identity| self.rejected_recovery_tuples.contains(&identity));
                let readiness_recheck =
                    eligible.is_some_and(EligibleLineageAdmission::is_traversal_readiness_recheck);
                let correction =
                    eligible.filter(|admission| admission.is_implementation_authority_correction());
                let correction_preview = correction
                    .is_some_and(EligibleLineageAdmission::is_implementation_candidate_preview);
                let completes_correction_inspection = correction.is_some_and(|admission| {
                    admission.completes_implementation_correction_inspection()
                });
                let correction_admissible = correction.is_some_and(|admission| {
                    !batch_has_exact_read_or_creation
                        && (!batch_has_correction_preview || correction_preview)
                        && (correction_preview || !correction_commit_selected)
                        && (if correction_preview {
                            !self.implementation_correction_inspection_completed
                        } else {
                            self.implementation_correction_inspection_completed
                        })
                        && call_key.is_some()
                        && admitted_root.as_deref().is_some_and(|root| {
                            admission.matches_root(root)
                                && self.implementation_correction_is_available(root)
                        })
                });

                // A traversal is meaningful only for the staged active root.
                // A selector owned by a retained sibling or by no root must be
                // denied instead of escaping to the provider.
                let staged_call = snapshot.as_ref().is_some_and(|snapshot| {
                    if is_traversal {
                        requested_action.is_some()
                            && (has_traversal_selector
                                || admission.is_some()
                                || incomplete_graph_selector.is_some())
                    } else {
                        (admission.is_some() || incomplete_graph_selector.is_some())
                            && requested_action.is_some()
                            && admitted_root
                                .as_deref()
                                .is_none_or(|root| root == snapshot.active_root)
                    }
                });
                if self.exploration != ExplorationStatus::Open || staged_call {
                    let readiness_recheck_admissible = readiness_recheck
                        && snapshot.as_ref().is_some_and(|snapshot| {
                            !already_rejected
                                && call_key.is_some()
                                && snapshot.missing.contains(&DecisionGap::Trace)
                                && admitted_root.as_deref() == Some(snapshot.active_root.as_str())
                        });
                    let admissible = correction_admissible
                        || readiness_recheck_admissible
                        || snapshot.as_ref().is_some_and(|snapshot| {
                            !already_rejected
                                && !(forest_selection_conflict
                                    && eligible
                                        .is_some_and(|admission| admission.selects_forest_root()))
                                && call_key.is_some()
                                && recovery_gap.is_some_and(|gap| {
                                    snapshot.missing.contains(&gap)
                                        && recovery_action.is_some_and(|action| {
                                            snapshot.compatible.contains(&action)
                                        })
                                        && !snapshot.pending.contains(&gap)
                                        && !selected.contains(&gap)
                                        && admitted_count < snapshot.remaining
                                })
                                && admitted_root.as_deref() == Some(snapshot.active_root.as_str())
                                && eligible.is_none_or(|admission| {
                                    DecisionGap::recovery_action_for_admission(admission)
                                        .is_some_and(|action| snapshot.compatible.contains(&action))
                                        && admission.matches_root(&snapshot.active_root)
                                })
                        });
                    if admissible {
                        if correction_admissible {
                            if !correction_preview {
                                correction_commit_selected = true;
                                self.implementation_correction_attempted = true;
                            }
                        } else {
                            let gap = recovery_gap.expect("admissible recovery has a purpose");
                            selected.insert(gap);
                            if !readiness_recheck_admissible {
                                admitted_actions.push(
                                    recovery_action
                                        .expect("admissible recovery has a closed action"),
                                );
                                admitted_count = admitted_count.saturating_add(1);
                            }
                        }
                    } else {
                        denial = Some(snapshot.as_ref().map_or_else(
                            || self.graph_exploration_denial(),
                            |snapshot| snapshot.denial.clone(),
                        ));
                        if traversal_not_ready {
                            local_readiness_deferrals += 1;
                        } else if traversal_readiness_exhausted {
                            local_readiness_exhaustions += 1;
                        } else if requested_action.is_some() {
                            let excluded = already_rejected
                                || tuple_identity.is_some_and(|identity| {
                                    if self.rejected_recovery_tuples.len()
                                        >= MAX_REJECTED_RECOVERY_TUPLES
                                    {
                                        true
                                    } else {
                                        self.rejected_recovery_tuples.insert(identity)
                                    }
                                });
                            let rejected_reference = *recovery_reference_disposition
                                == Some(
                                    temper_protocol_activity::GraphRecoveryReferenceDispositionV1::Rejected,
                                );
                            local_rejections.push((requested_action, excluded, rejected_reference));
                        }
                    }
                }

                if denial.is_none() && call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
                    if let Some(call_key) = call_key {
                        self.calls.insert(
                            call_key,
                            PendingCodebaseCall {
                                turn,
                                order,
                                recovery_gap,
                                admitted_root,
                                admission_checked: admission.is_some(),
                                implementation_correction_preview: correction_admissible
                                    && correction_preview,
                                completes_implementation_correction_inspection:
                                    correction_admissible && completes_correction_inspection,
                            },
                        );
                    }
                }
            }
            if denial.is_none() && call.name == "read" {
                if self.correction_inspection_blocks_exact_read(invocation_target.as_ref()) {
                    denial = Some(ToolCallDenial::DecisionAnchorCorrectionInspection);
                    self.queue_correction_inspection_guidance();
                } else {
                    self.register_exact_read(call, turn, order, invocation_target.as_ref());
                }
            }
            if denial.is_none()
                && self.blocks_invocation_mutation(&call.name, invocation_target.as_ref())
            {
                denial = Some(super::mutation_diagnostic::blocked_mutation_denial(
                    invocation_target.as_ref(),
                ));
            }
            denials.push(denial);
        }

        if admitted_count > 0 {
            if let Some(AnchorPhase::GapRecovery(recovery)) = self.phase.as_mut() {
                recovery.remaining = recovery.remaining.saturating_sub(admitted_count);
                if let Some(active) = recovery.anchors.roots.get_mut(&recovery.active_root) {
                    for action in admitted_actions {
                        if action == GraphRecoveryActionV1::focused_test_traversal() {
                            active.evidence.record_focused_test_traversal(turn);
                        } else if action == GraphRecoveryActionV1::focused_test_semantic_fallback()
                        {
                            active.evidence.record_focused_test_fallback(turn);
                        }
                    }
                }
            }
        }

        for _ in 0..local_readiness_exhaustions {
            self.enter_local_traversal_readiness_recovery();
            self.queue_local_denial_guidance(
                Some(GraphRecoveryActionV1::for_evidence(
                    GraphRecoveryEvidenceKindV1::Trace,
                )),
                true,
            );
        }
        let local_batch_made_no_progress = admitted_count == 0
            && local_rejections
                .iter()
                .any(|(_, _, rejected_reference)| !rejected_reference);
        if local_batch_made_no_progress {
            self.enter_local_traversal_readiness_recovery();
        }
        for (action, excluded, _) in local_rejections {
            self.queue_local_denial_guidance(action, excluded);
        }
        for _ in 0..local_readiness_deferrals {
            self.queue_local_traversal_readiness_guidance();
        }
        denials
    }

    #[cfg(test)]
    pub(in crate::machine) fn on_tool_dispatched_with_targets(
        &mut self,
        call: &ToolCall,
        turn: usize,
        admission: Option<&InvocationTargetAdmission>,
    ) -> Option<ToolCallDenial> {
        self.on_tool_batch_dispatched_with_admissions_and_targets(
            std::slice::from_ref(call),
            turn,
            &[None],
            &[admission.cloned()],
        )
        .pop()
        .flatten()
    }

    fn implementation_correction_is_available(&self, root: &str) -> bool {
        if self.implementation_correction_attempted || self.implementation_authority_exercised {
            return false;
        }
        let Some(AnchorPhase::EnabledComplete(anchors)) = self.phase.as_ref() else {
            return false;
        };
        anchors.roots.get(root).is_some_and(|anchor| {
            anchor.evidence.implementation_correction_available
                && !anchor.evidence.implementation_authority_corrected
        })
    }

    pub(super) fn root_matching_admission(
        &self,
        admission: &EligibleLineageAdmission,
    ) -> Option<String> {
        let anchors = match self.phase.as_ref()? {
            AnchorPhase::Root(anchors)
            | AnchorPhase::Trail(anchors)
            | AnchorPhase::EnabledComplete(anchors) => anchors,
            AnchorPhase::Recovery(recovery) => &recovery.anchors,
            AnchorPhase::GapRecovery(recovery) => &recovery.anchors,
            AnchorPhase::EnabledIncomplete(_) | AnchorPhase::ProviderUnavailable => {
                return None;
            }
        };
        anchors
            .roots
            .keys()
            .find(|root| admission.matches_root(root))
            .cloned()
    }

    fn staged_admission_snapshot(
        &self,
        forest_selection: Option<&ForestRootSelection>,
    ) -> Option<RecoveryAdmissionSnapshot> {
        let (active_root, active, route, remaining, denial) = match self.phase.as_ref()? {
            AnchorPhase::Root(anchors) | AnchorPhase::Trail(anchors)
                if self.exploration == ExplorationStatus::Open =>
            {
                let (active_root, route) = forest_selection
                    .map(|selection| (&selection.active_root, selection.route))
                    .or_else(|| anchors.active_selection(&BTreeSet::new()))?;
                let active = anchors.roots.get(active_root)?;
                let details = GraphExplorationClosedV1::recoverable_without_actions(
                    anchors.missing_kinds(active_root, route),
                    MAX_DECISION_GAP_RECOVERY_CALLS,
                );
                (
                    active_root.clone(),
                    active,
                    route,
                    MAX_DECISION_GAP_RECOVERY_CALLS,
                    ToolCallDenial::GraphExplorationClosed(details),
                )
            }
            AnchorPhase::GapRecovery(recovery) => {
                let active = recovery.anchors.roots.get(&recovery.active_root)?;
                (
                    recovery.active_root.clone(),
                    active,
                    recovery.route,
                    recovery.remaining,
                    self.graph_exploration_denial(),
                )
            }
            _ => return None,
        };
        let compatible = active.evidence.compatible_actions(active, route);
        Some(RecoveryAdmissionSnapshot {
            active_root: active_root.clone(),
            missing: match self.phase.as_ref() {
                Some(AnchorPhase::Root(anchors)) | Some(AnchorPhase::Trail(anchors)) => anchors
                    .missing_kinds(&active_root, route)
                    .into_iter()
                    .map(|kind| match kind {
                        GraphRecoveryEvidenceKindV1::Trace => DecisionGap::Trace,
                        GraphRecoveryEvidenceKindV1::Implementation => {
                            DecisionGap::Evidence(DecisionEvidenceKindV1::Implementation)
                        }
                        GraphRecoveryEvidenceKindV1::Caller => {
                            DecisionGap::Evidence(DecisionEvidenceKindV1::Caller)
                        }
                        GraphRecoveryEvidenceKindV1::FocusedTest => {
                            DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)
                        }
                    })
                    .collect(),
                Some(AnchorPhase::GapRecovery(recovery)) => recovery
                    .anchors
                    .missing_kinds(&active_root, route)
                    .into_iter()
                    .map(|kind| match kind {
                        GraphRecoveryEvidenceKindV1::Trace => DecisionGap::Trace,
                        GraphRecoveryEvidenceKindV1::Implementation => {
                            DecisionGap::Evidence(DecisionEvidenceKindV1::Implementation)
                        }
                        GraphRecoveryEvidenceKindV1::Caller => {
                            DecisionGap::Evidence(DecisionEvidenceKindV1::Caller)
                        }
                        GraphRecoveryEvidenceKindV1::FocusedTest => {
                            DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)
                        }
                    })
                    .collect(),
                _ => BTreeSet::new(),
            },
            compatible,
            pending: self
                .calls
                .values()
                .filter_map(|call| call.recovery_gap)
                .collect(),
            remaining,
            denial,
        })
    }

    fn enter_local_traversal_readiness_recovery(&mut self) {
        let Some(phase) = self.phase.take() else {
            return;
        };
        match phase {
            AnchorPhase::Root(anchors) | AnchorPhase::Trail(anchors) => {
                let _ = self.enter_gap_recovery(anchors);
                self.spend_local_recovery_slot();
            }
            AnchorPhase::GapRecovery(recovery) => {
                self.phase = Some(AnchorPhase::GapRecovery(recovery));
                self.spend_local_recovery_slot();
            }
            AnchorPhase::EnabledIncomplete(evidence) => {
                self.phase = Some(AnchorPhase::EnabledIncomplete(evidence));
                self.exploration = ExplorationStatus::EnabledIncomplete;
            }
            phase @ (AnchorPhase::Recovery(_)
            | AnchorPhase::EnabledComplete(_)
            | AnchorPhase::ProviderUnavailable) => {
                self.phase = Some(phase);
            }
        }
    }

    fn spend_local_recovery_slot(&mut self) {
        let Some(AnchorPhase::GapRecovery(recovery)) = self.phase.as_mut() else {
            return;
        };
        recovery.remaining = recovery.remaining.saturating_sub(1);
        if recovery.remaining > 0 {
            self.exploration = ExplorationStatus::GapRecovery;
            return;
        }
        let evidence = recovery
            .anchors
            .roots
            .get(&recovery.active_root)
            .map(|anchor| anchor.evidence.clone())
            .unwrap_or_default();
        self.phase = Some(AnchorPhase::EnabledIncomplete(evidence));
        self.exploration = ExplorationStatus::EnabledIncomplete;
    }
}

pub(super) fn hash_recovery_identity_part(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

impl RecoveryTupleIdentity {
    /// Retains only a fixed-width, process-local identity for the closed
    /// action/selector tuple. Raw selector values never enter policy state.
    pub(in crate::machine) fn for_call(
        call: &ToolCall,
        action: GraphRecoveryActionV1,
        active_root: &str,
    ) -> Option<Self> {
        let selector_field = match action.selector_kind {
            DecisionAnchorTargetKindV1::GraphQuery => "query",
            DecisionAnchorTargetKindV1::Pattern => "pattern",
            DecisionAnchorTargetKindV1::NamePattern => "name_pattern",
            DecisionAnchorTargetKindV1::QualifiedNamePattern => "qn_pattern",
            DecisionAnchorTargetKindV1::FunctionName => "function_name",
            DecisionAnchorTargetKindV1::QualifiedName => "qualified_name",
        };
        let selector = call
            .arguments
            .get(selector_field)
            .and_then(serde_json::Value::as_str)?;
        let mut digest = Sha256::new();
        digest.update(b"temper-rejected-recovery-tuple-v2\0");
        hash_recovery_identity_part(&mut digest, active_root.as_bytes());
        hash_recovery_identity_part(&mut digest, action.model_label().as_bytes());
        hash_recovery_identity_part(&mut digest, selector.as_bytes());
        Some(Self(digest.finalize().into()))
    }
}
