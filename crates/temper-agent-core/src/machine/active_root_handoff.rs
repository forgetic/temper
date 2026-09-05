//! Active-root opaque recovery-reference handoff policy.

use std::collections::BTreeMap;

use temper_protocol_activity::{
    GraphCorrelationToolV1, GraphRecoveryActionV1, GraphRecoveryReferenceDispositionV1,
};
use tongs::model::{AssistantMessage, ToolCall};

use crate::LineageAdmissionOutcome;

use super::core::{AgentMachine, extract_tool_calls};
use super::decision_anchor::active_root_selector_handoff;

impl AgentMachine {
    pub(super) fn capture_recovery_reference_dispositions(&mut self, assistant: &AssistantMessage) {
        let active_root = self
            .decision_anchors
            .as_ref()
            .and_then(|state| state.active_root_binding());
        self.recovery_reference_dispositions = extract_tool_calls(&assistant.content)
            .into_iter()
            .filter_map(|call| {
                self.lineage_admission
                    .as_ref()?
                    .recovery_reference_disposition(&call.name, &call.arguments, active_root)
                    .map(|disposition| (call.id, disposition))
            })
            .collect::<BTreeMap<_, _>>();
    }

    pub(super) fn resolve_recovery_reference_dispositions(
        &self,
        calls: &[ToolCall],
        admissions: &[Option<(
            LineageAdmissionOutcome,
            Option<GraphRecoveryReferenceDispositionV1>,
        )>],
        incomplete_graph_selectors: &[Option<GraphCorrelationToolV1>],
        _active_root: Option<&str>,
    ) -> Vec<Option<GraphRecoveryReferenceDispositionV1>> {
        calls
            .iter()
            .zip(admissions)
            .zip(incomplete_graph_selectors)
            .map(|((call, admission), incomplete)| {
                let resolved = admission.as_ref().and_then(|(_, disposition)| *disposition);
                let preflight = self.recovery_reference_dispositions.get(&call.id).copied();
                if resolved == Some(GraphRecoveryReferenceDispositionV1::Recognized) {
                    resolved
                } else if preflight == Some(GraphRecoveryReferenceDispositionV1::Rejected) {
                    preflight
                } else {
                    resolved.or(preflight).or_else(|| {
                        (*incomplete == Some(GraphCorrelationToolV1::TracePath))
                            .then_some(GraphRecoveryReferenceDispositionV1::Missing)
                    })
                }
            })
            .collect()
    }

    pub(super) fn refresh_active_root_handoff(
        &mut self,
        selection: Option<(String, GraphRecoveryActionV1)>,
    ) {
        self.decision_anchor_active_handoff = selection.and_then(|(active_root, action)| {
            let selectors = self
                .lineage_admission
                .as_ref()?
                .active_root_recovery_selectors(&active_root, action);
            active_root_selector_handoff(action, &selectors)
        });
    }
}
