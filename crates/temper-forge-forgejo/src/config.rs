//! Forgejo backend configuration.
//!
//! This crate is a library, not a process boundary: it never reads the process
//! environment. Callers build a [`ForgejoConfig`] from explicit values.

use std::collections::BTreeSet;

/// Default page size for paginated list requests.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Strategy for conditional (compare-and-swap) writes.
///
/// Forgejo does not yet expose a confirmed provider conditional-write contract,
/// so the backend's optimistic concurrency is best-effort. The mode selects
/// what happens when no provider validator is captured for an artifact.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum CasMode {
    /// Refuse conditional writes without a captured validator
    /// ([`temper_forge_model::ForgeError::InvalidRequest`]). Favors safety.
    Strict,
    /// Fall back to a documented weak read-before-write when no validator
    /// exists. Favors availability; the residual race is documented.
    #[default]
    BestEffort,
}

/// Explicit authenticated source for protected-workflow ordinary-failure proofs.
///
/// This is deliberately independent of the Forgejo REST identity. `bearer_token`
/// authenticates acquisition from the generic JSON endpoint and `hmac_key`
/// verifies each producer statement. Neither value is read from the environment
/// or exposed by `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct ForgejoFailureEvidenceConfig {
    endpoint: String,
    bearer_token: String,
    hmac_key: String,
    issuer_id: String,
    protected_producers: BTreeSet<String>,
}

impl std::fmt::Debug for ForgejoFailureEvidenceConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForgejoFailureEvidenceConfig")
            .field("endpoint", &self.endpoint)
            .field("bearer_token", &"[REDACTED]")
            .field("hmac_key", &"[REDACTED]")
            .field("issuer_id", &self.issuer_id)
            .field("protected_producers", &self.protected_producers)
            .finish()
    }
}

impl ForgejoFailureEvidenceConfig {
    /// Constructs a closed evidence-source configuration.
    ///
    /// Remote sources must use HTTPS. Plain HTTP is accepted only for an
    /// explicit loopback endpoint, matching the single-host Forgejo runner
    /// topology without putting either credential on a network link.
    pub fn new(
        endpoint: impl Into<String>,
        bearer_token: impl Into<String>,
        hmac_key: impl Into<String>,
        issuer_id: impl Into<String>,
        protected_producers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, String> {
        let endpoint = endpoint.into();
        let bearer_token = bearer_token.into();
        let hmac_key = hmac_key.into();
        let issuer_id = issuer_id.into().trim().to_string();
        let protected_producers = protected_producers
            .into_iter()
            .map(|value| value.into().trim().to_string())
            .collect::<BTreeSet<_>>();
        if !valid_evidence_endpoint(&endpoint) {
            return Err("CI failure evidence endpoint must be HTTPS or loopback HTTP and contain no query, fragment, user info, or whitespace".to_string());
        }
        if bearer_token.trim().is_empty() || hmac_key.trim().is_empty() {
            return Err("CI failure evidence authentication secrets must not be empty".to_string());
        }
        if !valid_identity(&issuer_id)
            || protected_producers.is_empty()
            || protected_producers
                .iter()
                .any(|value| !valid_identity(value))
        {
            return Err("CI failure evidence issuer and protected producer identities must be non-empty bounded identifiers".to_string());
        }
        Ok(Self {
            endpoint,
            bearer_token,
            hmac_key,
            issuer_id,
            protected_producers,
        })
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn bearer_token(&self) -> &str {
        &self.bearer_token
    }

    pub(crate) fn hmac_key(&self) -> &str {
        &self.hmac_key
    }

    pub(crate) fn issuer_id(&self) -> &str {
        &self.issuer_id
    }

    pub(crate) fn authorizes_producer(&self, producer: &str) -> bool {
        self.protected_producers.contains(producer)
    }
}

/// Configuration for the Forgejo backend.
#[derive(Clone, Eq, PartialEq)]
pub struct ForgejoConfig {
    /// Forgejo base URL with no trailing slash, e.g. `https://git.example.com`.
    pub base_url: String,
    /// Personal access token sent as `Authorization: token <token>`.
    pub token: String,
    /// Optional default repository owner used when callers omit one.
    pub default_owner: Option<String>,
    /// Optional default repository name used when callers omit one.
    pub default_name: Option<String>,
    /// Page size for paginated list requests.
    pub page_limit: u32,
    /// Conditional-write strategy.
    pub cas_mode: CasMode,
    /// Optional explicitly configured ordinary-failure proof source.
    pub failure_evidence: Option<ForgejoFailureEvidenceConfig>,
}

impl std::fmt::Debug for ForgejoConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForgejoConfig")
            .field("base_url", &self.base_url)
            .field("token", &"[REDACTED]")
            .field("default_owner", &self.default_owner)
            .field("default_name", &self.default_name)
            .field("page_limit", &self.page_limit)
            .field("cas_mode", &self.cas_mode)
            .field("failure_evidence", &self.failure_evidence)
            .finish()
    }
}

impl ForgejoConfig {
    /// Builds a configuration from a base URL and token, applying defaults.
    ///
    /// Trailing slashes are stripped from `base_url` so request paths join
    /// cleanly.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base_url: strip_trailing_slashes(base_url.into()),
            token: token.into(),
            default_owner: None,
            default_name: None,
            page_limit: DEFAULT_PAGE_LIMIT,
            cas_mode: CasMode::default(),
            failure_evidence: None,
        }
    }

    /// Sets the default repository owner/name pair.
    pub fn with_default_repo(mut self, owner: impl Into<String>, name: impl Into<String>) -> Self {
        self.default_owner = Some(owner.into());
        self.default_name = Some(name.into());
        self
    }

    /// Sets the page size for list requests.
    pub fn with_page_limit(mut self, page_limit: u32) -> Self {
        self.page_limit = page_limit;
        self
    }

    /// Sets the conditional-write strategy.
    pub fn with_cas_mode(mut self, cas_mode: CasMode) -> Self {
        self.cas_mode = cas_mode;
        self
    }

    /// Enables the one supported generic ordinary-failure evidence transport.
    pub fn with_failure_evidence(mut self, config: ForgejoFailureEvidenceConfig) -> Self {
        self.failure_evidence = Some(config);
        self
    }
}

fn strip_trailing_slashes(value: String) -> String {
    value.trim_end_matches('/').to_string()
}

fn valid_evidence_endpoint(value: &str) -> bool {
    if value.trim() != value
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
        || value.contains(['?', '#'])
    {
        return false;
    }
    let (secure, remainder) = if let Some(remainder) = value.strip_prefix("https://") {
        (true, remainder)
    } else if let Some(remainder) = value.strip_prefix("http://") {
        (false, remainder)
    } else {
        return false;
    };
    let authority = remainder.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    secure || loopback_authority(authority)
}

fn loopback_authority(authority: &str) -> bool {
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    matches!(host, "localhost" | "127.0.0.1")
        && port
            .is_none_or(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
}

fn valid_identity(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_strips_trailing_slashes_and_applies_defaults() {
        let config = ForgejoConfig::new("https://git.example.com///", "tok");
        assert_eq!(config.base_url, "https://git.example.com");
        assert_eq!(config.token, "tok");
        assert_eq!(config.default_owner, None);
        assert_eq!(config.page_limit, DEFAULT_PAGE_LIMIT);
        assert_eq!(config.cas_mode, CasMode::BestEffort);
        assert_eq!(config.failure_evidence, None);
    }

    #[test]
    fn builders_set_explicit_values() {
        let config = ForgejoConfig::new("https://git.example.com/", "secret")
            .with_default_repo("acme", "widgets")
            .with_page_limit(25)
            .with_cas_mode(CasMode::Strict);

        assert_eq!(config.base_url, "https://git.example.com");
        assert_eq!(config.token, "secret");
        assert_eq!(config.default_owner.as_deref(), Some("acme"));
        assert_eq!(config.default_name.as_deref(), Some("widgets"));
        assert_eq!(config.page_limit, 25);
        assert_eq!(config.cas_mode, CasMode::Strict);
    }

    #[test]
    fn evidence_source_is_explicit_validated_and_secret_redacted() {
        let evidence = ForgejoFailureEvidenceConfig::new(
            "https://evidence.example/v1/failures",
            "acquisition-secret",
            "integrity-secret",
            "runner-host",
            ["protected-workflow"],
        )
        .unwrap();
        let config = ForgejoConfig::new("https://forge.example", "forge-secret")
            .with_failure_evidence(evidence);
        let debug = format!("{config:?}");
        assert!(debug.contains("runner-host"));
        assert!(!debug.contains("forge-secret"));
        assert!(!debug.contains("acquisition-secret"));
        assert!(!debug.contains("integrity-secret"));

        assert!(
            ForgejoFailureEvidenceConfig::new(
                "http://remote.example/failures",
                "token",
                "key",
                "issuer",
                ["producer"]
            )
            .is_err()
        );
        assert!(
            ForgejoFailureEvidenceConfig::new(
                "http://localhost.evil.example/failures",
                "token",
                "key",
                "issuer",
                ["producer"]
            )
            .is_err()
        );
        assert!(
            ForgejoFailureEvidenceConfig::new(
                "https:///missing-authority",
                "token",
                "key",
                "issuer",
                ["producer"]
            )
            .is_err()
        );
        assert!(
            ForgejoFailureEvidenceConfig::new(
                "http://127.0.0.1:8080/failures",
                "token",
                "key",
                "issuer",
                ["producer"]
            )
            .is_ok()
        );
        assert!(
            ForgejoFailureEvidenceConfig::new(
                "https://evidence.example/failures?secret=value",
                "token",
                "key",
                "issuer",
                ["producer"]
            )
            .is_err()
        );
    }
}
