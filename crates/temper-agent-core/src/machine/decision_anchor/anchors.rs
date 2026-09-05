//! Bounded current-root forest construction and compatibility checks.

use super::*;

impl DecisionAnchorState {
    pub(in crate::machine) fn active_root_binding(&self) -> Option<&str> {
        match self.phase.as_ref()? {
            AnchorPhase::Root(anchors) | AnchorPhase::Trail(anchors) => anchors
                .active_selection(&BTreeSet::new())
                .map(|(binding, _)| binding.as_str()),
            AnchorPhase::EnabledComplete(anchors) => anchors
                .implementation_root()
                .map(|(binding, _)| binding.as_str()),
            AnchorPhase::Recovery(recovery) => recovery
                .anchors
                .active_selection(&BTreeSet::new())
                .map(|(binding, _)| binding.as_str()),
            AnchorPhase::GapRecovery(recovery) => Some(recovery.active_root.as_str()),
            AnchorPhase::EnabledIncomplete(_) | AnchorPhase::ProviderUnavailable => None,
        }
    }
}

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
            exact_graph_narrowing_selected: false,
            evidence,
        }
    }

    fn is_consumable(&self) -> bool {
        !self.result_target_kinds.is_empty()
    }

    pub(super) fn supports(&self, action: GraphRecoveryActionV1) -> bool {
        self.result_target_kinds.contains(&action.selector_kind)
    }

    pub(super) fn accepts(
        &self,
        call: &PendingCodebaseCall,
        lineage: &DecisionAnchorLineageV1,
    ) -> bool {
        call.turn > self.produced_turn
            && (self.result_target_kinds.contains(&lineage.target_kind)
                || (lineage.target_kind == DecisionAnchorTargetKindV1::NamePattern
                    && self
                        .result_target_kinds
                        .contains(&DecisionAnchorTargetKindV1::FunctionName))
                || (lineage.target_kind == DecisionAnchorTargetKindV1::QualifiedNamePattern
                    && self
                        .result_target_kinds
                        .contains(&DecisionAnchorTargetKindV1::QualifiedName)))
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
                    existing.exact_graph_narrowing_selected |= matches!(
                        output.lineage.target_kind,
                        DecisionAnchorTargetKindV1::NamePattern
                            | DecisionAnchorTargetKindV1::QualifiedNamePattern
                    );
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
                    existing.exact_graph_narrowing_selected |=
                        next_root.exact_graph_narrowing_selected;
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
        self.roots
            .iter()
            .any(|(implementation_binding, implementation)| {
                implementation.evidence.implementation_is_complete()
                    && self.roots.iter().any(|(focused_binding, focused)| {
                        (focused_binding != implementation_binding || self.roots.len() == 1)
                            && focused.evidence.focused_test_is_complete()
                    })
            })
    }

    pub(super) fn root_has_complete_evidence(&self, root_binding: &str) -> bool {
        self.roots.get(root_binding).is_some_and(|root| {
            root.evidence.implementation_is_complete()
                && self.roots.iter().any(|(focused_binding, focused)| {
                    (focused_binding != root_binding || self.roots.len() == 1)
                        && focused.evidence.focused_test_is_complete()
                })
        })
    }

    pub(super) fn active_evidence(&self) -> SourceEvidence {
        self.active_selection(&BTreeSet::new())
            .and_then(|(binding, _)| self.roots.get(binding))
            .or_else(|| self.implementation_root().map(|(_, root)| root))
            .map(|root| root.evidence.clone())
            .unwrap_or_default()
    }

    pub(super) fn recovery_selection(&self) -> Option<(String, RecoveryRoute)> {
        self.active_selection(&BTreeSet::new())
            .map(|(binding, route)| (binding.clone(), route))
    }

    pub(super) fn recovery_selection_excluding(
        &self,
        exhausted_routes: &BTreeSet<(String, RecoveryRoute)>,
    ) -> Option<(String, RecoveryRoute)> {
        self.active_selection(exhausted_routes)
            .map(|(binding, route)| (binding.clone(), route))
    }

    pub(super) fn mark_parallel_recovery(&mut self, active_root: &str, route: RecoveryRoute) {
        if route == RecoveryRoute::Implementation
            && self.roots.iter().any(|(binding, root)| {
                binding != active_root && root.evidence.focused_test_is_complete()
            })
        {
            let active = self
                .roots
                .get_mut(active_root)
                .expect("the selected recovery root remains installed");
            active.evidence.trace_before_implementation = true;
        }
    }

    pub(super) fn has_compatible_actions(&self) -> bool {
        self.active_selection(&BTreeSet::new()).is_some()
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
            .or_else(|| {
                self.active_selection(&BTreeSet::new())
                    .and_then(|(binding, _)| self.roots.get(binding))
            });
        root.is_some_and(|root| root.evidence.expects(gap, tool))
    }

    pub(super) fn active_selection(
        &self,
        exhausted_routes: &BTreeSet<(String, RecoveryRoute)>,
    ) -> Option<(&String, RecoveryRoute)> {
        if let Some((binding, _)) = self
            .ranked_implementation_roots()
            .find(|(binding, root)| {
                !exhausted_routes.contains(&(binding.to_string(), RecoveryRoute::Implementation))
                    && (!root
                        .evidence
                        .compatible_actions(root, RecoveryRoute::Implementation)
                        .is_empty()
                        || root.evidence.implementation_is_complete())
            })
            .filter(|(_, root)| !root.evidence.implementation_is_complete())
        {
            return Some((binding, RecoveryRoute::Implementation));
        }

        let (implementation_binding, implementation) = self.implementation_root()?;
        if !implementation.evidence.implementation_is_complete() {
            return None;
        }
        self.ranked_focused_test_roots(implementation_binding)
            .find(|(binding, root)| {
                !exhausted_routes.contains(&(binding.to_string(), RecoveryRoute::FocusedTest))
                    && !root.evidence.focused_test_is_complete()
                    && !root
                        .evidence
                        .compatible_actions(root, RecoveryRoute::FocusedTest)
                        .is_empty()
            })
            .map(|(binding, _)| (binding, RecoveryRoute::FocusedTest))
    }

    pub(super) fn missing_kinds(
        &self,
        active_root: &str,
        route: RecoveryRoute,
    ) -> Vec<GraphRecoveryEvidenceKindV1> {
        let mut missing = self
            .roots
            .get(active_root)
            .map_or_else(Vec::new, |root| root.evidence.missing_kinds(route));
        if route == RecoveryRoute::Implementation
            && self
                .roots
                .get(active_root)
                .is_some_and(|root| !root.evidence.focused_test_is_complete())
        {
            missing.push(GraphRecoveryEvidenceKindV1::FocusedTest);
            missing.sort();
            missing.dedup();
        }
        missing
    }

    pub(super) fn implementation_root(&self) -> Option<(&String, &Anchor)> {
        self.ranked_implementation_roots()
            .find(|(_, root)| root.evidence.implementation_is_complete())
            .or_else(|| self.ranked_implementation_roots().next())
    }

    fn ranked_implementation_roots(&self) -> impl Iterator<Item = (&String, &Anchor)> {
        let mut roots = self.roots.iter().collect::<Vec<_>>();
        roots.sort_by(|(left_binding, left), (right_binding, right)| {
            right
                .evidence
                .implementation_progress_count()
                .cmp(&left.evidence.implementation_progress_count())
                .then_with(|| left.produced_order.cmp(&right.produced_order))
                .then_with(|| left_binding.cmp(right_binding))
        });
        roots.into_iter()
    }

    fn ranked_focused_test_roots<'a>(
        &'a self,
        implementation_binding: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a Anchor)> {
        let mut roots = self.roots.iter().collect::<Vec<_>>();
        roots.sort_by(|(left_binding, left), (right_binding, right)| {
            let left_same = left_binding.as_str() == implementation_binding;
            let right_same = right_binding.as_str() == implementation_binding;
            left_same
                .cmp(&right_same)
                .then_with(|| {
                    right
                        .evidence
                        .focused_test_is_complete()
                        .cmp(&left.evidence.focused_test_is_complete())
                })
                .then_with(|| left.produced_order.cmp(&right.produced_order))
                .then_with(|| left_binding.cmp(right_binding))
        });
        roots.into_iter()
    }
}
