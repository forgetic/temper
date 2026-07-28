//! Canonical safe diagnostics for failed model calls.

use serde::{Deserialize, Deserializer, Serialize};

use crate::{ActivityValidationError, normalize_model_failure, validate_model_failure};

pub const MAX_MODEL_FAILURE_PROVIDER_BYTES: usize = 128;
pub const MAX_MODEL_FAILURE_MODEL_BYTES: usize = 256;
pub const MAX_MODEL_FAILURE_REQUEST_ID_BYTES: usize = 128;
pub const MAX_MODEL_FAILURE_PROVIDER_CODE_BYTES: usize = 64;
pub const MAX_MODEL_FAILURE_MESSAGE_BYTES: usize = 512;
pub const REDACTED_MODEL_FAILURE_MESSAGE: &str = "Provider failure details were redacted.";
pub const UNKNOWN_MODEL_FAILURE_IDENTITY: &str = "unknown";

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

/// Canonical and sole recovery authority for a model failure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFailureDispositionV1 {
    Retryable,
    NonRetryable,
    #[default]
    Unknown,
}

impl ModelFailureDispositionV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::NonRetryable => "non_retryable",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFailureBoundaryV1 {
    Http,
    Sse,
    #[default]
    Local,
}

impl ModelFailureBoundaryV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Sse => "sse",
            Self::Local => "local",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFailureEventKindV1 {
    HttpResponse,
    StreamError,
    ErrorCompletion,
    StreamEof,
    ConnectTimeout,
    StreamIdleTimeout,
    Transport,
    #[default]
    LocalError,
}

impl ModelFailureEventKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HttpResponse => "http_response",
            Self::StreamError => "stream_error",
            Self::ErrorCompletion => "error_completion",
            Self::StreamEof => "stream_eof",
            Self::ConnectTimeout => "connect_timeout",
            Self::StreamIdleTimeout => "stream_idle_timeout",
            Self::Transport => "transport",
            Self::LocalError => "local_error",
        }
    }
}

/// A bounded provider/model diagnostic safe for activity, terminal output, and
/// worker result transport. `retryable` is emitted only as a compatibility
/// projection and is never consulted as recovery authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelFailureV1 {
    pub provider: String,
    pub model: String,
    pub category: ModelFailureCategoryV1,
    pub disposition: ModelFailureDispositionV1,
    pub boundary: ModelFailureBoundaryV1,
    pub event_kind: ModelFailureEventKindV1,
    pub status_present: bool,
    pub code_present: bool,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelFailureWire {
    provider: String,
    model: String,
    category: ModelFailureCategoryV1,
    #[serde(default)]
    disposition: Option<ModelFailureDispositionV1>,
    #[serde(default)]
    boundary: Option<ModelFailureBoundaryV1>,
    #[serde(default)]
    event_kind: Option<ModelFailureEventKindV1>,
    #[serde(default)]
    status_present: Option<bool>,
    #[serde(default)]
    code_present: Option<bool>,
    // Read only for wire compatibility. Canonical disposition is reconstructed
    // from typed evidence so this legacy boolean cannot remain an authority.
    #[serde(default, rename = "retryable")]
    _retryable: bool,
    #[serde(default)]
    http_status: Option<u16>,
    #[serde(default)]
    provider_request_id: Option<String>,
    #[serde(default)]
    provider_error_code: Option<String>,
    message: String,
    detail_redacted: bool,
}

impl<'de> Deserialize<'de> for ModelFailureV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ModelFailureWire::deserialize(deserializer)?;
        let boundary = wire.boundary.unwrap_or(if wire.http_status.is_some() {
            ModelFailureBoundaryV1::Http
        } else {
            ModelFailureBoundaryV1::Local
        });
        let disposition = wire.disposition.unwrap_or_else(|| {
            canonical_disposition(
                wire.category,
                boundary,
                wire.http_status,
                wire.provider_error_code.as_deref(),
            )
        });
        Ok(Self {
            provider: wire.provider,
            model: wire.model,
            category: wire.category,
            disposition,
            boundary,
            event_kind: wire
                .event_kind
                .unwrap_or(if boundary == ModelFailureBoundaryV1::Http {
                    ModelFailureEventKindV1::HttpResponse
                } else {
                    ModelFailureEventKindV1::LocalError
                }),
            status_present: wire.status_present.unwrap_or(wire.http_status.is_some()),
            code_present: wire
                .code_present
                .unwrap_or(wire.provider_error_code.is_some()),
            retryable: disposition == ModelFailureDispositionV1::Retryable,
            http_status: wire.http_status,
            provider_request_id: wire.provider_request_id,
            provider_error_code: wire.provider_error_code,
            message: wire.message,
            detail_redacted: wire.detail_redacted,
        })
    }
}

impl ModelFailureV1 {
    pub fn unknown(
        provider: impl Into<String>,
        model: impl Into<String>,
        boundary: ModelFailureBoundaryV1,
        event_kind: ModelFailureEventKindV1,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            category: ModelFailureCategoryV1::RedactedUnknown,
            disposition: ModelFailureDispositionV1::Unknown,
            boundary,
            event_kind,
            status_present: false,
            code_present: false,
            retryable: false,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
            message: REDACTED_MODEL_FAILURE_MESSAGE.to_string(),
            detail_redacted: true,
        }
    }

    /// Compatibility constructor. Legacy `retryable=false` is not interpreted
    /// as proof of a permanent failure.
    pub fn redacted_unknown(
        provider: impl Into<String>,
        model: impl Into<String>,
        _legacy_retryable: bool,
    ) -> Self {
        Self::unknown(
            provider,
            model,
            ModelFailureBoundaryV1::Local,
            ModelFailureEventKindV1::LocalError,
        )
    }

    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        validate_model_failure(self)
    }

    pub fn normalize(&mut self) {
        normalize_model_failure(self);
    }

    pub fn redact_untrusted(&mut self) {
        self.redact_untrusted_with_identity(
            UNKNOWN_MODEL_FAILURE_IDENTITY,
            UNKNOWN_MODEL_FAILURE_IDENTITY,
        );
    }

    /// Removes child-controlled strings while attributing identity from trusted
    /// launch context supplied by the host.
    pub fn redact_untrusted_with_identity(&mut self, provider: &str, model: &str) {
        let boundary = self.boundary;
        let event_kind = self.event_kind;
        let http_status = self
            .http_status
            .filter(|status| (100..=599).contains(status));
        let status_present = self.status_present || http_status.is_some();
        let code_present = self.code_present || self.provider_error_code.is_some();
        *self = Self::unknown(provider, model, boundary, event_kind);
        self.http_status = http_status;
        self.status_present = status_present;
        self.code_present = code_present;
        self.disposition = canonical_disposition(self.category, boundary, http_status, None);
        self.retryable = self.disposition == ModelFailureDispositionV1::Retryable;
    }
}

pub(crate) fn canonical_disposition(
    category: ModelFailureCategoryV1,
    boundary: ModelFailureBoundaryV1,
    status: Option<u16>,
    code: Option<&str>,
) -> ModelFailureDispositionV1 {
    match status {
        Some(408 | 504) | Some(429) | Some(500..=599) => {
            return ModelFailureDispositionV1::Retryable;
        }
        Some(401 | 403) => return ModelFailureDispositionV1::NonRetryable,
        _ => {}
    }
    if let Some(code) = code {
        if crate::validation::is_retryable_provider_code(code) {
            return ModelFailureDispositionV1::Retryable;
        }
        if crate::validation::is_non_retryable_provider_code(code) {
            return ModelFailureDispositionV1::NonRetryable;
        }
    }
    match category {
        ModelFailureCategoryV1::Timeout
        | ModelFailureCategoryV1::Transport
        | ModelFailureCategoryV1::RateLimit => ModelFailureDispositionV1::Retryable,
        ModelFailureCategoryV1::Authentication | ModelFailureCategoryV1::Context => {
            ModelFailureDispositionV1::NonRetryable
        }
        ModelFailureCategoryV1::Response if boundary == ModelFailureBoundaryV1::Local => {
            ModelFailureDispositionV1::NonRetryable
        }
        ModelFailureCategoryV1::Response
        | ModelFailureCategoryV1::Provider
        | ModelFailureCategoryV1::RedactedUnknown => ModelFailureDispositionV1::Unknown,
    }
}
