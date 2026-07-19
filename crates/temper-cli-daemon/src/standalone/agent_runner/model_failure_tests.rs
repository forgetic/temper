use temper_protocol_activity::ModelFailureCategoryV1;
use tongs::{FailureCategory, ProviderFailureDiagnostic};

use super::*;

fn diagnostic(category: ModelFailureCategoryV1) -> temper_agent::ModelFailureDiagnostic {
    if category == ModelFailureCategoryV1::RedactedUnknown {
        return temper_agent::ModelFailureDiagnostic::redacted_unknown(
            "openai-codex",
            "gpt-test",
            false,
        );
    }
    let (upstream_category, retryable, status, request_id, code, message) = match category {
        ModelFailureCategoryV1::Timeout => (
            FailureCategory::Timeout,
            true,
            Some(504),
            None,
            None,
            "Model request timed out.",
        ),
        ModelFailureCategoryV1::RateLimit => (
            FailureCategory::RateLimit,
            true,
            Some(429),
            Some("req_rate_532"),
            Some("rate_limit"),
            "Rate limit exceeded; retry later.",
        ),
        ModelFailureCategoryV1::Response => (
            FailureCategory::Response,
            false,
            Some(502),
            Some("req_response_532"),
            Some("malformed_stream"),
            "Provider returned a malformed stream.",
        ),
        _ => unreachable!("daemon regression matrix uses four categories"),
    };
    let upstream = ProviderFailureDiagnostic::new(
        upstream_category,
        retryable,
        status,
        request_id,
        code,
        message,
    );
    temper_agent::ModelFailureDiagnostic::from_provider(
        &temper_agent::ModelIdentity::new("openai-codex", "gpt-test"),
        &upstream,
    )
}

#[test]
fn typed_model_failures_survive_terminal_selection_before_worker_conversion() {
    for category in [
        ModelFailureCategoryV1::Timeout,
        ModelFailureCategoryV1::RateLimit,
        ModelFailureCategoryV1::Response,
        ModelFailureCategoryV1::RedactedUnknown,
    ] {
        let error = CodingAgentError::ModelFailure(Box::new(diagnostic(category)));
        assert_eq!(
            agent_terminal_report(&Result::<(), _>::Err(error), false),
            (
                AgentTerminalStatus::Failed,
                Some(AgentTerminalReasonV1::ModelError)
            )
        );

        let error = CodingAgentError::ModelFailure(Box::new(diagnostic(category)));
        let failure = coding_agent_model_failure(&error).expect("protocol model failure");
        assert_eq!(failure.provider, "openai-codex");
        assert_eq!(failure.model, "gpt-test");
        assert_eq!(failure.category, category);
        if category == ModelFailureCategoryV1::RateLimit {
            assert_eq!(failure.http_status, Some(429));
            assert_eq!(failure.provider_request_id.as_deref(), Some("req_rate_532"));
        }
        if category == ModelFailureCategoryV1::RedactedUnknown {
            assert!(failure.detail_redacted);
        }

        let worker_error = classify_coding_agent_error(error, false);
        assert_eq!(worker_error.class, FailureClass::Transient);
        assert!(worker_error.message.contains(category.as_str()));
        assert!(!worker_error.message.contains("SECRET-SENTINEL-532"));
    }
}
