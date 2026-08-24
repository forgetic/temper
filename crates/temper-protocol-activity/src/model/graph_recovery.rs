//! Closed, privacy-safe graph recovery diagnostics.

use serde::{Deserialize, Serialize};

use super::{DecisionAnchorTargetKindV1, GraphCorrelationToolV1, ToolFailureReasonV1};

pub const MAX_GRAPH_RECOVERY_ALLOWANCE_V1: u8 = 4;
pub const MAX_GRAPH_RECOVERY_ACTIONS_V1: usize = 4;

/// A kind of evidence which a bounded current-root action may still supply.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRecoveryEvidenceKindV1 {
    Trace,
    Implementation,
    Caller,
    FocusedTest,
}

impl GraphRecoveryEvidenceKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Implementation => "implementation",
            Self::Caller => "caller",
            Self::FocusedTest => "focused_test",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRecoverySelectorOriginV1 {
    ImplementationEvidenceResult,
    CallerEvidenceResult,
    FocusedTestResult,
    TaskSemanticQuery,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRecoveryRelationshipV1 {
    Calls,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRecoveryDirectionV1 {
    Inbound,
}

/// One provider-neutral, current-root-compatible recovery action.
///
/// It deliberately contains no root binding, selector, query, path, source,
/// provider value, or call identity. The trusted run-local registry retains
/// those values and resolves a concrete call before provider dispatch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphRecoveryActionV1 {
    pub tool: GraphCorrelationToolV1,
    pub selector_kind: DecisionAnchorTargetKindV1,
    pub evidence_kind: GraphRecoveryEvidenceKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_origin: Option<GraphRecoverySelectorOriginV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship: Option<GraphRecoveryRelationshipV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<GraphRecoveryDirectionV1>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_tests: bool,
}

impl GraphRecoveryActionV1 {
    /// Returns the canonical direct evidence action for a missing kind.
    pub const fn for_evidence(evidence_kind: GraphRecoveryEvidenceKindV1) -> Self {
        match evidence_kind {
            GraphRecoveryEvidenceKindV1::Trace => Self {
                tool: GraphCorrelationToolV1::TracePath,
                selector_kind: DecisionAnchorTargetKindV1::FunctionName,
                evidence_kind,
                selector_origin: Some(GraphRecoverySelectorOriginV1::ImplementationEvidenceResult),
                relationship: Some(GraphRecoveryRelationshipV1::Calls),
                direction: Some(GraphRecoveryDirectionV1::Inbound),
                include_tests: false,
            },
            GraphRecoveryEvidenceKindV1::Implementation | GraphRecoveryEvidenceKindV1::Caller => {
                Self {
                    tool: GraphCorrelationToolV1::GetCodeSnippet,
                    selector_kind: DecisionAnchorTargetKindV1::QualifiedName,
                    evidence_kind,
                    selector_origin: None,
                    relationship: None,
                    direction: None,
                    include_tests: false,
                }
            }
            GraphRecoveryEvidenceKindV1::FocusedTest => Self {
                tool: GraphCorrelationToolV1::GetCodeSnippet,
                selector_kind: DecisionAnchorTargetKindV1::QualifiedName,
                evidence_kind,
                selector_origin: Some(GraphRecoverySelectorOriginV1::FocusedTestResult),
                relationship: None,
                direction: None,
                include_tests: false,
            },
        }
    }

    /// Discovers focused-test identities from a consumed caller on the same root.
    pub const fn focused_test_traversal() -> Self {
        Self {
            tool: GraphCorrelationToolV1::TracePath,
            selector_kind: DecisionAnchorTargetKindV1::FunctionName,
            evidence_kind: GraphRecoveryEvidenceKindV1::FocusedTest,
            selector_origin: Some(GraphRecoverySelectorOriginV1::CallerEvidenceResult),
            relationship: Some(GraphRecoveryRelationshipV1::Calls),
            direction: Some(GraphRecoveryDirectionV1::Inbound),
            include_tests: true,
        }
    }

    /// Discovers a focused test after a settled empty caller traversal.
    pub const fn focused_test_semantic_fallback() -> Self {
        Self {
            tool: GraphCorrelationToolV1::SearchGraph,
            selector_kind: DecisionAnchorTargetKindV1::GraphQuery,
            evidence_kind: GraphRecoveryEvidenceKindV1::FocusedTest,
            selector_origin: Some(GraphRecoverySelectorOriginV1::TaskSemanticQuery),
            relationship: None,
            direction: None,
            include_tests: false,
        }
    }

    pub fn is_valid(self) -> bool {
        match (self.tool, self.selector_kind, self.evidence_kind) {
            (
                GraphCorrelationToolV1::TracePath,
                DecisionAnchorTargetKindV1::FunctionName,
                GraphRecoveryEvidenceKindV1::Trace,
            ) => {
                self.selector_origin
                    == Some(GraphRecoverySelectorOriginV1::ImplementationEvidenceResult)
                    && self.relationship == Some(GraphRecoveryRelationshipV1::Calls)
                    && self.direction == Some(GraphRecoveryDirectionV1::Inbound)
                    && !self.include_tests
            }
            (
                GraphCorrelationToolV1::GetCodeSnippet,
                DecisionAnchorTargetKindV1::QualifiedName,
                GraphRecoveryEvidenceKindV1::Implementation | GraphRecoveryEvidenceKindV1::Caller,
            ) => {
                self.selector_origin.is_none()
                    && self.relationship.is_none()
                    && self.direction.is_none()
                    && !self.include_tests
            }
            (
                GraphCorrelationToolV1::GetCodeSnippet,
                DecisionAnchorTargetKindV1::QualifiedName,
                GraphRecoveryEvidenceKindV1::FocusedTest,
            ) => {
                self.selector_origin == Some(GraphRecoverySelectorOriginV1::FocusedTestResult)
                    && self.relationship.is_none()
                    && self.direction.is_none()
                    && !self.include_tests
            }
            (
                GraphCorrelationToolV1::TracePath,
                DecisionAnchorTargetKindV1::FunctionName,
                GraphRecoveryEvidenceKindV1::FocusedTest,
            ) => {
                self.selector_origin == Some(GraphRecoverySelectorOriginV1::CallerEvidenceResult)
                    && self.relationship == Some(GraphRecoveryRelationshipV1::Calls)
                    && self.direction == Some(GraphRecoveryDirectionV1::Inbound)
                    && self.include_tests
            }
            (
                GraphCorrelationToolV1::SearchGraph,
                DecisionAnchorTargetKindV1::GraphQuery,
                GraphRecoveryEvidenceKindV1::FocusedTest,
            ) => {
                self.selector_origin == Some(GraphRecoverySelectorOriginV1::TaskSemanticQuery)
                    && self.relationship.is_none()
                    && self.direction.is_none()
                    && !self.include_tests
            }
            _ => false,
        }
    }

    fn label(self) -> String {
        let mut label = format!(
            "{}/{}/{}",
            match self.tool {
                GraphCorrelationToolV1::SearchGraph => "search_graph",
                GraphCorrelationToolV1::SearchCode => "search_code",
                GraphCorrelationToolV1::TracePath => "trace_path",
                GraphCorrelationToolV1::GetCodeSnippet => "get_code_snippet",
            },
            selector_label(self.selector_kind),
            self.evidence_kind.as_str(),
        );
        if let Some(origin) = self.selector_origin {
            label.push_str(match origin {
                GraphRecoverySelectorOriginV1::ImplementationEvidenceResult => {
                    "/selector=implementation_evidence_result"
                }
                GraphRecoverySelectorOriginV1::CallerEvidenceResult => {
                    "/selector=caller_evidence_result"
                }
                GraphRecoverySelectorOriginV1::FocusedTestResult => "/selector=focused_test_result",
                GraphRecoverySelectorOriginV1::TaskSemanticQuery => "/selector=task_semantic_query",
            });
        }
        if self.relationship == Some(GraphRecoveryRelationshipV1::Calls) {
            label.push_str("/relationship=calls");
        }
        if self.direction == Some(GraphRecoveryDirectionV1::Inbound) {
            label.push_str("/direction=inbound");
        }
        if self.include_tests {
            label.push_str("/include_tests=true");
        }
        label
    }
}

const fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphExplorationClosedReasonV1 {
    Completed,
    RecoverableIncompleteEvidence,
    RecoveryExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRecoveryPermittedActionV1 {
    ConventionalDiscovery,
    TargetedCurrentRootGraphCall,
    StopWithoutProduct,
}

impl GraphRecoveryPermittedActionV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConventionalDiscovery => "conventional_discovery",
            Self::TargetedCurrentRootGraphCall => "targeted_current_root_graph_call",
            Self::StopWithoutProduct => "stop_without_product",
        }
    }
}

/// Closed, privacy-safe graph lifecycle state. Missing kinds are sorted and
/// deduplicated, as are compatible actions. No provider output, selector,
/// root binding, path, source, or call identity can enter this representation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphExplorationClosedV1 {
    pub reason: GraphExplorationClosedReasonV1,
    pub missing_evidence: Vec<GraphRecoveryEvidenceKindV1>,
    pub permitted_action: GraphRecoveryPermittedActionV1,
    pub remaining_allowance: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compatible_actions: Vec<GraphRecoveryActionV1>,
}

impl GraphExplorationClosedV1 {
    pub fn completed() -> Self {
        Self {
            reason: GraphExplorationClosedReasonV1::Completed,
            missing_evidence: Vec::new(),
            permitted_action: GraphRecoveryPermittedActionV1::ConventionalDiscovery,
            remaining_allowance: 0,
            compatible_actions: Vec::new(),
        }
    }

    pub fn recoverable(
        missing_evidence: impl IntoIterator<Item = GraphRecoveryEvidenceKindV1>,
        remaining_allowance: u8,
    ) -> Option<Self> {
        let missing_evidence = sorted_missing(missing_evidence);
        let compatible_actions = missing_evidence
            .iter()
            .copied()
            .map(GraphRecoveryActionV1::for_evidence)
            .collect::<Vec<_>>();
        Self::recoverable_with_actions(missing_evidence, remaining_allowance, compatible_actions)
    }

    /// Builds the compact denial emitted for an incompatible local recovery
    /// attempt. Compatible actions remain available in the separately queued
    /// recovery guidance, while this form preserves the stable denial wire.
    pub fn recoverable_without_actions(
        missing_evidence: impl IntoIterator<Item = GraphRecoveryEvidenceKindV1>,
        remaining_allowance: u8,
    ) -> Option<Self> {
        let missing_evidence = sorted_missing(missing_evidence);
        (!missing_evidence.is_empty()
            && (1..=MAX_GRAPH_RECOVERY_ALLOWANCE_V1).contains(&remaining_allowance))
        .then_some(Self {
            reason: GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence,
            missing_evidence,
            permitted_action: GraphRecoveryPermittedActionV1::TargetedCurrentRootGraphCall,
            remaining_allowance,
            compatible_actions: Vec::new(),
        })
    }

    pub fn recoverable_with_actions(
        missing_evidence: impl IntoIterator<Item = GraphRecoveryEvidenceKindV1>,
        remaining_allowance: u8,
        compatible_actions: impl IntoIterator<Item = GraphRecoveryActionV1>,
    ) -> Option<Self> {
        let missing_evidence = sorted_missing(missing_evidence);
        let compatible_actions = sorted_actions(compatible_actions);
        (!missing_evidence.is_empty()
            && (1..=MAX_GRAPH_RECOVERY_ALLOWANCE_V1).contains(&remaining_allowance)
            && !compatible_actions.is_empty()
            && compatible_actions.len() <= MAX_GRAPH_RECOVERY_ACTIONS_V1
            && compatible_actions.iter().all(|action| {
                action.is_valid() && missing_evidence.contains(&action.evidence_kind)
            }))
        .then_some(Self {
            reason: GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence,
            missing_evidence,
            permitted_action: GraphRecoveryPermittedActionV1::TargetedCurrentRootGraphCall,
            remaining_allowance,
            compatible_actions,
        })
    }

    pub fn exhausted(
        missing_evidence: impl IntoIterator<Item = GraphRecoveryEvidenceKindV1>,
    ) -> Option<Self> {
        let missing_evidence = sorted_missing(missing_evidence);
        (!missing_evidence.is_empty()).then_some(Self {
            reason: GraphExplorationClosedReasonV1::RecoveryExhausted,
            missing_evidence,
            permitted_action: GraphRecoveryPermittedActionV1::StopWithoutProduct,
            remaining_allowance: 0,
            compatible_actions: Vec::new(),
        })
    }

    pub fn is_valid(&self) -> bool {
        self.missing_evidence
            .windows(2)
            .all(|pair| pair[0] < pair[1])
            && self
                .compatible_actions
                .windows(2)
                .all(|pair| pair[0] < pair[1])
            && self.compatible_actions.len() <= MAX_GRAPH_RECOVERY_ACTIONS_V1
            && match self.reason {
                GraphExplorationClosedReasonV1::Completed => {
                    self.missing_evidence.is_empty()
                        && self.permitted_action
                            == GraphRecoveryPermittedActionV1::ConventionalDiscovery
                        && self.remaining_allowance == 0
                        && self.compatible_actions.is_empty()
                }
                GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence => {
                    !self.missing_evidence.is_empty()
                        && self.permitted_action
                            == GraphRecoveryPermittedActionV1::TargetedCurrentRootGraphCall
                        && (1..=MAX_GRAPH_RECOVERY_ALLOWANCE_V1).contains(&self.remaining_allowance)
                        && self.compatible_actions.iter().all(|action| {
                            action.is_valid()
                                && self.missing_evidence.contains(&action.evidence_kind)
                        })
                }
                GraphExplorationClosedReasonV1::RecoveryExhausted => {
                    !self.missing_evidence.is_empty()
                        && self.permitted_action
                            == GraphRecoveryPermittedActionV1::StopWithoutProduct
                        && self.remaining_allowance == 0
                        && self.compatible_actions.is_empty()
                }
            }
    }

    pub fn failure_reason(&self) -> ToolFailureReasonV1 {
        match self.reason {
            GraphExplorationClosedReasonV1::Completed => ToolFailureReasonV1::ExplorationClosed,
            GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence => {
                ToolFailureReasonV1::DecisionEvidenceIncomplete
            }
            GraphExplorationClosedReasonV1::RecoveryExhausted => {
                ToolFailureReasonV1::DecisionEvidenceRecoveryExhausted
            }
        }
    }

    pub fn model_message(&self) -> String {
        match self.reason {
            GraphExplorationClosedReasonV1::Completed => ToolFailureReasonV1::ExplorationClosed
                .safe_message()
                .to_string(),
            GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence => {
                let message = format!(
                    "decision-evidence recovery required; missing evidence: [{}]; permitted action: {}; remaining allowance: {}",
                    missing_labels(&self.missing_evidence),
                    self.permitted_action.as_str(),
                    self.remaining_allowance,
                );
                if self.compatible_actions.is_empty() {
                    message
                } else {
                    format!(
                        "{message}; compatible actions: [{}]; use only these current-root actions; no retry, root switch, or mutation",
                        action_labels(&self.compatible_actions),
                    )
                }
            }
            GraphExplorationClosedReasonV1::RecoveryExhausted => format!(
                "decision-evidence recovery exhausted; missing evidence: [{}]; permitted action: {}; remaining allowance: 0",
                missing_labels(&self.missing_evidence),
                self.permitted_action.as_str(),
            ),
        }
    }
}

fn sorted_missing(
    missing_evidence: impl IntoIterator<Item = GraphRecoveryEvidenceKindV1>,
) -> Vec<GraphRecoveryEvidenceKindV1> {
    let mut missing_evidence = missing_evidence.into_iter().collect::<Vec<_>>();
    missing_evidence.sort();
    missing_evidence.dedup();
    missing_evidence
}

fn sorted_actions(
    compatible_actions: impl IntoIterator<Item = GraphRecoveryActionV1>,
) -> Vec<GraphRecoveryActionV1> {
    let mut compatible_actions = compatible_actions.into_iter().collect::<Vec<_>>();
    compatible_actions.sort();
    compatible_actions.dedup();
    compatible_actions
}

fn missing_labels(missing: &[GraphRecoveryEvidenceKindV1]) -> String {
    missing
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn action_labels(actions: &[GraphRecoveryActionV1]) -> String {
    actions
        .iter()
        .copied()
        .map(GraphRecoveryActionV1::label)
        .collect::<Vec<_>>()
        .join(", ")
}

const fn selector_label(kind: DecisionAnchorTargetKindV1) -> &'static str {
    match kind {
        DecisionAnchorTargetKindV1::GraphQuery => "graph_query",
        DecisionAnchorTargetKindV1::Pattern => "pattern",
        DecisionAnchorTargetKindV1::NamePattern => "name_pattern",
        DecisionAnchorTargetKindV1::QualifiedNamePattern => "qualified_name_pattern",
        DecisionAnchorTargetKindV1::FunctionName => "function_name",
        DecisionAnchorTargetKindV1::QualifiedName => "qualified_name",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_denial_preserves_the_stable_missing_evidence_wire() {
        let details = GraphExplorationClosedV1::recoverable_without_actions(
            [GraphRecoveryEvidenceKindV1::Caller],
            4,
        )
        .expect("compact recovery denial");
        assert!(details.is_valid());
        assert_eq!(
            details.model_message(),
            "decision-evidence recovery required; missing evidence: [caller]; permitted action: targeted_current_root_graph_call; remaining allowance: 4"
        );
        assert_eq!(
            serde_json::to_value(details).unwrap(),
            serde_json::json!({
                "reason": "recoverable_incomplete_evidence",
                "missing_evidence": ["caller"],
                "permitted_action": "targeted_current_root_graph_call",
                "remaining_allowance": 4,
            })
        );
    }

    #[test]
    fn recoverable_message_is_an_exact_bounded_current_root_instruction() {
        const PRIVATE_SELECTOR: &str = "private::selector::must_not_escape";
        let details = GraphExplorationClosedV1::recoverable(
            [
                GraphRecoveryEvidenceKindV1::Caller,
                GraphRecoveryEvidenceKindV1::Trace,
            ],
            2,
        )
        .expect("actionable recovery guidance");

        let message = details.model_message();
        assert_eq!(
            message,
            "decision-evidence recovery required; missing evidence: [trace, caller]; permitted action: targeted_current_root_graph_call; remaining allowance: 2; compatible actions: [trace_path/function_name/trace/selector=implementation_evidence_result/relationship=calls/direction=inbound, get_code_snippet/qualified_name/caller]; use only these current-root actions; no retry, root switch, or mutation"
        );
        assert!(!message.contains(PRIVATE_SELECTOR));
        assert!(!message.contains("root_binding"));
        assert!(!message.contains("qualified_name="));
        assert!(details.is_valid());
    }
}
