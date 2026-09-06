//! Classification of typed and legacy model run failures.
use super::CodingAgentError;

/// Promotes typed model-unavailability facts without parsing provider display
/// text. All other model failures retain their complete safe diagnostic.
pub(crate) fn classify_model_failure(
    diagnostic: temper_agent_core::ModelFailureDiagnostic,
) -> CodingAgentError {
    let unavailable_code = diagnostic.provider_error_code().is_some_and(|code| {
        matches!(
            code.to_ascii_lowercase().as_str(),
            "model_not_found" | "model_unavailable" | "unknown_model"
        )
    });
    let unavailable_status = diagnostic.http_status() == Some(404)
        && matches!(
            diagnostic.category(),
            temper_agent_core::ModelFailureCategory::Provider
                | temper_agent_core::ModelFailureCategory::Context
        );
    if unavailable_code || unavailable_status {
        CodingAgentError::ModelUnavailable {
            model: diagnostic.model().to_string(),
            detail: diagnostic.message().to_string(),
            diagnostic: Box::new(diagnostic),
        }
    } else {
        CodingAgentError::ModelFailure(Box::new(diagnostic))
    }
}

/// Classifies a legacy run/stop error message, promoting a model-availability
/// rejection to [`CodingAgentError::ModelUnavailable`] (which names the model
/// and points at the override env vars) and leaving everything else as a
/// generic abnormal stop.
///
/// Providers phrase this differently — Anthropic returns `404` with
/// `"<Model> is not available"` / `"Please use Opus 4.8"`; OpenAI returns
/// `"model ... does not exist or you do not have access"`. We match on these
/// stable fragments rather than a status code because the message reaches us
/// as flattened text.
pub(crate) fn classify_run_error(model: &str, message: String) -> CodingAgentError {
    let lower = message.to_ascii_lowercase();
    let unavailable = lower.contains("is not available")
        || lower.contains("does not exist")
        || lower.contains("do not have access")
        || lower.contains("model_not_found")
        || (lower.contains("model") && lower.contains("unavailable"));
    if unavailable {
        CodingAgentError::ModelUnavailable {
            model: model.to_string(),
            detail: message,
            diagnostic: Box::new(temper_agent_core::ModelFailureDiagnostic::redacted_unknown(
                "unknown", model, false,
            )),
        }
    } else {
        CodingAgentError::AgentStopped(message)
    }
}
