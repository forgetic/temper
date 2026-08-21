use temper_agent_core::{
    ToolCallStatus, ToolFailureCategory, ToolFailureDiagnostic, ToolFailureReason,
};
use temper_protocol_activity::{
    ToolFailureCategoryV1, ToolFailureDiagnosticV1, ToolFailureReasonV1, ToolStatusV1,
};

pub(super) fn map_tool_status(status: ToolCallStatus) -> ToolStatusV1 {
    match status {
        ToolCallStatus::Succeeded => ToolStatusV1::Succeeded,
        ToolCallStatus::Failed => ToolStatusV1::Failed,
        ToolCallStatus::Cancelled => ToolStatusV1::Cancelled,
    }
}

pub(super) fn map_tool_failure(value: ToolFailureDiagnostic) -> ToolFailureDiagnosticV1 {
    let value = value.canonical();
    let category = match value.category {
        ToolFailureCategory::SchemaArgumentMismatch => {
            ToolFailureCategoryV1::SchemaArgumentMismatch
        }
        ToolFailureCategory::PolicyDenial => ToolFailureCategoryV1::PolicyDenial,
        ToolFailureCategory::ExecutionFailure => ToolFailureCategoryV1::ExecutionFailure,
        ToolFailureCategory::ConfigurationStartup => ToolFailureCategoryV1::ConfigurationStartup,
        ToolFailureCategory::ProjectNotReady => ToolFailureCategoryV1::ProjectNotReady,
        ToolFailureCategory::IndexFailure => ToolFailureCategoryV1::IndexFailure,
        ToolFailureCategory::Timeout => ToolFailureCategoryV1::Timeout,
        ToolFailureCategory::Transport => ToolFailureCategoryV1::Transport,
        ToolFailureCategory::ProcessExit => ToolFailureCategoryV1::ProcessExit,
        ToolFailureCategory::ProviderProtocol => ToolFailureCategoryV1::ProviderProtocol,
        ToolFailureCategory::InvalidModelInput => ToolFailureCategoryV1::InvalidModelInput,
        ToolFailureCategory::CircuitOpen => ToolFailureCategoryV1::CircuitOpen,
        ToolFailureCategory::Cancellation => ToolFailureCategoryV1::Cancellation,
        ToolFailureCategory::GraphLifecycleDenial => ToolFailureCategoryV1::GraphLifecycleDenial,
        ToolFailureCategory::CircuitRedirect => ToolFailureCategoryV1::CircuitRedirect,
    };
    let reason = match value.reason {
        ToolFailureReason::UnknownTool => ToolFailureReasonV1::UnknownTool,
        ToolFailureReason::InvalidArguments => ToolFailureReasonV1::InvalidArguments,
        ToolFailureReason::PolicyPrecondition => ToolFailureReasonV1::PolicyPrecondition,
        ToolFailureReason::AccessDenied => ToolFailureReasonV1::AccessDenied,
        ToolFailureReason::ToolReportedFailure => ToolFailureReasonV1::ToolReportedFailure,
        ToolFailureReason::ToolExecutionError => ToolFailureReasonV1::ToolExecutionError,
        ToolFailureReason::DeadlineExceeded => ToolFailureReasonV1::DeadlineExceeded,
        ToolFailureReason::RunCancelled => ToolFailureReasonV1::RunCancelled,
        ToolFailureReason::ExplorationClosed => ToolFailureReasonV1::ExplorationClosed,
        ToolFailureReason::DecisionEvidenceIncomplete => {
            ToolFailureReasonV1::DecisionEvidenceIncomplete
        }
        ToolFailureReason::DecisionEvidenceRecoveryExhausted => {
            ToolFailureReasonV1::DecisionEvidenceRecoveryExhausted
        }
        ToolFailureReason::RepeatedNonRetryable => ToolFailureReasonV1::RepeatedNonRetryable,
        ToolFailureReason::RetryBudgetExhausted => ToolFailureReasonV1::RetryBudgetExhausted,
        ToolFailureReason::ConfigurationStartup => ToolFailureReasonV1::ConfigurationStartup,
        ToolFailureReason::ProjectNotReady => ToolFailureReasonV1::ProjectNotReady,
        ToolFailureReason::IndexFailure => ToolFailureReasonV1::IndexFailure,
        ToolFailureReason::GraphTimeout => ToolFailureReasonV1::GraphTimeout,
        ToolFailureReason::Transport => ToolFailureReasonV1::Transport,
        ToolFailureReason::ProcessExit => ToolFailureReasonV1::ProcessExit,
        ToolFailureReason::ProviderProtocol => ToolFailureReasonV1::ProviderProtocol,
        ToolFailureReason::InvalidModelInput => ToolFailureReasonV1::InvalidModelInput,
        ToolFailureReason::GraphCircuitOpen => ToolFailureReasonV1::GraphCircuitOpen,
    };
    match value.graph_exploration {
        Some(details) => ToolFailureDiagnosticV1::with_graph_exploration(details),
        None => ToolFailureDiagnosticV1::with_reason(category, reason),
    }
}
