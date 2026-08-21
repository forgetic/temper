//! Content-free local tool completion and model-output helpers.

use tongs::model::ToolCall;

use crate::machine::{
    AgentCompletion, AgentEvent, BatchGeneration, CODEBASE_MEMORY_TOOL_PREFIX, OperationGeneration,
    SAFE_TOOL_FAILURE_DETAIL_KEY, ToolCallStatus, ToolFailureDiagnostic, ToolResultMetadata,
};

/// Builds the event/completion pair for a machine-selected circuit redirect.
/// No registry reference is accepted at this boundary, so execution is
/// structurally impossible.
pub(super) fn local_redirect(
    operation_generation: OperationGeneration,
    batch_generation: BatchGeneration,
    call: ToolCall,
    failure: ToolFailureDiagnostic,
) -> (AgentEvent, AgentCompletion) {
    let result = ToolResultMetadata {
        failure: Some(failure.clone()),
        ..ToolResultMetadata::default()
    };
    let event = AgentEvent::ToolEnd {
        id: call.id.clone(),
        name: call.name.clone(),
        status: ToolCallStatus::Failed,
        duration_ms: 0,
        result,
    };
    let completion = AgentCompletion::ToolFinished {
        operation_generation,
        batch_generation,
        id: call.id,
        output: diagnostic_tool_output(&call.name, &failure),
        failure: Some(failure),
    };
    (event, completion)
}

pub(super) fn diagnostic_tool_output(
    name: &str,
    diagnostic: &ToolFailureDiagnostic,
) -> tongs::tools::ToolOutput {
    let mut output = tool_error_output(&diagnostic.model_message());
    if name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        output.details = Some(serde_json::json!({
            SAFE_TOOL_FAILURE_DETAIL_KEY: {
                "source": "codebase_memory",
                "category": diagnostic.category.as_str(),
            }
        }));
    }
    output
}

/// Builds an error [`tongs::tools::ToolOutput`] carrying `message` as text.
pub(super) fn tool_error_output(message: &str) -> tongs::tools::ToolOutput {
    tongs::tools::ToolOutput {
        content: vec![tongs::model::ContentBlock::Text(
            tongs::model::TextContent {
                text: message.to_string(),
                text_signature: None,
            },
        )],
        details: None,
        is_error: true,
    }
}
