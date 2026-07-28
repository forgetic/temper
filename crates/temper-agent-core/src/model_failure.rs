//! Safe, provider-neutral diagnostics for failed model calls.
//!
//! Provider SDK errors are converted at the shell boundary. Unstructured
//! `Display` text is never retained: typed provider facts are copied from
//! tongs, while legacy/unclassified errors use a fixed redacted fallback.

use tongs::{FailureCategory, ProviderFailureDiagnostic};

use crate::shell::ModelIdentity;

#[path = "model_failure/classification.rs"]
mod classification;

/// The only message retained when structured provider detail is unavailable.
pub const REDACTED_MODEL_FAILURE_MESSAGE: &str = tongs::REDACTED_PROVIDER_MESSAGE;

/// Stable provider-neutral failure categories exposed by the native agent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFailureCategory {
    Timeout,
    Transport,
    RateLimit,
    Authentication,
    Context,
    Response,
    Provider,
    RedactedUnknown,
}

impl ModelFailureCategory {
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

impl std::fmt::Display for ModelFailureCategory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<FailureCategory> for ModelFailureCategory {
    fn from(category: FailureCategory) -> Self {
        match category {
            FailureCategory::Timeout => Self::Timeout,
            FailureCategory::Transport => Self::Transport,
            FailureCategory::RateLimit => Self::RateLimit,
            FailureCategory::Authentication => Self::Authentication,
            FailureCategory::Context => Self::Context,
            FailureCategory::Response => Self::Response,
            FailureCategory::Provider => Self::Provider,
            FailureCategory::RedactedUnknown => Self::RedactedUnknown,
        }
    }
}

/// Canonical recovery meaning. This is the sole retry authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFailureDisposition {
    Retryable,
    NonRetryable,
    Unknown,
}

impl ModelFailureDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::NonRetryable => "non_retryable",
            Self::Unknown => "unknown",
        }
    }
}

/// Boundary at which the retained evidence was established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFailureBoundary {
    Http,
    Sse,
    Local,
}

impl ModelFailureBoundary {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Sse => "sse",
            Self::Local => "local",
        }
    }
}

/// Bounded event vocabulary for provider/model failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFailureEventKind {
    HttpResponse,
    StreamError,
    ErrorCompletion,
    StreamEof,
    ConnectTimeout,
    StreamIdleTimeout,
    Transport,
    LocalError,
}

impl ModelFailureEventKind {
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

/// A bounded diagnostic safe to carry beyond the provider boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelFailureDiagnostic {
    provider: String,
    model: String,
    category: ModelFailureCategory,
    disposition: ModelFailureDisposition,
    boundary: ModelFailureBoundary,
    event_kind: ModelFailureEventKind,
    status_present: bool,
    code_present: bool,
    http_status: Option<u16>,
    provider_request_id: Option<String>,
    provider_error_code: Option<String>,
    message: String,
    detail_redacted: bool,
}

impl ModelFailureDiagnostic {
    /// Copies a validated provider contract, inferring the strongest boundary
    /// available for compatibility callers.
    pub fn from_provider(identity: &ModelIdentity, diagnostic: &ProviderFailureDiagnostic) -> Self {
        let boundary = if diagnostic.http_status().is_some() {
            ModelFailureBoundary::Http
        } else {
            ModelFailureBoundary::Local
        };
        let event_kind = if boundary == ModelFailureBoundary::Http {
            ModelFailureEventKind::HttpResponse
        } else {
            ModelFailureEventKind::LocalError
        };
        Self::from_provider_at(identity, diagnostic, boundary, event_kind)
    }

    pub(crate) fn from_stream_event(
        identity: &ModelIdentity,
        diagnostic: &ProviderFailureDiagnostic,
    ) -> Self {
        Self::from_provider_at(
            identity,
            diagnostic,
            ModelFailureBoundary::Sse,
            ModelFailureEventKind::StreamError,
        )
    }

    fn from_provider_at(
        identity: &ModelIdentity,
        diagnostic: &ProviderFailureDiagnostic,
        boundary: ModelFailureBoundary,
        event_kind: ModelFailureEventKind,
    ) -> Self {
        let category = diagnostic.category().into();
        let code_present = diagnostic.provider_error_code().is_some();
        let provider_error_code = diagnostic
            .provider_error_code()
            .filter(|code| classification::is_allowlisted_code(code))
            .map(str::to_owned);
        let disposition = classification::disposition(
            category,
            boundary,
            diagnostic.http_status(),
            provider_error_code.as_deref(),
        );
        Self {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            category,
            disposition,
            boundary,
            event_kind,
            status_present: diagnostic.http_status().is_some(),
            code_present,
            http_status: diagnostic.http_status(),
            provider_request_id: diagnostic.provider_request_id().map(str::to_owned),
            provider_error_code,
            message: diagnostic.message().to_owned(),
            detail_redacted: diagnostic.detail_redacted(),
        }
    }

    pub(crate) fn timeout(
        identity: &ModelIdentity,
        event_kind: ModelFailureEventKind,
        message: &'static str,
    ) -> Self {
        Self::local(
            identity,
            ModelFailureCategory::Timeout,
            ModelFailureBoundary::Local,
            event_kind,
            message,
        )
    }

    pub(crate) fn response(
        identity: &ModelIdentity,
        boundary: ModelFailureBoundary,
        event_kind: ModelFailureEventKind,
        message: &'static str,
    ) -> Self {
        Self::local(
            identity,
            ModelFailureCategory::Response,
            boundary,
            event_kind,
            message,
        )
    }

    /// Creates an explicitly unknown diagnostic from trusted launch identity.
    pub fn unknown(
        provider: impl Into<String>,
        model: impl Into<String>,
        boundary: ModelFailureBoundary,
        event_kind: ModelFailureEventKind,
    ) -> Self {
        let provider = provider.into();
        let model = model.into();
        Self::local(
            &ModelIdentity::new(provider, model),
            ModelFailureCategory::RedactedUnknown,
            boundary,
            event_kind,
            REDACTED_MODEL_FAILURE_MESSAGE,
        )
    }

    /// Compatibility constructor. A legacy `false` is deliberately projected
    /// to unknown rather than becoming evidence of a permanent failure.
    pub fn redacted_unknown(
        provider: impl Into<String>,
        model: impl Into<String>,
        _legacy_retryable: bool,
    ) -> Self {
        Self::unknown(
            provider,
            model,
            ModelFailureBoundary::Local,
            ModelFailureEventKind::LocalError,
        )
    }

    pub(crate) fn from_tongs_error(identity: &ModelIdentity, error: &tongs::Error) -> Self {
        if let Some(diagnostic) = error.diagnostic() {
            return Self::from_provider(identity, diagnostic);
        }

        use tongs::Error;
        match error {
            Error::Provider(_) => unreachable!("provider diagnostics returned above"),
            Error::Http(_) => Self::local(
                identity,
                ModelFailureCategory::Transport,
                ModelFailureBoundary::Local,
                ModelFailureEventKind::Transport,
                REDACTED_MODEL_FAILURE_MESSAGE,
            ),
            Error::Api { status, .. } => {
                let category = match *status {
                    408 | 504 => ModelFailureCategory::Timeout,
                    429 => ModelFailureCategory::RateLimit,
                    401 | 403 => ModelFailureCategory::Authentication,
                    _ => ModelFailureCategory::Provider,
                };
                Self::known_redacted(identity, category, Some(*status))
            }
            Error::Auth(_) => Self::local(
                identity,
                ModelFailureCategory::Authentication,
                ModelFailureBoundary::Local,
                ModelFailureEventKind::LocalError,
                REDACTED_MODEL_FAILURE_MESSAGE,
            ),
            Error::Decode(_) => Self::local(
                identity,
                ModelFailureCategory::Response,
                ModelFailureBoundary::Local,
                ModelFailureEventKind::LocalError,
                REDACTED_MODEL_FAILURE_MESSAGE,
            ),
            Error::Tool(_) | Error::Aborted | Error::Other(_) => Self::unknown(
                identity.provider.clone(),
                identity.model.clone(),
                ModelFailureBoundary::Local,
                ModelFailureEventKind::LocalError,
            ),
        }
    }

    fn local(
        identity: &ModelIdentity,
        category: ModelFailureCategory,
        boundary: ModelFailureBoundary,
        event_kind: ModelFailureEventKind,
        message: &'static str,
    ) -> Self {
        debug_assert!(!message.trim().is_empty());
        let disposition = classification::disposition(category, boundary, None, None);
        Self {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            category,
            disposition,
            boundary,
            event_kind,
            status_present: false,
            code_present: false,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
            message: message.to_string(),
            detail_redacted: message == REDACTED_MODEL_FAILURE_MESSAGE,
        }
    }

    fn known_redacted(
        identity: &ModelIdentity,
        category: ModelFailureCategory,
        http_status: Option<u16>,
    ) -> Self {
        let boundary = ModelFailureBoundary::Http;
        Self {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            category,
            disposition: classification::disposition(category, boundary, http_status, None),
            boundary,
            event_kind: ModelFailureEventKind::HttpResponse,
            status_present: http_status.is_some(),
            code_present: false,
            http_status,
            provider_request_id: None,
            provider_error_code: None,
            message: REDACTED_MODEL_FAILURE_MESSAGE.to_string(),
            detail_redacted: true,
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn category(&self) -> ModelFailureCategory {
        self.category
    }
    pub fn disposition(&self) -> ModelFailureDisposition {
        self.disposition
    }
    pub fn boundary(&self) -> ModelFailureBoundary {
        self.boundary
    }
    pub fn event_kind(&self) -> ModelFailureEventKind {
        self.event_kind
    }
    pub fn status_present(&self) -> bool {
        self.status_present
    }
    pub fn code_present(&self) -> bool {
        self.code_present
    }
    /// Compatibility projection; canonical decisions use [`Self::disposition`].
    pub fn retryable(&self) -> bool {
        self.disposition == ModelFailureDisposition::Retryable
    }
    pub fn eligible_for_turn_retry(&self) -> bool {
        self.disposition == ModelFailureDisposition::Retryable
            || (self.disposition == ModelFailureDisposition::Unknown
                && self.boundary == ModelFailureBoundary::Sse
                && matches!(
                    self.event_kind,
                    ModelFailureEventKind::StreamError | ModelFailureEventKind::StreamEof
                ))
    }
    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }
    pub fn provider_request_id(&self) -> Option<&str> {
        self.provider_request_id.as_deref()
    }
    pub fn provider_error_code(&self) -> Option<&str> {
        self.provider_error_code.as_deref()
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn detail_redacted(&self) -> bool {
        self.detail_redacted
    }
}

impl std::fmt::Display for ModelFailureDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}/{}: {} (disposition={}, boundary={}, event_kind={}",
            self.provider,
            self.model,
            self.category,
            self.disposition.as_str(),
            self.boundary.as_str(),
            self.event_kind.as_str(),
        )?;
        if let Some(status) = self.http_status {
            write!(formatter, ", http_status={status}")?;
        }
        if let Some(request_id) = &self.provider_request_id {
            write!(formatter, ", request_id={request_id}")?;
        }
        if let Some(code) = &self.provider_error_code {
            write!(formatter, ", provider_code={code}")?;
        }
        write!(formatter, "): {}", self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_diagnostic_preserves_only_allowlisted_structured_facts() {
        let upstream = ProviderFailureDiagnostic::new(
            FailureCategory::RateLimit,
            true,
            Some(429),
            Some("req_123"),
            Some("rate_limit"),
            "Please retry later.",
        );
        let diagnostic = ModelFailureDiagnostic::from_provider(
            &ModelIdentity::new("openai", "gpt-test"),
            &upstream,
        );

        assert_eq!(diagnostic.disposition(), ModelFailureDisposition::Retryable);
        assert_eq!(diagnostic.boundary(), ModelFailureBoundary::Http);
        assert_eq!(diagnostic.http_status(), Some(429));
        assert_eq!(diagnostic.provider_request_id(), Some("req_123"));
        assert_eq!(diagnostic.provider_error_code(), Some("rate_limit"));
        assert_eq!(diagnostic.message(), REDACTED_MODEL_FAILURE_MESSAGE);
    }

    #[test]
    fn unclassified_stream_is_unknown_but_http_4xx_is_not_turn_retryable() {
        let identity = ModelIdentity::new("test", "model");
        let upstream = ProviderFailureDiagnostic::new(
            FailureCategory::RedactedUnknown,
            false,
            None,
            Some("req_stream"),
            Some("future_code"),
            "unsafe",
        );
        let streamed = ModelFailureDiagnostic::from_stream_event(&identity, &upstream);
        assert_eq!(streamed.disposition(), ModelFailureDisposition::Unknown);
        assert!(streamed.eligible_for_turn_retry());
        assert!(streamed.code_present());
        assert_eq!(streamed.provider_error_code(), None);

        let http = ModelFailureDiagnostic::from_tongs_error(
            &identity,
            &tongs::Error::Api {
                status: 418,
                message: "unsafe".into(),
            },
        );
        assert_eq!(http.disposition(), ModelFailureDisposition::Unknown);
        assert_eq!(http.boundary(), ModelFailureBoundary::Http);
        assert!(!http.eligible_for_turn_retry());
    }
}
