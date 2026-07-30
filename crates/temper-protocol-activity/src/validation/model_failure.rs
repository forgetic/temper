use crate::{
    ActivityValidationCode, ActivityValidationError, MAX_MODEL_FAILURE_MESSAGE_BYTES,
    MAX_MODEL_FAILURE_MODEL_BYTES, MAX_MODEL_FAILURE_PROVIDER_BYTES,
    MAX_MODEL_FAILURE_PROVIDER_CODE_BYTES, MAX_MODEL_FAILURE_REQUEST_ID_BYTES,
    ModelFailureCategoryV1, ModelFailureDispositionV1, ModelFailureV1,
    REDACTED_MODEL_FAILURE_MESSAGE, UNKNOWN_MODEL_FAILURE_IDENTITY,
};

use super::error;

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
    "provider_error",
    "quota_exceeded",
    "request_too_large",
    "unauthorized",
    "usage_limit",
    "usage_limit_reached",
];

pub(crate) fn is_retryable_provider_code(code: &str) -> bool {
    RETRYABLE_CODES
        .iter()
        .any(|allowed| code.eq_ignore_ascii_case(allowed))
}

pub(crate) fn is_non_retryable_provider_code(code: &str) -> bool {
    NON_RETRYABLE_CODES
        .iter()
        .any(|allowed| code.eq_ignore_ascii_case(allowed))
}

fn is_allowlisted_provider_code(code: &str) -> bool {
    is_retryable_provider_code(code) || is_non_retryable_provider_code(code)
}

pub fn validate_model_failure(value: &ModelFailureV1) -> Result<(), ActivityValidationError> {
    validate_model_failure_at(value, "model_failure")
}

pub(crate) fn validate_model_failure_at(
    value: &ModelFailureV1,
    path: &str,
) -> Result<(), ActivityValidationError> {
    model_identity(
        &value.provider,
        MAX_MODEL_FAILURE_PROVIDER_BYTES,
        &format!("{path}.provider"),
    )?;
    model_identity(
        &value.model,
        MAX_MODEL_FAILURE_MODEL_BYTES,
        &format!("{path}.model"),
    )?;
    if value
        .http_status
        .is_some_and(|status| !(100..=599).contains(&status))
    {
        return Err(invalid(
            format!("{path}.http_status"),
            "must be between 100 and 599",
        ));
    }
    if value.http_status.is_some() && !value.status_present {
        return Err(invalid(
            format!("{path}.status_present"),
            "must be true when http_status is retained",
        ));
    }
    if let Some(request_id) = &value.provider_request_id {
        provider_identifier(
            request_id,
            MAX_MODEL_FAILURE_REQUEST_ID_BYTES,
            true,
            &format!("{path}.provider_request_id"),
        )?;
    }
    if let Some(code) = &value.provider_error_code {
        provider_identifier(
            code,
            MAX_MODEL_FAILURE_PROVIDER_CODE_BYTES,
            false,
            &format!("{path}.provider_error_code"),
        )?;
        if !is_allowlisted_provider_code(code) {
            return Err(invalid(
                format!("{path}.provider_error_code"),
                "is not an allowlisted provider code",
            ));
        }
        if !value.code_present {
            return Err(invalid(
                format!("{path}.code_present"),
                "must be true when provider_error_code is retained",
            ));
        }
    }
    safe_message(&value.message, &format!("{path}.message"))?;

    if value.category == ModelFailureCategoryV1::RedactedUnknown
        && (!value.detail_redacted || value.message != REDACTED_MODEL_FAILURE_MESSAGE)
    {
        return Err(invalid(
            format!("{path}.category"),
            "redacted_unknown must use the fixed redacted detail",
        ));
    }
    let expected = crate::model::canonical_disposition(
        value.category,
        value.boundary,
        value.http_status,
        value.provider_error_code.as_deref(),
    );
    if value.disposition != expected {
        return Err(invalid(
            format!("{path}.disposition"),
            format!(
                "must be {} for the retained typed evidence",
                expected.as_str()
            ),
        ));
    }
    if value.retryable != (value.disposition == ModelFailureDispositionV1::Retryable) {
        return Err(invalid(
            format!("{path}.retryable"),
            "must be the canonical disposition compatibility projection",
        ));
    }
    Ok(())
}

/// Canonicalizes established first-party evidence. Unknown code values are
/// reduced to `code_present=true`; malformed text rewrites to the fixed
/// redacted form without discarding independently typed status/request facts.
pub fn normalize_model_failure(value: &mut ModelFailureV1) {
    value.message = value.message.trim().to_string();
    value.status_present |= value.http_status.is_some();
    value.code_present |= value.provider_error_code.is_some();
    if value
        .provider_error_code
        .as_deref()
        .is_some_and(|code| !is_allowlisted_provider_code(code))
    {
        value.provider_error_code = None;
    }
    value.disposition = crate::model::canonical_disposition(
        value.category,
        value.boundary,
        value.http_status,
        value.provider_error_code.as_deref(),
    );
    value.retryable = value.disposition == ModelFailureDispositionV1::Retryable;
    if validate_model_failure(value).is_ok() {
        return;
    }

    let provider = normalized_identity(
        &value.provider,
        MAX_MODEL_FAILURE_PROVIDER_BYTES,
        UNKNOWN_MODEL_FAILURE_IDENTITY,
    );
    let model = normalized_identity(
        &value.model,
        MAX_MODEL_FAILURE_MODEL_BYTES,
        UNKNOWN_MODEL_FAILURE_IDENTITY,
    );
    let http_status = value
        .http_status
        .filter(|status| (100..=599).contains(status));
    let provider_request_id = value
        .provider_request_id
        .as_deref()
        .filter(|request_id| {
            valid_provider_identifier(request_id, MAX_MODEL_FAILURE_REQUEST_ID_BYTES, true)
        })
        .map(str::to_owned);
    let boundary = value.boundary;
    let disposition = crate::model::canonical_disposition(
        ModelFailureCategoryV1::RedactedUnknown,
        boundary,
        http_status,
        None,
    );

    *value = ModelFailureV1 {
        provider,
        model,
        category: ModelFailureCategoryV1::RedactedUnknown,
        disposition,
        boundary,
        event_kind: value.event_kind,
        status_present: value.status_present || http_status.is_some(),
        code_present: value.code_present,
        retryable: disposition == ModelFailureDispositionV1::Retryable,
        http_status,
        provider_request_id,
        provider_error_code: None,
        message: REDACTED_MODEL_FAILURE_MESSAGE.to_string(),
        detail_redacted: true,
    };
}

fn normalized_identity(value: &str, max_bytes: usize, fallback: &str) -> String {
    if valid_model_identity(value, max_bytes) {
        value.to_string()
    } else {
        fallback.to_string()
    }
}

fn model_identity(
    value: &str,
    max_bytes: usize,
    path: &str,
) -> Result<(), ActivityValidationError> {
    if valid_model_identity(value, max_bytes) {
        Ok(())
    } else {
        Err(invalid(
            path,
            format!(
                "must be 1..={max_bytes} ASCII bytes using only letters, digits, '-', '_', '.', ':', or '/'"
            ),
        ))
    }
}

fn valid_model_identity(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
}

fn provider_identifier(
    value: &str,
    max_bytes: usize,
    request_id: bool,
    path: &str,
) -> Result<(), ActivityValidationError> {
    if valid_provider_identifier(value, max_bytes, request_id) {
        Ok(())
    } else {
        Err(invalid(
            path,
            format!("must be 1..={max_bytes} bytes using the provider identifier character set"),
        ))
    }
}

fn valid_provider_identifier(value: &str, max_bytes: usize, request_id: bool) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.')
                || (request_id && matches!(byte, b':' | b'/'))
        })
}

fn safe_message(value: &str, path: &str) -> Result<(), ActivityValidationError> {
    if value.trim().is_empty() {
        return Err(invalid(path, "must not be blank"));
    }
    if value.len() > MAX_MODEL_FAILURE_MESSAGE_BYTES {
        return Err(error(
            ActivityValidationCode::OversizedInlineValue,
            path,
            format!("exceeds {MAX_MODEL_FAILURE_MESSAGE_BYTES} UTF-8 bytes"),
        ));
    }
    if value != value.trim()
        || value.chars().any(|character| {
            character.is_control()
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        || contains_sensitive_marker(value)
    {
        return Err(invalid(path, "is not a safe canonical message"));
    }
    Ok(())
}

fn contains_sensitive_marker(value: &str) -> bool {
    let folded = value.to_ascii_lowercase();
    const SENSITIVE_MARKERS: [&str; 8] = [
        "authorization:",
        "bearer ",
        "x-api-key",
        "api_key",
        "api key",
        "access token",
        "refresh token",
        "set-cookie:",
    ];
    SENSITIVE_MARKERS
        .iter()
        .any(|marker| folded.contains(marker))
}

fn invalid(field: impl Into<String>, detail: impl Into<String>) -> ActivityValidationError {
    error(ActivityValidationCode::InvalidEvent, field, detail)
}
