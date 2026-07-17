use temper_agent_core::AgentStop;
use temper_protocol_activity::{AgentTerminalReasonV1, ScopeStatusV1};

pub(super) fn scope_terminal(reason: AgentStop) -> (ScopeStatusV1, AgentTerminalReasonV1) {
    match reason {
        AgentStop::Completed => (ScopeStatusV1::Succeeded, AgentTerminalReasonV1::Completed),
        AgentStop::ModelError => (ScopeStatusV1::Failed, AgentTerminalReasonV1::ModelError),
        AgentStop::Aborted => (ScopeStatusV1::Cancelled, AgentTerminalReasonV1::Aborted),
        AgentStop::BudgetExhausted => (
            ScopeStatusV1::Failed,
            AgentTerminalReasonV1::BudgetExhausted,
        ),
    }
}
