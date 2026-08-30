//! Immutable, root-local pre-provider recovery admission.

use crate::{EligibleLineageAdmission, LineageAdmissionOutcome, OpaqueRecoverySelectorReference};
use sha2::{Digest as _, Sha256};

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
        trace_recovery_selectors: &[Option<OpaqueRecoverySelectorReference>],
    ) -> Vec<Option<ToolCallDenial>> {
        debug_assert_eq!(calls.len(), admissions.len());
        debug_assert_eq!(calls.len(), invocation_targets.len());
        debug_assert_eq!(calls.len(), incomplete_graph_selectors.len());
        debug_assert_eq!(calls.len(), trace_recovery_selectors.len());
        let snapshot = self.staged_admission_snapshot();
        let mut selected = BTreeSet::new();
        let mut admitted_actions = Vec::new();
        let mut admitted_count = 0u8;
        let mut local_rejections = Vec::new();
        let mut local_readiness_deferrals = 0usize;
        let mut local_readiness_exhaustions = 0usize;
        let mut denials = Vec::with_capacity(calls.len());

        for (
            (((call, admission), invocation_target), incomplete_graph_selector),
            trace_recovery_selector,
        ) in calls
            .iter()
            .zip(admissions)
            .zip(invocation_targets)
            .zip(incomplete_graph_selectors)
            .zip(trace_recovery_selectors)
        {
            let order = self.next_call_order;
            self.next_call_order = self.next_call_order.saturating_add(1);
            let mut denial = None;
            if call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX)
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
                    let admissible = readiness_recheck_admissible
                        || snapshot.as_ref().is_some_and(|snapshot| {
                            !already_rejected
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
                        let gap = recovery_gap.expect("admissible recovery has a purpose");
                        selected.insert(gap);
                        if !readiness_recheck_admissible {
                            admitted_actions.push(
                                recovery_action.expect("admissible recovery has a closed action"),
                            );
                            admitted_count = admitted_count.saturating_add(1);
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
                                    self.rejected_recovery_tuples.insert(identity)
                                });
                            let recovery_selector = trace_recovery_selector
                                .as_ref()
                                .filter(|_| {
                                    requested_gap == Some(DecisionGap::Trace)
                                        && snapshot.as_ref().is_some_and(|snapshot| {
                                            requested_action.is_some_and(|action| {
                                                snapshot.compatible.contains(&action)
                                            })
                                        })
                                })
                                .cloned();
                            local_rejections.push((requested_action, excluded, recovery_selector));
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

        for _ in 0..local_readiness_exhaustions {
            self.enter_local_traversal_readiness_recovery();
            self.queue_local_denial_guidance(
                Some(GraphRecoveryActionV1::for_evidence(
                    GraphRecoveryEvidenceKindV1::Trace,
                )),
                true,
                None,
            );
        }
        for (action, excluded, recovery_selector) in local_rejections {
            self.queue_local_denial_guidance(action, excluded, recovery_selector.as_ref());
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

    fn root_matching_admission(&self, admission: &EligibleLineageAdmission) -> Option<String> {
        let anchors = match self.phase.as_ref()? {
            AnchorPhase::Root(anchors)
            | AnchorPhase::Trail(anchors)
            | AnchorPhase::EnabledComplete(anchors) => anchors,
            AnchorPhase::Recovery(recovery) => &recovery.anchors,
            AnchorPhase::GapRecovery(recovery) => &recovery.anchors,
            AnchorPhase::EnabledIncomplete(_) | AnchorPhase::ProviderUnavailable { .. } => {
                return None;
            }
        };
        anchors
            .roots
            .keys()
            .find(|root| admission.matches_root(root))
            .cloned()
    }

    fn staged_admission_snapshot(&self) -> Option<RecoveryAdmissionSnapshot> {
        let (active_root, active, remaining, denial) = match self.phase.as_ref()? {
            AnchorPhase::Root(anchors) | AnchorPhase::Trail(anchors)
                if self.exploration == ExplorationStatus::Open =>
            {
                let (active_root, active) = anchors.active_root()?;
                let details = GraphExplorationClosedV1::recoverable_without_actions(
                    active.evidence.missing_kinds(),
                    MAX_DECISION_GAP_RECOVERY_CALLS,
                );
                (
                    active_root.clone(),
                    active,
                    MAX_DECISION_GAP_RECOVERY_CALLS,
                    ToolCallDenial::GraphExplorationClosed(details),
                )
            }
            AnchorPhase::GapRecovery(recovery) => {
                let active = recovery.anchors.roots.get(&recovery.active_root)?;
                (
                    recovery.active_root.clone(),
                    active,
                    recovery.remaining,
                    self.graph_exploration_denial(),
                )
            }
            _ => return None,
        };
        let mut compatible = active.evidence.compatible_actions(active);
        let semantic_search = GraphRecoveryActionV1::focused_test_semantic_fallback();
        // Preserve the direct semantic action for a healthy route. If the
        // route already accumulated a bounded non-progress detour, retain the
        // legacy caller-to-test checkpoint so recovery does not silently skip
        // a provider result and shift every later selector.
        if self.non_progressing_batches > 0 && compatible.contains(&semantic_search) {
            let staged_traversal = GraphRecoveryActionV1::focused_test_traversal();
            if active.supports(staged_traversal) {
                compatible.insert(staged_traversal);
            }
        }
        Some(RecoveryAdmissionSnapshot {
            active_root,
            missing: active.evidence.missing_gaps(),
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
            }
            AnchorPhase::GapRecovery(mut recovery) => {
                recovery.remaining = recovery.remaining.saturating_sub(1);
                if recovery.remaining == 0 {
                    let evidence = recovery
                        .anchors
                        .roots
                        .get(&recovery.active_root)
                        .map(|anchor| anchor.evidence.clone())
                        .unwrap_or_default();
                    self.phase = Some(AnchorPhase::EnabledIncomplete(evidence));
                    self.exploration = ExplorationStatus::EnabledIncomplete;
                } else {
                    self.phase = Some(AnchorPhase::GapRecovery(recovery));
                    self.exploration = ExplorationStatus::GapRecovery;
                }
            }
            AnchorPhase::EnabledIncomplete(evidence) => {
                self.phase = Some(AnchorPhase::EnabledIncomplete(evidence));
                self.exploration = ExplorationStatus::EnabledIncomplete;
            }
            phase @ (AnchorPhase::Recovery(_)
            | AnchorPhase::EnabledComplete(_)
            | AnchorPhase::ProviderUnavailable { .. }) => {
                self.phase = Some(phase);
            }
        }
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
        if call.name == GraphCorrelationToolV1::SearchGraph.public_name()
            && call.arguments.get("query").is_some()
        {
            return Some(GraphRecoveryActionV1::focused_test_semantic_fallback());
        }
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
