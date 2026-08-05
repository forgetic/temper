use temper_agent_core::{ToolCallStatus, ToolFailureCategory, ToolFailureDiagnostic};
use temper_protocol_activity::{ToolFailureCategoryV1, ToolFailureDiagnosticV1, ToolStatusV1};

pub(super) fn map_tool_status(status: ToolCallStatus) -> ToolStatusV1 {
    match status {
        ToolCallStatus::Succeeded => ToolStatusV1::Succeeded,
        ToolCallStatus::Failed => ToolStatusV1::Failed,
        ToolCallStatus::Cancelled => ToolStatusV1::Cancelled,
    }
}

pub(super) fn map_tool_failure(value: ToolFailureDiagnostic) -> ToolFailureDiagnosticV1 {
    let category = match value.category {
        ToolFailureCategory::ConfigurationStartup => ToolFailureCategoryV1::ConfigurationStartup,
        ToolFailureCategory::ProjectNotReady => ToolFailureCategoryV1::ProjectNotReady,
        ToolFailureCategory::IndexFailure => ToolFailureCategoryV1::IndexFailure,
        ToolFailureCategory::Timeout => ToolFailureCategoryV1::Timeout,
        ToolFailureCategory::Transport => ToolFailureCategoryV1::Transport,
        ToolFailureCategory::ProcessExit => ToolFailureCategoryV1::ProcessExit,
        ToolFailureCategory::ProviderProtocol => ToolFailureCategoryV1::ProviderProtocol,
        ToolFailureCategory::InvalidModelInput => ToolFailureCategoryV1::InvalidModelInput,
        ToolFailureCategory::CircuitOpen => ToolFailureCategoryV1::CircuitOpen,
    };
    ToolFailureDiagnosticV1::new(category)
}
