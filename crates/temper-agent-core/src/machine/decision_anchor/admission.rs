//! Immutable, root-local pre-provider recovery admission.

use crate::{EligibleLineageAdmission, LineageAdmissionOutcome};

use super::*;

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
    /// admitted calls are retained for settlement. An ineligible call-shaped
    /// attempt at the current compatible action is recorded as non-progress
    /// and consumes one bounded recovery slot instead of returning an
    /// unchanged menu indefinitely.
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
    pub(in crate::machine) fn on_tool_batch_dispatched_with_admissions_and_targets(
        &mut self,
        calls: &[ToolCall],
        turn: usize,
        admissions: &[Option<LineageAdmissionOutcome>],
        invocation_targets: &[Option<InvocationTargetAdmission>],
    ) -> Vec<Option<ToolCallDenial>> {
        debug_assert_eq!(calls.len(), admissions.len());
        debug_assert_eq!(calls.len(), invocation_targets.len());
        let snapshot = self.recovery_admission_snapshot();
        let mut selected = BTreeSet::new();
        let mut admitted_actions = Vec::new();
        let mut admitted_count = 0u8;
        let mut local_rejections = Vec::new();
        let mut denials = Vec::with_capacity(calls.len());

        for ((call, admission), invocation_target) in
            calls.iter().zip(admissions).zip(invocation_targets)
        {
            let order = self.next_call_order;
            self.next_call_order = self.next_call_order.saturating_add(1);
            let mut denial = None;
            if call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
                let eligible = match admission.as_ref() {
                    Some(LineageAdmissionOutcome::Eligible(admission)) => Some(admission),
                    Some(LineageAdmissionOutcome::Ineligible(_)) | None => None,
                };
                let requested_gap = DecisionGap::from_call(call);
                let requested_action = DecisionGap::recovery_action_for_call(call);
                let recovery_gap = eligible
                    .and_then(DecisionGap::from_admission)
                    .or_else(|| admission.is_none().then_some(requested_gap).flatten());
                let recovery_action = eligible
                    .and_then(DecisionGap::recovery_action_for_admission)
                    .or_else(|| admission.is_none().then_some(requested_action).flatten());
                let admitted_root = eligible
                    .and_then(|admission| self.root_matching_admission(admission))
                    // Tests and non-codebase-memory compositions retain the
                    // legacy typed-call path without interpreting a selector.
                    .or_else(|| {
                        (admission.is_none())
                            .then(|| {
                                snapshot
                                    .as_ref()
                                    .map(|snapshot| snapshot.active_root.clone())
                            })
                            .flatten()
                    });
                let call_key = GraphCorrelationV1::target_digest(&call.id);

                if self.exploration != ExplorationStatus::Open {
                    let admissible = snapshot.as_ref().is_some_and(|snapshot| {
                        call_key.is_some()
                            && recovery_gap.is_some_and(|gap| {
                                snapshot.missing.contains(&gap)
                                    && recovery_action
                                        .is_some_and(|action| snapshot.compatible.contains(&action))
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
                        let gap = recovery_gap.expect("admissible recovery has a purpose");
                        selected.insert(gap);
                        admitted_actions.push(
                            recovery_action.expect("admissible recovery has a closed action"),
                        );
                        admitted_count = admitted_count.saturating_add(1);
                    } else {
                        denial = Some(snapshot.as_ref().map_or_else(
                            || self.graph_exploration_denial(),
                            |snapshot| snapshot.denial.clone(),
                        ));
                        if matches!(admission, Some(LineageAdmissionOutcome::Ineligible(_))) {
                            local_rejections.push((requested_gap, requested_action));
                        }
                    }
                }

                if denial.is_none() {
                    if let Some(call_key) = call_key {
                        self.calls.insert(
                            call_key,
                            PendingCodebaseCall {
                                turn,
                                order,
                                recovery_gap,
                                admitted_root,
                                admission_checked: admission.is_some(),
                            },
                        );
                    }
                }
            }
            if denial.is_none() && call.name == "read" {
                self.register_exact_read(call, turn, order, invocation_target.as_ref());
            }
            if denial.is_none()
                && self.blocks_invocation_mutation(&call.name, invocation_target.as_ref())
            {
                denial = Some(ToolCallDenial::DecisionAnchorMutation);
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

        let rejected_gaps = snapshot.as_ref().map_or_else(BTreeSet::new, |snapshot| {
            local_rejections
                .iter()
                .filter_map(|(gap, action)| {
                    gap.filter(|gap| {
                        snapshot.missing.contains(gap)
                            && !selected.contains(gap)
                            && action.is_some_and(|action| snapshot.compatible.contains(&action))
                    })
                })
                .collect::<BTreeSet<_>>()
        });
        if !rejected_gaps.is_empty() {
            if let Some(AnchorPhase::GapRecovery(recovery)) = self.phase.as_mut() {
                recovery.remaining = recovery
                    .remaining
                    .saturating_sub(u8::try_from(rejected_gaps.len()).unwrap_or(u8::MAX));
                self.denied_recovery_exhausted = recovery.remaining == 0;
            }
        }
        for (gap, action) in local_rejections {
            self.queue_local_denial_guidance(
                action,
                gap.is_some_and(|gap| rejected_gaps.contains(&gap)),
            );
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

    fn root_matching_admission(&self, admission: &EligibleLineageAdmission) -> Option<String> {
        let anchors = match self.phase.as_ref()? {
            AnchorPhase::Root(anchors)
            | AnchorPhase::Trail(anchors)
            | AnchorPhase::AwaitingExactRead(anchors) => anchors,
            AnchorPhase::Recovery(recovery) => &recovery.anchors,
            AnchorPhase::GapRecovery(recovery) => &recovery.anchors,
            AnchorPhase::Exhausted(_) => return None,
        };
        anchors
            .roots
            .keys()
            .find(|root| admission.matches_root(root))
            .cloned()
    }

    fn recovery_admission_snapshot(&self) -> Option<RecoveryAdmissionSnapshot> {
        if self.exploration != ExplorationStatus::GapRecovery {
            return None;
        }
        let AnchorPhase::GapRecovery(recovery) = self.phase.as_ref()? else {
            return None;
        };
        let active = recovery.anchors.roots.get(&recovery.active_root)?;
        Some(RecoveryAdmissionSnapshot {
            active_root: recovery.active_root.clone(),
            missing: active.evidence.missing_gaps(),
            compatible: active.evidence.compatible_actions(active),
            pending: self
                .calls
                .values()
                .filter_map(|call| call.recovery_gap)
                .collect(),
            remaining: recovery.remaining,
            denial: self.graph_exploration_denial(),
        })
    }
}

impl DecisionGap {
    pub(super) fn recovery_kind(self) -> GraphRecoveryEvidenceKindV1 {
        match self {
            Self::Trace => GraphRecoveryEvidenceKindV1::Trace,
            Self::Evidence(DecisionEvidenceKindV1::Implementation) => {
                GraphRecoveryEvidenceKindV1::Implementation
            }
            Self::Evidence(DecisionEvidenceKindV1::Caller) => GraphRecoveryEvidenceKindV1::Caller,
            Self::Evidence(DecisionEvidenceKindV1::FocusedTest) => {
                GraphRecoveryEvidenceKindV1::FocusedTest
            }
        }
    }

    fn recovery_action_for_admission(
        admission: &EligibleLineageAdmission,
    ) -> Option<GraphRecoveryActionV1> {
        if admission.recovery_purpose() == Some(DecisionEvidenceKindV1::FocusedTest) {
            return match admission.tool_kind() {
                GraphCorrelationToolV1::TracePath => {
                    Some(GraphRecoveryActionV1::focused_test_traversal())
                }
                GraphCorrelationToolV1::SearchGraph => {
                    Some(GraphRecoveryActionV1::focused_test_semantic_fallback())
                }
                GraphCorrelationToolV1::SearchCode | GraphCorrelationToolV1::GetCodeSnippet => None,
            };
        }
        DecisionGap::from_admission(admission)
            .map(|gap| GraphRecoveryActionV1::for_evidence(gap.recovery_kind()))
    }

    fn recovery_action_for_call(call: &ToolCall) -> Option<GraphRecoveryActionV1> {
        let gap = DecisionGap::from_call(call)?;
        if call.name == GraphCorrelationToolV1::TracePath.public_name()
            && gap == DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest)
        {
            Some(GraphRecoveryActionV1::focused_test_traversal())
        } else {
            Some(GraphRecoveryActionV1::for_evidence(gap.recovery_kind()))
        }
    }
}
