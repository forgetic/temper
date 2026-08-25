//! Bounded opaque authority for post-source exact reads and mutations.

use super::*;

impl DecisionAnchorState {
    pub(super) fn record_source_authority(
        &mut self,
        root_binding: &str,
        call: &PendingCodebaseCall,
        outcome: Option<&TargetAdmissionOutcome>,
    ) {
        let Some(TargetAdmissionOutcome::Eligible(target)) = outcome else {
            return;
        };
        self.exact_read_authorities.retain(|authority| {
            authority.root_binding != root_binding || !authority.target.matches(target)
        });
        if let Some(existing) = self.source_authorities.iter_mut().find(|authority| {
            authority.root_binding == root_binding && authority.target.matches(target)
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
                completed_turn: call.turn,
                completed_order: call.order,
                completed_batch: self.settled_batches,
            });
        }
    }

    pub(super) fn register_exact_read(
        &mut self,
        call: &ToolCall,
        turn: usize,
        order: u64,
        admission: Option<&InvocationTargetAdmission>,
    ) {
        if matches!(self.phase, Some(AnchorPhase::ConventionalFallback(_))) {
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
        let Some(InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(target))) =
            admission
        else {
            return;
        };
        let sources = self
            .source_authorities
            .iter()
            .filter(|source| {
                source.target.matches(target)
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
            }
        }
    }

    fn mutation_targets_authorized(
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
                self.exact_read_authorities.iter().any(|authority| {
                    authority.target.matches(target)
                        && anchors.root_has_complete_evidence(&authority.root_binding)
                        && authority.read_dispatched_after_batch >= authority.source_completed_batch
                        && (authority.read_dispatched_turn > authority.source_completed_turn
                            || authority.read_dispatched_turn == authority.source_completed_turn
                                && authority.read_dispatched_order
                                    > authority.source_completed_order)
                })
            })
    }

    fn conventional_mutation_targets_authorized(
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
        &self,
        name: &str,
        admission: Option<&InvocationTargetAdmission>,
    ) -> bool {
        if !self.mutation_tools.contains(name) {
            return false;
        }
        match admission {
            Some(
                InvocationTargetAdmission::SourceNeutralProcess
                | InvocationTargetAdmission::ControlPlane,
            ) => false,
            Some(InvocationTargetAdmission::Mutation(_)) => {
                self.phase.as_ref().is_some_and(|phase| match phase {
                    AnchorPhase::AwaitingExactRead(anchors) => {
                        !self.mutation_targets_authorized(anchors, admission)
                    }
                    AnchorPhase::Root(_)
                    | AnchorPhase::Trail(_)
                    | AnchorPhase::Recovery(_)
                    | AnchorPhase::GapRecovery(_)
                    | AnchorPhase::Exhausted(_) => true,
                    AnchorPhase::ConventionalFallback(_) => {
                        !self.conventional_mutation_targets_authorized(admission)
                    }
                })
            }
            Some(InvocationTargetAdmission::Read(_) | InvocationTargetAdmission::Ineligible(_))
            | None => self.phase.is_some(),
        }
    }

    #[cfg(test)]
    pub(in crate::machine) fn blocks_mutation(&self, name: &str) -> bool {
        self.blocks_invocation_mutation(name, None)
    }
}
