//! Immutable, root-local pre-provider recovery admission.

use crate::{EligibleLineageAdmission, LineageAdmissionOutcome};

use super::*;

struct RecoveryAdmissionSnapshot {
    active_root: String,
    missing: BTreeSet<DecisionGap>,
    compatible: BTreeSet<DecisionGap>,
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
    /// admitted calls are retained for settlement, so every other call stops
    /// before registry or provider execution without consuming allowance.
    pub(in crate::machine) fn on_tool_batch_dispatched_with_admissions(
        &mut self,
        calls: &[ToolCall],
        turn: usize,
        admissions: &[Option<LineageAdmissionOutcome>],
    ) -> Vec<Option<ToolCallDenial>> {
        debug_assert_eq!(calls.len(), admissions.len());
        let snapshot = self.recovery_admission_snapshot();
        let mut selected = BTreeSet::new();
        let mut admitted_count = 0u8;
        let mut denials = Vec::with_capacity(calls.len());

        for (call, admission) in calls.iter().zip(admissions) {
            let order = self.next_call_order;
            self.next_call_order = self.next_call_order.saturating_add(1);
            let mut denial = None;
            if call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
                let eligible = match admission.as_ref() {
                    Some(LineageAdmissionOutcome::Eligible(admission)) => Some(admission),
                    Some(LineageAdmissionOutcome::Ineligible(_)) | None => None,
                };
                let recovery_gap = eligible.and_then(DecisionGap::from_admission).or_else(|| {
                    admission
                        .is_none()
                        .then(|| DecisionGap::from_call(call))
                        .flatten()
                });
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
                                    && snapshot.compatible.contains(&gap)
                                    && !snapshot.pending.contains(&gap)
                                    && !selected.contains(&gap)
                                    && admitted_count < snapshot.remaining
                            })
                            && admitted_root.as_deref() == Some(snapshot.active_root.as_str())
                            && eligible.is_none_or(|admission| {
                                recovery_gap.is_some_and(|gap| gap.matches_admission(admission))
                                    && admission.matches_root(&snapshot.active_root)
                            })
                    });
                    if admissible {
                        let gap = recovery_gap.expect("admissible recovery has a purpose");
                        selected.insert(gap);
                        admitted_count = admitted_count.saturating_add(1);
                    } else {
                        denial = Some(snapshot.as_ref().map_or_else(
                            || self.graph_exploration_denial(),
                            |snapshot| snapshot.denial.clone(),
                        ));
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
                            },
                        );
                    }
                }
            }
            if denial.is_none() && self.blocks_mutation(&call.name) {
                denial = Some(ToolCallDenial::DecisionAnchorMutation);
            }
            denials.push(denial);
        }

        if admitted_count > 0 {
            if let Some(AnchorPhase::GapRecovery(recovery)) = self.phase.as_mut() {
                recovery.remaining = recovery.remaining.saturating_sub(admitted_count);
            }
        }
        denials
    }

    fn root_matching_admission(&self, admission: &EligibleLineageAdmission) -> Option<String> {
        let anchors = match self.phase.as_ref()? {
            AnchorPhase::Root(anchors) | AnchorPhase::Trail(anchors) => anchors,
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
            compatible: active.evidence.compatible_gaps(active),
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

    fn matches_admission(self, admission: &EligibleLineageAdmission) -> bool {
        let action = GraphRecoveryActionV1::for_evidence(self.recovery_kind());
        admission.tool_kind() == action.tool
            && admission.selector_kind() == action.selector_kind
            && admission.evidence_purpose()
                == match self {
                    Self::Trace => None,
                    Self::Evidence(kind) => Some(kind),
                }
    }
}
