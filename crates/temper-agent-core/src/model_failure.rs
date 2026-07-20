//! Safe, provider-neutral diagnostics for failed model calls.
//!
//! Provider SDK errors are converted at the shell boundary. Unstructured
//! `Display` text is never retained: typed provider facts are copied from
//! tongs, while legacy/unclassified errors use a fixed redacted fallback.

use tongs::{FailureCategory, ProviderFailureDiagnostic};

use crate::shell::ModelIdentity;

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
    /// Stable snake-case representation used by downstream projections.
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

/// A bounded diagnostic safe to carry beyond the provider boundary.
///
/// Fields are private so arbitrary provider text cannot be inserted after the
/// tongs trust boundary. Downstream crates can inspect the allowlisted facts
/// through the accessors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelFailureDiagnostic {
    provider: String,
    model: String,
    category: ModelFailureCategory,
    retryable: bool,
    http_status: Option<u16>,
    provider_request_id: Option<String>,
    provider_error_code: Option<String>,
    message: String,
    detail_redacted: bool,
}

impl ModelFailureDiagnostic {
    /// Copies the validated provider contract into the agent-owned type.
    pub fn from_provider(identity: &ModelIdentity, diagnostic: &ProviderFailureDiagnostic) -> Self {
        Self {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            category: diagnostic.category().into(),
            retryable: diagnostic.retryable(),
            http_status: diagnostic.http_status(),
            provider_request_id: diagnostic.provider_request_id().map(str::to_owned),
            provider_error_code: diagnostic.provider_error_code().map(str::to_owned),
            message: diagnostic.message().to_owned(),
            detail_redacted: diagnostic.detail_redacted(),
        }
    }

    /// Creates the typed diagnostic for a Temper-enforced model deadline.
    pub(crate) fn timeout(identity: &ModelIdentity, message: &'static str) -> Self {
        Self::local(identity, ModelFailureCategory::Timeout, true, message)
    }

    /// Creates the typed diagnostic for a malformed or incomplete stream.
    pub(crate) fn response(
        identity: &ModelIdentity,
        retryable: bool,
        message: &'static str,
    ) -> Self {
        Self::local(identity, ModelFailureCategory::Response, retryable, message)
    }

    /// Creates the fixed fail-closed diagnostic when no structured detail is
    /// available. This is public primarily for compatibility fixtures and
    /// downstream trust-boundary normalization.
    pub fn redacted_unknown(
        provider: impl Into<String>,
        model: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            category: ModelFailureCategory::RedactedUnknown,
            retryable,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
            message: REDACTED_MODEL_FAILURE_MESSAGE.to_string(),
            detail_redacted: true,
        }
    }

    pub(crate) fn from_tongs_error(identity: &ModelIdentity, error: &tongs::Error) -> Self {
        if let Some(diagnostic) = error.diagnostic() {
            return Self::from_provider(identity, diagnostic);
        }

        use tongs::Error;
        match error {
            Error::Provider(_) => unreachable!("provider diagnostics returned above"),
            Error::Http(_) => {
                Self::known_redacted(identity, ModelFailureCategory::Transport, true, None)
            }
            Error::Api { status, .. } => {
                let (category, retryable) = match *status {
                    408 | 504 => (ModelFailureCategory::Timeout, true),
                    429 => (ModelFailureCategory::RateLimit, true),
                    401 | 403 => (ModelFailureCategory::Authentication, false),
                    500..=599 => (ModelFailureCategory::Provider, true),
                    _ => (ModelFailureCategory::Provider, false),
                };
                Self::known_redacted(identity, category, retryable, Some(*status))
            }
            Error::Auth(_) => {
                Self::known_redacted(identity, ModelFailureCategory::Authentication, false, None)
            }
            Error::Decode(_) => {
                Self::known_redacted(identity, ModelFailureCategory::Response, false, None)
            }
            Error::Tool(_) | Error::Aborted | Error::Other(_) => {
                Self::redacted_unknown(identity.provider.clone(), identity.model.clone(), false)
            }
        }
    }

    fn local(
        identity: &ModelIdentity,
        category: ModelFailureCategory,
        retryable: bool,
        message: &'static str,
    ) -> Self {
        debug_assert!(!message.trim().is_empty());
        debug_assert!(message.len() <= tongs::MAX_PROVIDER_MESSAGE_LEN);
        Self {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            category,
            retryable,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
            message: message.to_string(),
            detail_redacted: false,
        }
    }

    fn known_redacted(
        identity: &ModelIdentity,
        category: ModelFailureCategory,
        retryable: bool,
        http_status: Option<u16>,
    ) -> Self {
        Self {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            category,
            retryable,
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

    pub fn retryable(&self) -> bool {
        self.retryable
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
            "{}/{}: {} (retryable={}",
            self.provider, self.model, self.category, self.retryable
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
    fn provider_diagnostic_preserves_only_structured_facts() {
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

        assert_eq!(diagnostic.category(), ModelFailureCategory::RateLimit);
        assert!(diagnostic.retryable());
        assert_eq!(diagnostic.http_status(), Some(429));
        assert_eq!(diagnostic.provider_request_id(), Some("req_123"));
        assert_eq!(diagnostic.provider_error_code(), Some("rate_limit"));
        assert_eq!(diagnostic.message(), REDACTED_MODEL_FAILURE_MESSAGE);
        assert!(diagnostic.detail_redacted());
    }

    #[test]
    fn unstructured_errors_never_become_durable_text() {
        let identity = ModelIdentity::new("test", "model");
        let secret = "Authorization: Bearer SECRET_SENTINEL";
        let diagnostic = ModelFailureDiagnostic::from_tongs_error(
            &identity,
            &tongs::Error::Http(secret.to_string()),
        );

        assert_eq!(diagnostic.category(), ModelFailureCategory::Transport);
        assert_eq!(diagnostic.message(), REDACTED_MODEL_FAILURE_MESSAGE);
        assert!(diagnostic.detail_redacted());
        assert!(!format!("{diagnostic:?}").contains("SECRET_SENTINEL"));
    }
}
