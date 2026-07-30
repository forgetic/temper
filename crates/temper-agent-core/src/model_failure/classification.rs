use super::{ModelFailureBoundary, ModelFailureCategory, ModelFailureDisposition};

const RETRYABLE_CODES: &[&str] = &[
    "api_error",
    "internal_error",
    "overloaded",
    "overloaded_error",
    "rate_limit",
    "rate_limit_error",
    "rate_limit_exceeded",
    "rate_limit_exceeded.v2",
    "request_timeout",
    "request_timed_out",
    "server_error",
    "timeout",
    "timeout_error",
    "too_many_requests",
    "unavailable",
];

const NON_RETRYABLE_CODES: &[&str] = &[
    "authentication_error",
    "billing_not_active",
    "context_length_exceeded",
    "context_window_exceeded",
    "entitlement_required",
    "insufficient_permissions",
    "insufficient_quota",
    "invalid_api_key",
    "invalid_request_error",
    "malformed_sse",
    "malformed_stream",
    "max_tokens_exceeded",
    "model_not_found",
    "not_found_error",
    "permission_denied",
    "permission_error",
    "prompt_too_long",
    "quota_exceeded",
    "request_too_large",
    "unauthorized",
    "usage_limit",
    "usage_limit_reached",
];

pub(super) fn is_allowlisted_code(code: &str) -> bool {
    RETRYABLE_CODES
        .iter()
        .chain(NON_RETRYABLE_CODES)
        .any(|allowed| code.eq_ignore_ascii_case(allowed))
}

pub(super) fn disposition(
    category: ModelFailureCategory,
    boundary: ModelFailureBoundary,
    status: Option<u16>,
    code: Option<&str>,
) -> ModelFailureDisposition {
    // Independently typed HTTP status takes precedence over a contradictory
    // provider-controlled code. Unknown 4xx remains unknown and is not eligible
    // for an immediate turn retry.
    match status {
        Some(408 | 504) => return ModelFailureDisposition::Retryable,
        Some(429) => return ModelFailureDisposition::Retryable,
        Some(401 | 403) => return ModelFailureDisposition::NonRetryable,
        Some(500..=599) => return ModelFailureDisposition::Retryable,
        _ => {}
    }

    if let Some(code) = code {
        if RETRYABLE_CODES
            .iter()
            .any(|allowed| code.eq_ignore_ascii_case(allowed))
        {
            return ModelFailureDisposition::Retryable;
        }
        if NON_RETRYABLE_CODES
            .iter()
            .any(|allowed| code.eq_ignore_ascii_case(allowed))
        {
            return ModelFailureDisposition::NonRetryable;
        }
    }

    match category {
        ModelFailureCategory::Timeout
        | ModelFailureCategory::Transport
        | ModelFailureCategory::RateLimit => ModelFailureDisposition::Retryable,
        ModelFailureCategory::Authentication | ModelFailureCategory::Context => {
            ModelFailureDisposition::NonRetryable
        }
        // Local decoding/request-shape errors are deterministic. An equivalent
        // streamed error without typed evidence remains unknown.
        ModelFailureCategory::Response if boundary == ModelFailureBoundary::Local => {
            ModelFailureDisposition::NonRetryable
        }
        ModelFailureCategory::Response
        | ModelFailureCategory::Provider
        | ModelFailureCategory::RedactedUnknown => ModelFailureDisposition::Unknown,
    }
}
