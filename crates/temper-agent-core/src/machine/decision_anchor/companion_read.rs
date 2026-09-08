//! Exact reads of companion files bound to an already-read implementation.

use super::exact_read::MAX_EXACT_TARGET_AUTHORITIES;
use super::*;

#[derive(Default)]
pub(super) struct CompanionReads {
    pending: BTreeMap<String, CompanionRead>,
    completed: Vec<CompanionRead>,
}

struct CompanionRead {
    target: EligibleWorkspaceTarget,
    implementation: ExactReadAuthority,
}

impl CompanionRead {
    fn is_current(&self, anchors: &AnchorForest, authorities: &[ExactReadAuthority]) -> bool {
        anchors.implementation_root().is_some_and(|(root, _)| {
            *root == self.implementation.root_binding
                && authorities.iter().any(|authority| {
                    authority.same_read(&self.implementation)
                        && authority.authorizes(&authority.target, anchors)
                })
        })
    }
}

impl DecisionAnchorState {
    pub(super) fn register_companion_read(
        &mut self,
        call: &ToolCall,
        admission: Option<&InvocationTargetAdmission>,
    ) {
        let Some(AnchorPhase::EnabledComplete(anchors)) = self.phase.as_ref() else {
            return;
        };
        let Some(InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(target))) =
            admission
        else {
            return;
        };
        if call.name != "read" {
            return;
        }
        let Some((root, _)) = anchors.implementation_root() else {
            return;
        };
        let Some(implementation) = self.exact_read_authorities.iter().find(|authority| {
            authority.root_binding == *root && authority.authorizes(&authority.target, anchors)
        }) else {
            return;
        };
        if self
            .exact_read_authorities
            .iter()
            .any(|authority| authority.authorizes(target, anchors))
        {
            return;
        }
        self.companion_reads
            .completed
            .retain(|read| read.is_current(anchors, &self.exact_read_authorities));
        self.companion_reads
            .pending
            .retain(|_, read| read.is_current(anchors, &self.exact_read_authorities));
        if self.companion_reads.pending.len() >= MAX_EXACT_TARGET_AUTHORITIES {
            return;
        }
        if let Some(key) = GraphCorrelationV1::target_digest(&call.id) {
            self.companion_reads.pending.insert(
                key,
                CompanionRead {
                    target: target.clone(),
                    implementation: implementation.clone(),
                },
            );
        }
    }

    pub(super) fn settle_companion_reads(&mut self, completed: &[SettledToolCall<'_>]) {
        for (id, name, output, _, succeeded) in completed {
            let Some(key) = GraphCorrelationV1::target_digest(id) else {
                continue;
            };
            let Some(read) = self.companion_reads.pending.remove(&key) else {
                continue;
            };
            let Some(AnchorPhase::EnabledComplete(anchors)) = self.phase.as_ref() else {
                continue;
            };
            if *name != "read"
                || !succeeded
                || output.is_error
                || !read.is_current(anchors, &self.exact_read_authorities)
            {
                continue;
            }
            self.companion_reads
                .completed
                .retain(|current| !current.target.matches(&read.target));
            if self.companion_reads.completed.len() < MAX_EXACT_TARGET_AUTHORITIES {
                self.companion_reads.completed.push(read);
            }
        }
    }

    pub(super) fn companion_target_authorized(
        &self,
        target: &EligibleWorkspaceTarget,
        anchors: &AnchorForest,
    ) -> bool {
        self.companion_reads.completed.iter().any(|read| {
            read.target.matches(target) && read.is_current(anchors, &self.exact_read_authorities)
        })
    }
}
