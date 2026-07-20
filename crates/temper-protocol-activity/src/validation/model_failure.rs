use crate::{
    ActivityValidationCode, ActivityValidationError, MAX_MODEL_FAILURE_MESSAGE_BYTES,
    MAX_MODEL_FAILURE_MODEL_BYTES, MAX_MODEL_FAILURE_PROVIDER_BYTES,
    MAX_MODEL_FAILURE_PROVIDER_CODE_BYTES, MAX_MODEL_FAILURE_REQUEST_ID_BYTES,
    ModelFailureCategoryV1, ModelFailureV1, REDACTED_MODEL_FAILURE_MESSAGE,
    UNKNOWN_MODEL_FAILURE_IDENTITY,
};

use super::error;

/// Validates one canonical model failure independently of its enclosing event.
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
    Ok(())
}

/// Canonicalizes a model failure whose provenance has already been established.
///
/// Valid values are preserved (with harmless outer message whitespace removed).
/// Any malformed field rewrites the value to `redacted_unknown`. Independently
/// valid status and request-ID facts survive that rewrite. Untrusted activity
/// must additionally call [`ModelFailureV1::redact_untrusted`]; syntax alone
/// never establishes that diagnostic text is safe.
pub fn normalize_model_failure(value: &mut ModelFailureV1) {
    value.message = value.message.trim().to_string();
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

    *value = ModelFailureV1 {
        provider,
        model,
        category: ModelFailureCategoryV1::RedactedUnknown,
        retryable: value.retryable,
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
                || matches!(
                    character,
                    '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
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
