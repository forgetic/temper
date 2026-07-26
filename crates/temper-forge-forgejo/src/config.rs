//! Forgejo backend configuration.
//!
//! This crate is a library, not a process boundary: it never reads the process
//! environment. Callers build a [`ForgejoConfig`] from explicit values.

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

/// Configuration for the Forgejo backend.
#[derive(Clone, Debug, Eq, PartialEq)]
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
}

fn strip_trailing_slashes(value: String) -> String {
    value.trim_end_matches('/').to_string()
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
}
