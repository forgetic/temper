//! Conversation messages synthesized by the pure machine.

use tongs::model::{AssistantMessage, ContentBlock, StopReason, ToolResultMessage};
use tongs::tools::ToolOutput;

use super::ToolFailureDiagnostic;

/// Builds the tool-result message appended to the conversation after a tool runs.
pub(super) fn tool_result_message(
    tool_call_id: &str,
    tool_name: &str,
    output: ToolOutput,
    failure: Option<ToolFailureDiagnostic>,
) -> ToolResultMessage {
    let (content, details, is_error) = match failure {
        Some(failure) => (
            vec![ContentBlock::Text(tongs::model::TextContent {
                text: failure.model_message(),
                text_signature: None,
            })],
            None,
            true,
        ),
        None => (output.content, output.details, output.is_error),
    };
    ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content,
        details,
        is_error,
        timestamp: 0,
    }
}

/// Synthesizes a terminal assistant message carrying an error string, for the
/// paths where the run ends without a real model message.
pub(super) fn error_assistant(message: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text(tongs::model::TextContent {
            text: message.to_string(),
            text_signature: None,
        })],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: tongs::model::Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some(message.to_string()),
        timestamp: 0,
    }
}
