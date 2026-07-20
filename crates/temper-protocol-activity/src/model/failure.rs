//! Canonical safe diagnostics for failed model calls.

use serde::{Deserialize, Serialize};

use crate::{ActivityValidationError, normalize_model_failure, validate_model_failure};

/// Maximum UTF-8 byte length of a provider identifier in a model failure.
pub const MAX_MODEL_FAILURE_PROVIDER_BYTES: usize = 128;
/// Maximum UTF-8 byte length of a model identifier in a model failure.
pub const MAX_MODEL_FAILURE_MODEL_BYTES: usize = 256;
/// Maximum UTF-8 byte length of a provider request ID.
pub const MAX_MODEL_FAILURE_REQUEST_ID_BYTES: usize = 128;
/// Maximum UTF-8 byte length of a provider error code.
pub const MAX_MODEL_FAILURE_PROVIDER_CODE_BYTES: usize = 64;
/// Maximum UTF-8 byte length of a sanitized provider message.
pub const MAX_MODEL_FAILURE_MESSAGE_BYTES: usize = 512;
/// The sole message used when diagnostic detail cannot be retained safely.
pub const REDACTED_MODEL_FAILURE_MESSAGE: &str = "Provider failure details were redacted.";
/// The non-sensitive identity used when a provider or model identifier is invalid.
pub const UNKNOWN_MODEL_FAILURE_IDENTITY: &str = "unknown";

/// Stable, provider-neutral model failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFailureCategoryV1 {
    Timeout,
    Transport,
    RateLimit,
    Authentication,
    Context,
    Response,
    Provider,
    RedactedUnknown,
}

impl ModelFailureCategoryV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::RateLimit => "rate_limit",
            Self::Authentication => "authentication",
            Self::Context => "context",
            Self::Response => "response",
            Self::Provider => "provider",
            Self::RedactedUnknown => "redacted_unknown",
        }
    }
}

/// A bounded provider/model diagnostic safe for canonical activity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelFailureV1 {
    pub provider: String,
    pub model: String,
    pub category: ModelFailureCategoryV1,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error_code: Option<String>,
    pub message: String,
    pub detail_redacted: bool,
}

impl ModelFailureV1 {
    /// Creates the fixed fail-closed diagnostic used when safe detail is absent.
    pub fn redacted_unknown(
        provider: impl Into<String>,
        model: impl Into<String>,
        retryable: bool,
    ) -> Self {
        let mut value = Self {
            provider: provider.into(),
            model: model.into(),
            category: ModelFailureCategoryV1::RedactedUnknown,
            retryable,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
            message: REDACTED_MODEL_FAILURE_MESSAGE.to_string(),
            detail_redacted: true,
        };
        normalize_model_failure(&mut value);
        value
    }

    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        validate_model_failure(self)
    }

    /// Rewrites malformed or unsafe detail to the fixed fail-closed form.
    pub fn normalize(&mut self) {
        normalize_model_failure(self);
    }

    /// Removes every child-controlled string before crossing a trust boundary.
    ///
    /// Character and length validation cannot establish the provenance of an
    /// otherwise valid identifier or message. Only non-textual facts that
    /// cannot carry provider content survive this conservative rewrite.
    pub fn redact_untrusted(&mut self) {
        let http_status = self
            .http_status
            .filter(|status| (100..=599).contains(status));
        *self = Self::redacted_unknown(
            UNKNOWN_MODEL_FAILURE_IDENTITY,
            UNKNOWN_MODEL_FAILURE_IDENTITY,
            self.retryable,
        );
        self.http_status = http_status;
    }
}
