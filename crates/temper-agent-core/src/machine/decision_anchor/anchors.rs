//! Bounded current-root forest construction and compatibility checks.

use super::*;

impl Anchor {
    fn from_output(turn: usize, lineage: &DecisionAnchorLineageV1) -> Self {
        Self {
            produced_turn: turn,
            result_target_kinds: lineage.result_target_kinds.iter().copied().collect(),
        }
    }

    fn is_consumable(&self) -> bool {
        !self.result_target_kinds.is_empty()
    }

    fn accepts(&self, call: &PendingCodebaseCall, lineage: &DecisionAnchorLineageV1) -> bool {
        call.turn > self.produced_turn && self.result_target_kinds.contains(&lineage.target_kind)
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
        let mut trace_root_turn: Option<usize> = None;
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
            if output.tool == GraphCorrelationToolV1::TracePath {
                trace_root_turn = Some(
                    trace_root_turn.map_or(finished.call.turn, |turn| turn.min(finished.call.turn)),
                );
            }
            if roots.len() >= MAX_DECISION_ANCHOR_ROOTS {
                // Overflowing roots are not partially usable: that would make
                // policy depend on an arbitrary provider-result subset.
                valid = false;
                continue;
            }
            match roots.entry(output.lineage.root_binding.clone()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Anchor::from_output(finished.call.turn, &output.lineage));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.produced_turn = existing.produced_turn.min(finished.call.turn);
                    existing
                        .result_target_kinds
                        .extend(output.lineage.result_target_kinds.iter().copied());
                }
            }
        }

        saw_root.then_some(Self {
            roots,
            valid,
            latest_produced_turn,
            trace_root_turn,
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
        self.trace_root_turn = match (self.trace_root_turn, next.trace_root_turn) {
            (Some(current), Some(next)) => Some(current.min(next)),
            (Some(turn), None) | (None, Some(turn)) => Some(turn),
            (None, None) => None,
        };
        for (root_binding, next_root) in next.roots {
            match self.roots.entry(root_binding) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(next_root);
                    progressed = true;
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.produced_turn = existing.produced_turn.min(next_root.produced_turn);
                    let before = existing.result_target_kinds.len();
                    existing
                        .result_target_kinds
                        .extend(next_root.result_target_kinds);
                    progressed |= existing.result_target_kinds.len() > before;
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

    pub(super) fn supports(&self, gap: DecisionGap) -> bool {
        let target_kind = match gap {
            DecisionGap::Trace => DecisionAnchorTargetKindV1::FunctionName,
            DecisionGap::Evidence(_) => DecisionAnchorTargetKindV1::QualifiedName,
        };
        self.valid
            && self
                .roots
                .values()
                .any(|root| root.result_target_kinds.contains(&target_kind))
    }

    pub(super) fn accepts(
        &self,
        call: &PendingCodebaseCall,
        lineage: &DecisionAnchorLineageV1,
    ) -> bool {
        self.valid
            && lineage.stage == DecisionAnchorLineageStageV1::CarryForward
            && self
                .roots
                .get(&lineage.root_binding)
                .is_some_and(|root| root.accepts(call, lineage))
    }

    pub(super) fn contains_producer_turn(&self, call: &PendingCodebaseCall) -> bool {
        call.turn <= self.latest_produced_turn
    }
}
