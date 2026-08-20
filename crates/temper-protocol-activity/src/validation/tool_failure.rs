use crate::{
    ActivityValidationError, MAX_TOOL_FAILURE_MESSAGE_BYTES, ToolFailureDiagnosticV1,
    ToolRetryDispositionV1,
};

use super::{ActivityValidationCode, error};

pub(super) fn validate_tool_failure(
    value: &ToolFailureDiagnosticV1,
    path: &str,
) -> Result<(), ActivityValidationError> {
    if !value.reason.valid_for(value.category) {
        return Err(invalid_event(
            &format!("{path}.reason"),
            "must use a closed reason valid for its tool-failure category",
        ));
    }
    if value.message != value.reason.safe_message() {
        return Err(invalid_event(
            &format!("{path}.message"),
            "must use the fixed summary for its tool-failure reason",
        ));
    }
    if value.message.is_empty() || value.message.len() > MAX_TOOL_FAILURE_MESSAGE_BYTES {
        return Err(error(
            ActivityValidationCode::OversizedInlineValue,
            format!("{path}.message"),
            format!("must be between 1 and {MAX_TOOL_FAILURE_MESSAGE_BYTES} UTF-8 bytes"),
        ));
    }
    if value.retry_disposition != value.reason.retry_disposition() {
        return Err(invalid_event(
            &format!("{path}.retry_disposition"),
            "must use the fixed retry disposition for its tool-failure reason",
        ));
    }
    if value.retryable != matches!(value.retry_disposition, ToolRetryDispositionV1::Retryable) {
        return Err(invalid_event(
            &format!("{path}.retryable"),
            "must agree with the closed retry disposition",
        ));
    }
    if value.fallback_to_conventional_discovery != value.reason.fallback_to_conventional_discovery()
    {
        return Err(invalid_event(
            &format!("{path}.fallback_to_conventional_discovery"),
            "must use the fixed fallback guidance for its tool-failure reason",
        ));
    }
    Ok(())
}

fn invalid_event(field: &str, detail: impl Into<String>) -> ActivityValidationError {
    error(ActivityValidationCode::InvalidEvent, field, detail)
}
