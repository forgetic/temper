//! Closed, privacy-safe graph recovery diagnostics.

use serde::{Deserialize, Serialize};

use super::ToolFailureReasonV1;

pub const MAX_GRAPH_RECOVERY_ALLOWANCE_V1: u8 = 4;

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
/// deduplicated; no provider output, selector, path, source, or call identity
/// can enter this representation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphExplorationClosedV1 {
    pub reason: GraphExplorationClosedReasonV1,
    pub missing_evidence: Vec<GraphRecoveryEvidenceKindV1>,
    pub permitted_action: GraphRecoveryPermittedActionV1,
    pub remaining_allowance: u8,
}

impl GraphExplorationClosedV1 {
    pub fn completed() -> Self {
        Self {
            reason: GraphExplorationClosedReasonV1::Completed,
            missing_evidence: Vec::new(),
            permitted_action: GraphRecoveryPermittedActionV1::ConventionalDiscovery,
            remaining_allowance: 0,
        }
    }

    pub fn recoverable(
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
        })
    }

    pub fn is_valid(&self) -> bool {
        self.missing_evidence
            .windows(2)
            .all(|pair| pair[0] < pair[1])
            && match self.reason {
                GraphExplorationClosedReasonV1::Completed => {
                    self.missing_evidence.is_empty()
                        && self.permitted_action
                            == GraphRecoveryPermittedActionV1::ConventionalDiscovery
                        && self.remaining_allowance == 0
                }
                GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence => {
                    !self.missing_evidence.is_empty()
                        && self.permitted_action
                            == GraphRecoveryPermittedActionV1::TargetedCurrentRootGraphCall
                        && (1..=MAX_GRAPH_RECOVERY_ALLOWANCE_V1).contains(&self.remaining_allowance)
                }
                GraphExplorationClosedReasonV1::RecoveryExhausted => {
                    !self.missing_evidence.is_empty()
                        && self.permitted_action
                            == GraphRecoveryPermittedActionV1::StopWithoutProduct
                        && self.remaining_allowance == 0
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
            GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence => format!(
                "decision-evidence recovery required; missing evidence: [{}]; permitted action: {}; remaining allowance: {}",
                missing_labels(&self.missing_evidence),
                self.permitted_action.as_str(),
                self.remaining_allowance,
            ),
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

fn missing_labels(missing: &[GraphRecoveryEvidenceKindV1]) -> String {
    missing
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
