//! Bounded opaque authority for post-source exact reads and mutations.

use super::*;

pub(super) const MAX_EXACT_TARGET_AUTHORITIES: usize = 64;

pub(super) struct PendingExactRead {
    sources: Vec<SourceTargetAuthority>,
    dispatched_turn: usize,
    dispatched_order: u64,
    dispatched_after_batch: u64,
}

#[derive(Clone)]
pub(super) struct ExactReadAuthority {
    pub(super) target: EligibleWorkspaceTarget,
    pub(super) root_binding: String,
    source_completed_turn: usize,
    source_completed_order: u64,
    source_completed_batch: u64,
    read_dispatched_turn: usize,
    read_dispatched_order: u64,
    read_dispatched_after_batch: u64,
}

impl ExactReadAuthority {
    pub(super) fn authorizes(
        &self,
        target: &EligibleWorkspaceTarget,
        anchors: &AnchorForest,
    ) -> bool {
        self.target.matches(target)
            && anchors.root_has_complete_evidence(&self.root_binding)
            && self.read_dispatched_after_batch >= self.source_completed_batch
            && (self.read_dispatched_turn > self.source_completed_turn
                || self.read_dispatched_turn == self.source_completed_turn
                    && self.read_dispatched_order > self.source_completed_order)
    }

    pub(super) fn same_read(&self, other: &Self) -> bool {
        self.target.matches(&other.target)
            && self.root_binding == other.root_binding
            && self.source_completed_turn == other.source_completed_turn
            && self.source_completed_order == other.source_completed_order
            && self.source_completed_batch == other.source_completed_batch
            && self.read_dispatched_turn == other.read_dispatched_turn
            && self.read_dispatched_order == other.read_dispatched_order
            && self.read_dispatched_after_batch == other.read_dispatched_after_batch
    }
}

#[derive(Clone)]
pub(super) struct SourceTargetAuthority {
    target: EligibleWorkspaceTarget,
    root_binding: String,
    evidence_kind: DecisionEvidenceKindV1,
    completed_turn: usize,
    completed_order: u64,
    completed_batch: u64,
}

impl DecisionAnchorState {
    pub(super) fn record_source_authority(
        &mut self,
        root_binding: &str,
        call: &PendingCodebaseCall,
        evidence_kind: DecisionEvidenceKindV1,
        outcome: Option<&TargetAdmissionOutcome>,
    ) {
        let Some(TargetAdmissionOutcome::Eligible(target)) = outcome else {
            return;
        };
        self.exact_read_authorities.retain(|authority| {
            authority.root_binding != root_binding || !authority.target.matches(target)
        });
        if let Some(existing) = self.source_authorities.iter_mut().find(|authority| {
            authority.root_binding == root_binding
                && authority.evidence_kind == evidence_kind
                && authority.target.matches(target)
        }) {
            existing.completed_turn = call.turn;
            existing.completed_order = call.order;
            existing.completed_batch = self.settled_batches;
            return;
        }
        if self.source_authorities.len() < MAX_EXACT_TARGET_AUTHORITIES {
            self.source_authorities.push(SourceTargetAuthority {
                target: target.clone(),
                root_binding: root_binding.to_string(),
                evidence_kind,
                completed_turn: call.turn,
                completed_order: call.order,
                completed_batch: self.settled_batches,
            });
        }
    }

    pub(super) fn replace_implementation_source_authority(
        &mut self,
        root_binding: &str,
        call: &PendingCodebaseCall,
        outcome: Option<&TargetAdmissionOutcome>,
    ) {
        self.source_authorities
            .retain(|authority| authority.root_binding != root_binding);
        self.pending_exact_reads.retain(|_, pending| {
            pending
                .sources
                .iter()
                .all(|source| source.root_binding != root_binding)
        });
        self.exact_read_authorities
            .retain(|authority| authority.root_binding != root_binding);
        self.record_source_authority(
            root_binding,
            call,
            DecisionEvidenceKindV1::Implementation,
            outcome,
        );
    }

    pub(super) fn correction_inspection_blocks_exact_read(
        &self,
        admission: Option<&InvocationTargetAdmission>,
    ) -> bool {
        if self.implementation_correction_inspection_completed {
            return false;
        }
        let Some(AnchorPhase::EnabledComplete(anchors)) = self.phase.as_ref() else {
            return false;
        };
        let Some((root_binding, implementation)) = anchors.implementation_root() else {
            return false;
        };
        if !implementation.evidence.implementation_correction_available
            || implementation.evidence.implementation_authority_corrected
        {
            return false;
        }
        let Some(InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(target))) =
            admission
        else {
            return false;
        };
        self.source_authorities.iter().any(|source| {
            source.root_binding == *root_binding
                && source.evidence_kind == DecisionEvidenceKindV1::Implementation
                && source.target.matches(target)
        })
    }

    pub(super) fn register_exact_read(
        &mut self,
        call: &ToolCall,
        turn: usize,
        order: u64,
        admission: Option<&InvocationTargetAdmission>,
    ) {
        self.register_companion_read(call, admission);
        if matches!(self.phase, Some(AnchorPhase::ProviderUnavailable)) {
            let Some(InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(target))) =
                admission
            else {
                return;
            };
            if let Some(call_key) = GraphCorrelationV1::target_digest(&call.id) {
                self.pending_conventional_reads
                    .insert(call_key, target.clone());
            }
            return;
        }
        let Some(AnchorPhase::EnabledComplete(anchors)) = self.phase.as_ref() else {
            return;
        };
        let Some(InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(target))) =
            admission
        else {
            return;
        };
        let sources = self
            .source_authorities
            .iter()
            .filter(|source| {
                anchors.root_has_complete_evidence(&source.root_binding)
                    && source.evidence_kind == DecisionEvidenceKindV1::Implementation
                    && source.target.matches(target)
                    && source.completed_batch <= self.settled_batches
                    && (turn > source.completed_turn
                        || turn == source.completed_turn && order > source.completed_order)
            })
            .cloned()
            .collect::<Vec<_>>();
        if sources.is_empty() || self.pending_exact_reads.len() >= MAX_EXACT_TARGET_AUTHORITIES {
            return;
        }
        if let Some(call_key) = GraphCorrelationV1::target_digest(&call.id) {
            self.implementation_authority_exercised = true;
            self.pending_exact_reads.insert(
                call_key,
                PendingExactRead {
                    sources,
                    dispatched_turn: turn,
                    dispatched_order: order,
                    dispatched_after_batch: self.settled_batches,
                },
            );
        }
    }

    pub(super) fn settle_exact_reads(&mut self, completed: &[SettledToolCall<'_>]) {
        self.settle_companion_reads(completed);
        for (id, name, output, _, succeeded) in completed {
            let Some(call_key) = GraphCorrelationV1::target_digest(id) else {
                continue;
            };
            let Some(target) = self.pending_conventional_reads.remove(&call_key) else {
                continue;
            };
            if *name == "read"
                && *succeeded
                && !output.is_error
                && !self
                    .conventional_read_authorities
                    .iter()
                    .any(|current| current.matches(&target))
                && self.conventional_read_authorities.len() < MAX_EXACT_TARGET_AUTHORITIES
            {
                self.conventional_read_authorities.push(target);
            }
        }
        let mut settled_roots = BTreeSet::new();
        for (id, name, output, _, succeeded) in completed {
            let Some(call_key) = GraphCorrelationV1::target_digest(id) else {
                continue;
            };
            let Some(pending) = self.pending_exact_reads.remove(&call_key) else {
                continue;
            };
            if *name != "read" || !succeeded || output.is_error {
                continue;
            }
            for source in pending.sources {
                let still_current = self.source_authorities.iter().any(|current| {
                    current.root_binding == source.root_binding
                        && current.target.matches(&source.target)
                        && current.completed_turn == source.completed_turn
                        && current.completed_order == source.completed_order
                        && current.completed_batch == source.completed_batch
                });
                if !still_current {
                    continue;
                }
                let settled_root = source.root_binding.clone();
                if let Some(existing) = self.exact_read_authorities.iter_mut().find(|authority| {
                    authority.root_binding == source.root_binding
                        && authority.target.matches(&source.target)
                }) {
                    existing.read_dispatched_turn = pending.dispatched_turn;
                    existing.read_dispatched_order = pending.dispatched_order;
                    existing.read_dispatched_after_batch = pending.dispatched_after_batch;
                } else if self.exact_read_authorities.len() < MAX_EXACT_TARGET_AUTHORITIES {
                    self.exact_read_authorities.push(ExactReadAuthority {
                        target: source.target,
                        root_binding: source.root_binding,
                        source_completed_turn: source.completed_turn,
                        source_completed_order: source.completed_order,
                        source_completed_batch: source.completed_batch,
                        read_dispatched_turn: pending.dispatched_turn,
                        read_dispatched_order: pending.dispatched_order,
                        read_dispatched_after_batch: pending.dispatched_after_batch,
                    });
                }
                settled_roots.insert(settled_root);
            }
        }
        if settled_roots.is_empty() {
            return;
        }
        if let Some(AnchorPhase::EnabledComplete(anchors)) = self.phase.as_mut() {
            for root in &settled_roots {
                if let Some(anchor) = anchors.roots.get_mut(root) {
                    anchor
                        .evidence
                        .retain_provisional_implementation_authority();
                }
            }
        }
        self.source_authorities
            .retain(|source| !settled_roots.contains(&source.root_binding));
        self.pending_exact_reads.retain(|_, pending| {
            pending
                .sources
                .iter()
                .all(|source| !settled_roots.contains(&source.root_binding))
        });
        self.implementation_correction_attempted = true;
        self.implementation_correction_inspection_completed = false;
    }

    pub(super) fn mutation_targets_authorized(
        &self,
        anchors: &AnchorForest,
        admission: Option<&InvocationTargetAdmission>,
    ) -> bool {
        let Some(InvocationTargetAdmission::Mutation(targets)) = admission else {
            return false;
        };
        !targets.is_empty()
            && targets.iter().all(|outcome| {
                let TargetAdmissionOutcome::Eligible(target) = outcome else {
                    return false;
                };
                self.exact_read_authorities
                    .iter()
                    .any(|authority| authority.authorizes(target, anchors))
                    || self.companion_target_authorized(target, anchors)
            })
    }

    pub(super) fn conventional_mutation_targets_authorized(
        &self,
        admission: Option<&InvocationTargetAdmission>,
    ) -> bool {
        let Some(InvocationTargetAdmission::Mutation(targets)) = admission else {
            return false;
        };
        !targets.is_empty()
            && targets.iter().all(|outcome| {
                let TargetAdmissionOutcome::Eligible(target) = outcome else {
                    return false;
                };
                self.conventional_read_authorities
                    .iter()
                    .any(|authority| authority.matches(target))
            })
    }

    pub(super) fn blocks_invocation_mutation(
        &mut self,
        name: &str,
        admission: Option<&InvocationTargetAdmission>,
    ) -> bool {
        if !self.mutation_tools.contains(name) {
            return false;
        }
        let blocked = match admission {
            Some(
                InvocationTargetAdmission::SourceNeutralProcess
                | InvocationTargetAdmission::ControlPlane,
            ) => false,
            Some(InvocationTargetAdmission::Mutation(_)) => {
                self.phase.as_ref().is_none_or(|phase| match phase {
                    AnchorPhase::EnabledComplete(anchors) => {
                        !self.mutation_targets_authorized(anchors, admission)
                    }
                    AnchorPhase::Root(_)
                    | AnchorPhase::Trail(_)
                    | AnchorPhase::Recovery(_)
                    | AnchorPhase::GapRecovery(_)
                    | AnchorPhase::EnabledIncomplete(_) => true,
                    AnchorPhase::ProviderUnavailable => {
                        !self.conventional_mutation_targets_authorized(admission)
                    }
                })
            }
            Some(InvocationTargetAdmission::PatchCreation {
                existing,
                creations,
            }) => {
                name != "apply_patch"
                    || !self.patch_creation_authorized(existing, !creations.is_empty())
            }
            Some(InvocationTargetAdmission::Read(_) | InvocationTargetAdmission::Ineligible(_))
            | None => true,
        };
        if !blocked
            && matches!(
                admission,
                Some(
                    InvocationTargetAdmission::Mutation(_)
                        | InvocationTargetAdmission::PatchCreation { .. }
                )
            )
        {
            self.implementation_authority_exercised = true;
        }
        blocked
    }

    #[cfg(test)]
    pub(in crate::machine) fn blocks_mutation(&mut self, name: &str) -> bool {
        self.blocks_invocation_mutation(name, None)
    }
}
