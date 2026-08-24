//! Bounded current-root forest construction and compatibility checks.

use super::*;

impl Anchor {
    fn from_output(call: &PendingCodebaseCall, output: &AnchorOutput) -> Self {
        let mut evidence = SourceEvidence::default();
        if output.tool == GraphCorrelationToolV1::TracePath
            && output.lineage.caller_discovery.is_some()
        {
            evidence.record_trace(call.turn);
            evidence.record_caller_discovery(output.lineage.caller_discovery);
        }
        evidence.record_focused_test_discovery(output.tool, output.lineage.focused_test_discovery);
        Self {
            produced_turn: call.turn,
            produced_order: call.order,
            result_target_kinds: output.lineage.result_target_kinds.iter().copied().collect(),
            evidence,
        }
    }

    fn is_consumable(&self) -> bool {
        !self.result_target_kinds.is_empty()
    }

    pub(super) fn supports(&self, action: GraphRecoveryActionV1) -> bool {
        action == GraphRecoveryActionV1::focused_test_semantic_fallback()
            || self.result_target_kinds.contains(&action.selector_kind)
    }

    pub(super) fn accepts(
        &self,
        call: &PendingCodebaseCall,
        lineage: &DecisionAnchorLineageV1,
    ) -> bool {
        call.turn > self.produced_turn
            && (self.result_target_kinds.contains(&lineage.target_kind)
                || (call.recovery_gap
                    == Some(DecisionGap::Evidence(DecisionEvidenceKindV1::FocusedTest))
                    && lineage.target_kind == DecisionAnchorTargetKindV1::GraphQuery))
    }
}

impl AnchorForest {
    pub(super) fn from_finished(
        finished: &[FinishedCodebaseCall<'_>],
        after_turn: Option<usize>,
    ) -> Option<Self> {
        let mut roots = BTreeMap::new();
        let mut valid = true;
        let mut latest_produced_turn = 0;
        let mut saw_root = false;

        for finished in finished {
            let Some(output) = anchor_output(finished.name, finished.output) else {
                continue;
            };
            if output.lineage.stage != DecisionAnchorLineageStageV1::Root
                || after_turn.is_some_and(|turn| finished.call.turn <= turn)
            {
                continue;
            }
            saw_root = true;
            latest_produced_turn = latest_produced_turn.max(finished.call.turn);
            if roots.len() >= MAX_DECISION_ANCHOR_ROOTS {
                valid = false;
                continue;
            }
            match roots.entry(output.lineage.root_binding.clone()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Anchor::from_output(&finished.call, &output));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.produced_turn = existing.produced_turn.min(finished.call.turn);
                    existing.produced_order = existing.produced_order.min(finished.call.order);
                    existing
                        .result_target_kinds
                        .extend(output.lineage.result_target_kinds.iter().copied());
                    if output.tool == GraphCorrelationToolV1::TracePath {
                        existing.evidence.record_trace(finished.call.turn);
                    }
                }
            }
        }

        saw_root.then_some(Self {
            roots,
            valid,
            latest_produced_turn,
        })
    }

    pub(super) fn merge_limited(&mut self, next: Self, remaining_roots: usize) -> RootMerge {
        if !next.valid {
            self.valid = false;
            return RootMerge::NoProgress;
        }
        let new_roots = next
            .roots
            .keys()
            .filter(|root| !self.roots.contains_key(*root))
            .count();
        if new_roots > remaining_roots
            || self.roots.len().saturating_add(new_roots) > MAX_DECISION_ANCHOR_ROOTS
        {
            return RootMerge::LimitExceeded;
        }
        let mut progressed = false;
        self.latest_produced_turn = self.latest_produced_turn.max(next.latest_produced_turn);
        for (root_binding, next_root) in next.roots {
            match self.roots.entry(root_binding) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(next_root);
                    progressed = true;
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.produced_turn = existing.produced_turn.min(next_root.produced_turn);
                    existing.produced_order = existing.produced_order.min(next_root.produced_order);
                    let kinds_before = existing.result_target_kinds.len();
                    let evidence_before = existing.evidence.progress_count();
                    existing
                        .result_target_kinds
                        .extend(next_root.result_target_kinds);
                    existing.evidence.merge(next_root.evidence);
                    progressed |= existing.result_target_kinds.len() > kinds_before
                        || existing.evidence.progress_count() > evidence_before;
                }
            }
        }
        if progressed {
            RootMerge::Progress(new_roots)
        } else {
            RootMerge::NoProgress
        }
    }

    pub(super) fn is_consumable(&self) -> bool {
        self.valid && self.roots.values().any(Anchor::is_consumable)
    }

    pub(super) fn accepted_root(
        &self,
        call: &PendingCodebaseCall,
        lineage: &DecisionAnchorLineageV1,
    ) -> Option<String> {
        (self.valid && lineage.stage == DecisionAnchorLineageStageV1::CarryForward)
            .then(|| self.roots.get(&lineage.root_binding))
            .flatten()
            .filter(|root| root.accepts(call, lineage))
            .map(|_| lineage.root_binding.clone())
    }

    pub(super) fn contains_producer_turn(&self, call: &PendingCodebaseCall) -> bool {
        call.turn <= self.latest_produced_turn
    }

    pub(super) fn has_any_evidence(&self) -> bool {
        self.roots
            .values()
            .any(|root| root.evidence.progress_count() > 0)
    }

    pub(super) fn has_complete_evidence(&self) -> bool {
        self.roots.values().any(|root| root.evidence.is_complete())
    }

    pub(super) fn root_has_complete_evidence(&self, root_binding: &str) -> bool {
        self.roots
            .get(root_binding)
            .is_some_and(|root| root.evidence.is_complete())
    }

    pub(super) fn active_evidence(&self) -> SourceEvidence {
        self.active_root()
            .map(|(_, root)| root.evidence.clone())
            .unwrap_or_default()
    }

    /// Selects one recoverable root by actual typed progress, then by the
    /// wrapper-independent stable call order which first produced that root.
    pub(super) fn recovery_root_binding(&self) -> Option<String> {
        self.ranked_roots()
            .next()
            .filter(|(_, root)| !root.evidence.compatible_actions(root).is_empty())
            .map(|(binding, _)| binding.clone())
    }

    pub(super) fn active_has_compatible_actions(&self) -> bool {
        self.active_root()
            .is_some_and(|(_, root)| !root.evidence.compatible_actions(root).is_empty())
    }

    pub(super) fn expected_for_call(
        &self,
        call: &PendingCodebaseCall,
        gap: Option<DecisionGap>,
        tool: Option<GraphCorrelationToolV1>,
    ) -> bool {
        let root = call
            .admitted_root
            .as_ref()
            .and_then(|binding| self.roots.get(binding))
            .or_else(|| self.active_root().map(|(_, root)| root));
        root.is_some_and(|root| root.evidence.expects(gap, tool))
    }

    fn active_root(&self) -> Option<(&String, &Anchor)> {
        self.ranked_roots().next()
    }

    fn ranked_roots(&self) -> impl Iterator<Item = (&String, &Anchor)> {
        let mut roots = self.roots.iter().collect::<Vec<_>>();
        roots.sort_by(|(left_binding, left), (right_binding, right)| {
            right
                .evidence
                .progress_count()
                .cmp(&left.evidence.progress_count())
                .then_with(|| left.produced_order.cmp(&right.produced_order))
                .then_with(|| left_binding.cmp(right_binding))
        });
        roots.into_iter()
    }
}
