//! Model-visible, privacy-safe active-root progress guidance.

use crate::OpaqueRecoverySelectorReference;

use super::*;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum AcceptedEvidence {
    Root,
    Trace,
    Implementation,
    Caller,
    FocusedTestRoute,
    FocusedTest,
}

impl From<DecisionEvidenceKindV1> for AcceptedEvidence {
    fn from(value: DecisionEvidenceKindV1) -> Self {
        match value {
            DecisionEvidenceKindV1::Implementation => Self::Implementation,
            DecisionEvidenceKindV1::Caller => Self::Caller,
            DecisionEvidenceKindV1::FocusedTest => Self::FocusedTest,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ResultProgress {
    accepted: BTreeSet<AcceptedEvidence>,
    route_progressed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResultDisposition {
    ActiveRootProgress,
    SiblingRootEvidence,
    NonProgress,
}

struct GuidanceSnapshot {
    active_root: Option<String>,
    missing: Vec<GraphRecoveryEvidenceKindV1>,
    next_actions: Vec<String>,
    lifecycle: &'static str,
    remaining: Option<u8>,
    complete: bool,
}

impl DecisionAnchorState {
    pub(super) fn mark_accepted(&mut self, id: &str, evidence: AcceptedEvidence) {
        self.batch_progress
            .entry(id.to_string())
            .or_default()
            .accepted
            .insert(evidence);
    }

    pub(super) fn mark_route_progress(&mut self, id: &str) {
        self.batch_progress
            .entry(id.to_string())
            .or_default()
            .route_progressed = true;
    }

    pub(super) fn queue_finished_guidance(&mut self, finished: &[FinishedCodebaseCall<'_>]) {
        if matches!(self.phase, Some(AnchorPhase::ProviderUnavailable)) {
            return;
        }
        let snapshot = self.guidance_snapshot();
        for finished in finished {
            if finished.output.is_error || !finished.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
                continue;
            }
            let progress = self.batch_progress.remove(finished.id).unwrap_or_default();
            let output = anchor_output(finished.name, finished.output);
            let disposition = output
                .as_ref()
                .map_or(ResultDisposition::NonProgress, |output| {
                    if snapshot.active_root.as_deref() != Some(output.lineage.root_binding.as_str())
                    {
                        ResultDisposition::SiblingRootEvidence
                    } else if output.lineage.stage == DecisionAnchorLineageStageV1::Root
                        || !progress.accepted.is_empty()
                        || progress.route_progressed
                    {
                        ResultDisposition::ActiveRootProgress
                    } else {
                        ResultDisposition::NonProgress
                    }
                });
            let mut accepted = progress.accepted;
            if output.as_ref().is_some_and(|output| {
                disposition == ResultDisposition::ActiveRootProgress
                    && output.lineage.stage == DecisionAnchorLineageStageV1::Root
            }) {
                accepted.insert(AcceptedEvidence::Root);
                if output
                    .as_ref()
                    .is_some_and(|output| output.lineage.caller_discovery.is_some())
                {
                    accepted.insert(AcceptedEvidence::Trace);
                }
            }
            self.model_guidance
                .push(snapshot.model_message(disposition, &accepted, None));
        }
    }

    pub(super) fn queue_local_denial_guidance(
        &mut self,
        rejected_action: Option<GraphRecoveryActionV1>,
        selector_tuple_excluded: bool,
    ) {
        let snapshot = self.guidance_snapshot();
        self.model_guidance.push(snapshot.model_message(
            ResultDisposition::NonProgress,
            &BTreeSet::new(),
            rejected_action.map(|action| (action, selector_tuple_excluded)),
        ));
    }

    pub(super) fn queue_local_traversal_readiness_guidance(&mut self) {
        let implementation_action =
            GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Implementation);
        self.handoff_override = Some(implementation_action);
        let mut snapshot = self.guidance_snapshot();
        snapshot.next_actions = vec![implementation_action.model_label()];
        let mut guidance =
            snapshot.model_message(ResultDisposition::NonProgress, &BTreeSet::new(), None);
        guidance.push_str(" [Traversal readiness: the current-root traversal remains closed because its typed provider snapshot reported callers without caller identities; no provider call or recovery allowance was consumed. Repeat exactly one typed implementation get_code_snippet lookup in the next model turn by copying the existing implementation_evidence_result recovery reference into qualified_name.]");
        self.model_guidance.push(guidance);
    }

    pub(in crate::machine) fn take_model_guidance(&mut self) -> Vec<String> {
        self.handoff_override = None;
        std::mem::take(&mut self.model_guidance)
    }

    pub(in crate::machine) fn active_recovery_action(
        &self,
    ) -> Option<(String, GraphRecoveryActionV1)> {
        let (binding, active, route) = match self.phase.as_ref()? {
            AnchorPhase::Root(anchors) | AnchorPhase::Trail(anchors) => {
                let (binding, route) = anchors.active_selection(&BTreeSet::new())?;
                (binding, anchors.roots.get(binding)?, route)
            }
            AnchorPhase::Recovery(recovery) => {
                let (binding, route) = recovery.anchors.active_selection(&BTreeSet::new())?;
                (binding, recovery.anchors.roots.get(binding)?, route)
            }
            AnchorPhase::GapRecovery(recovery) => (
                &recovery.active_root,
                recovery.anchors.roots.get(&recovery.active_root)?,
                recovery.route,
            ),
            AnchorPhase::EnabledComplete(_)
            | AnchorPhase::EnabledIncomplete(_)
            | AnchorPhase::ProviderUnavailable => return None,
        };
        self.handoff_override
            .or_else(|| {
                active
                    .evidence
                    .compatible_actions(active, route)
                    .into_iter()
                    .next()
            })
            .map(|action| (binding.clone(), action))
    }

    fn guidance_snapshot(&self) -> GuidanceSnapshot {
        match self.phase.as_ref() {
            Some(AnchorPhase::Root(anchors)) | Some(AnchorPhase::Trail(anchors)) => anchors
                .active_selection(&BTreeSet::new())
                .and_then(|(binding, route)| {
                    anchors.roots.get(binding).map(|active| {
                        GuidanceSnapshot::from_active(
                            binding,
                            active,
                            route,
                            anchors.missing_kinds(binding, route),
                            "open",
                            None,
                            false,
                        )
                    })
                })
                .unwrap_or_else(GuidanceSnapshot::empty),
            Some(AnchorPhase::EnabledComplete(anchors)) => anchors
                .implementation_root()
                .map(|(binding, active)| {
                    GuidanceSnapshot::from_active(
                        binding,
                        active,
                        RecoveryRoute::Implementation,
                        Vec::new(),
                        "complete",
                        None,
                        true,
                    )
                })
                .unwrap_or_else(GuidanceSnapshot::empty),
            Some(AnchorPhase::Recovery(recovery)) => recovery
                .anchors
                .active_selection(&BTreeSet::new())
                .and_then(|(binding, route)| {
                    recovery.anchors.roots.get(binding).map(|active| {
                        GuidanceSnapshot::from_active(
                            binding,
                            active,
                            route,
                            recovery.anchors.missing_kinds(binding, route),
                            "root_recovery",
                            None,
                            false,
                        )
                    })
                })
                .unwrap_or_else(GuidanceSnapshot::empty),
            Some(AnchorPhase::GapRecovery(recovery)) => recovery
                .anchors
                .roots
                .get(&recovery.active_root)
                .map(|active| {
                    GuidanceSnapshot::from_active(
                        &recovery.active_root,
                        active,
                        recovery.route,
                        recovery
                            .anchors
                            .missing_kinds(&recovery.active_root, recovery.route),
                        "evidence_recovery",
                        Some(recovery.remaining),
                        false,
                    )
                })
                .unwrap_or_else(GuidanceSnapshot::empty),
            Some(AnchorPhase::EnabledIncomplete(evidence)) => GuidanceSnapshot {
                active_root: None,
                missing: evidence.all_missing_kinds(),
                next_actions: Vec::new(),
                lifecycle: "exhausted",
                remaining: Some(0),
                complete: false,
            },
            Some(AnchorPhase::ProviderUnavailable) => GuidanceSnapshot::empty(),
            None => GuidanceSnapshot::empty(),
        }
    }
}

impl GuidanceSnapshot {
    fn from_active(
        binding: &str,
        active: &Anchor,
        route: RecoveryRoute,
        missing: Vec<GraphRecoveryEvidenceKindV1>,
        lifecycle: &'static str,
        remaining: Option<u8>,
        complete: bool,
    ) -> Self {
        let exhausted = remaining == Some(0);
        let lifecycle = if exhausted { "exhausted" } else { lifecycle };
        let mut next_actions = if exhausted {
            Vec::new()
        } else {
            active
                .evidence
                .compatible_actions(active, route)
                .into_iter()
                .map(GraphRecoveryActionV1::model_label)
                .collect::<Vec<_>>()
        };
        if complete {
            next_actions = vec!["read/workspace_target/exact_post_source".to_string()];
        }
        Self {
            active_root: Some(binding.to_string()),
            missing,
            next_actions,
            lifecycle,
            remaining,
            complete,
        }
    }

    fn empty() -> Self {
        Self {
            active_root: None,
            missing: Vec::new(),
            next_actions: Vec::new(),
            lifecycle: "unanchored",
            remaining: None,
            complete: false,
        }
    }

    fn model_message(
        &self,
        disposition: ResultDisposition,
        accepted: &BTreeSet<AcceptedEvidence>,
        rejected: Option<(GraphRecoveryActionV1, bool)>,
    ) -> String {
        let disposition = match disposition {
            ResultDisposition::ActiveRootProgress => "active_root_progress",
            ResultDisposition::SiblingRootEvidence => "sibling_or_cross_root_evidence",
            ResultDisposition::NonProgress => "non_progress",
        };
        let accepted = accepted
            .iter()
            .map(|kind| match kind {
                AcceptedEvidence::Root => "root",
                AcceptedEvidence::Trace => "trace",
                AcceptedEvidence::Implementation => "implementation",
                AcceptedEvidence::Caller => "caller",
                AcceptedEvidence::FocusedTestRoute => "focused_test_route",
                AcceptedEvidence::FocusedTest => "focused_test",
            })
            .collect::<Vec<_>>()
            .join(", ");
        let missing = self
            .missing
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let actions = self.next_actions.join(", ");
        let remaining = self
            .remaining
            .map_or_else(|| "n/a".to_string(), |remaining| remaining.to_string());
        let required_next_stage = if self.next_actions.len() == 1 && !self.complete {
            format!(
                "; required next stage=[{}]; issue exactly this one action in the next model turn",
                self.next_actions[0],
            )
        } else if self.next_actions.len() > 1 {
            "; required next stage=[parallel typed recovery]; use only the listed actions from this immutable snapshot"
                .to_string()
        } else {
            String::new()
        };
        let rejected = rejected.map_or_else(String::new, |(action, excluded)| {
            format!(
                "; rejected action=[{}]; rejected selector tuple excluded={excluded}; do not repeat that selector value",
                action.model_label(),
            )
        });
        let root_note = if disposition == "sibling_or_cross_root_evidence" {
            "; this result does not satisfy the selected active root"
        } else {
            ""
        };
        let completion = if self.complete {
            "; graph exploration=closed; perform one successful ordinary exact read of the selected workspace target now, then make only the matching minimal mutation"
        } else if self.lifecycle == "exhausted" {
            "; no compatible provider-derived action remains; stop without a product"
        } else {
            "; selector values must come from provider results accepted for this active root"
        };
        format!(
            "[Decision guidance: result={disposition}; accepted evidence=[{accepted}]; active-root missing evidence=[{missing}]; recovery={}; remaining allowance={remaining}; next compatible actions=[{actions}]{required_next_stage}{rejected}{root_note}{completion}.]",
            self.lifecycle,
        )
    }
}

pub(in crate::machine) fn active_root_selector_handoff(
    action: GraphRecoveryActionV1,
    selectors: &[OpaqueRecoverySelectorReference],
) -> Option<String> {
    const MAX_SELECTOR_MENU: usize = 4;

    if !action.is_valid() || selectors.is_empty() || selectors.len() > MAX_SELECTOR_MENU {
        return None;
    }
    let references = selectors
        .iter()
        .map(OpaqueRecoverySelectorReference::as_public_selector)
        .collect::<Vec<_>>();
    if references.iter().copied().collect::<BTreeSet<_>>().len() != selectors.len() {
        return None;
    }
    if references.len() == 1 {
        let reference = references[0];
        let (selector_field, arguments) = match (action.tool, action.selector_kind) {
            (GraphCorrelationToolV1::GetCodeSnippet, DecisionAnchorTargetKindV1::QualifiedName) => {
                (
                    "qualified_name",
                    format!("{{\"qualified_name\":\"{reference}\"}}"),
                )
            }
            (GraphCorrelationToolV1::TracePath, DecisionAnchorTargetKindV1::FunctionName) => (
                "function_name",
                format!(
                    "{{\"function_name\":\"{reference}\",\"mode\":\"calls\",\"direction\":\"inbound\",\"include_tests\":{}}}",
                    action.include_tests,
                ),
            ),
            _ => return None,
        };
        return Some(format!(
            "[Active-root selector handoff: required next call=[{} arguments={}]; selector field={selector_field}; exactly one active-root-bound opaque reference is authorized; provider-result-local sibling and alternate references are not actionable; issue exactly this one call in the next model turn.]",
            action.tool.public_name(),
            arguments,
        ));
    }
    let selector_field = match (action.tool, action.selector_kind) {
        (GraphCorrelationToolV1::GetCodeSnippet, DecisionAnchorTargetKindV1::QualifiedName) => {
            "qualified_name"
        }
        (GraphCorrelationToolV1::TracePath, DecisionAnchorTargetKindV1::FunctionName) => {
            "function_name"
        }
        _ => return None,
    };
    let options = references
        .iter()
        .enumerate()
        .map(|(index, reference)| format!("candidate_{}={reference}", index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let arguments = if action.tool == GraphCorrelationToolV1::GetCodeSnippet {
        "{\"qualified_name\":\"<copy exactly one candidate reference>\"}".to_string()
    } else {
        format!(
            "{{\"function_name\":\"<copy exactly one candidate reference>\",\"mode\":\"calls\",\"direction\":\"inbound\",\"include_tests\":{}}}",
            action.include_tests,
        )
    };
    Some(format!(
        "[Active-root selector handoff: required next call=[{} arguments={}]; selector field={selector_field}; current-active-root candidate references=[{options}]; choose and copy exactly one presented opaque reference into the selector field; that reference alone becomes authoritative and every unchosen, sibling, raw, or fabricated value remains non-actionable; issue exactly one call in the next model turn.]",
        action.tool.public_name(),
        arguments,
    ))
}
