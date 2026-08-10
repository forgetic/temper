use serde::{Deserialize, Serialize};

/// Typed reason the agent scope reached its terminal boundary.
///
/// This is distinct from [`super::StopReasonV1`], which describes why one model
/// turn stopped. A terminal reason describes the outcome of the complete agent
/// machine, including exhaustion of its tool-iteration budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminalReasonV1 {
    Completed,
    ModelError,
    Aborted,
    BudgetExhausted,
    DecisionAnchorRecoveryExhausted,
}

impl AgentTerminalReasonV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::ModelError => "model_error",
            Self::Aborted => "aborted",
            Self::BudgetExhausted => "budget_exhausted",
            Self::DecisionAnchorRecoveryExhausted => "decision_anchor_recovery_exhausted",
        }
    }
}
