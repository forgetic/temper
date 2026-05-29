//! Forgejo backend configuration and environment parsing.

use thiserror::Error;

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
    /// ([`harness_forge::ForgeError::InvalidRequest`]). Favors safety.
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

/// Failure parsing configuration from the environment.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    MissingEnv(&'static str),
    #[error("invalid {name}: {value:?}")]
    Invalid { name: &'static str, value: String },
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

    /// Builds configuration from the process environment.
    ///
    /// Reads `FORGEJO_URL` and `FORGEJO_ACCESS_TOKEN` (both required) and the
    /// optional `FORGEJO_DEFAULT_REPO` in `owner/repo` form. These match the
    /// environment names used by the reference TypeScript integration.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Builds configuration from an arbitrary key lookup; used for tests.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let base_url = required(&lookup, "FORGEJO_URL")?;
        let token = required(&lookup, "FORGEJO_ACCESS_TOKEN")?;
        let mut config = ForgejoConfig::new(base_url, token);

        if let Some(repo) = non_empty(lookup("FORGEJO_DEFAULT_REPO")) {
            let (owner, name) = repo.split_once('/').ok_or_else(|| ConfigError::Invalid {
                name: "FORGEJO_DEFAULT_REPO",
                value: repo.clone(),
            })?;
            if owner.is_empty() || name.is_empty() || name.contains('/') {
                return Err(ConfigError::Invalid {
                    name: "FORGEJO_DEFAULT_REPO",
                    value: repo,
                });
            }
            config = config.with_default_repo(owner, name);
        }

        Ok(config)
    }
}

fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    key: &'static str,
) -> Result<String, ConfigError> {
    non_empty(lookup(key)).ok_or(ConfigError::MissingEnv(key))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|candidate| !candidate.trim().is_empty())
}

fn strip_trailing_slashes(value: String) -> String {
    value.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

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
    fn from_env_parses_required_and_optional_values() {
        let config = ForgejoConfig::from_lookup(lookup(&[
            ("FORGEJO_URL", "https://git.example.com/"),
            ("FORGEJO_ACCESS_TOKEN", "secret"),
            ("FORGEJO_DEFAULT_REPO", "acme/widgets"),
        ]))
        .unwrap();

        assert_eq!(config.base_url, "https://git.example.com");
        assert_eq!(config.token, "secret");
        assert_eq!(config.default_owner.as_deref(), Some("acme"));
        assert_eq!(config.default_name.as_deref(), Some("widgets"));
    }

    #[test]
    fn from_env_allows_missing_default_repo() {
        let config = ForgejoConfig::from_lookup(lookup(&[
            ("FORGEJO_URL", "https://git.example.com"),
            ("FORGEJO_ACCESS_TOKEN", "secret"),
        ]))
        .unwrap();
        assert_eq!(config.default_owner, None);
        assert_eq!(config.default_name, None);
    }

    #[test]
    fn from_env_requires_url_and_token() {
        assert_eq!(
            ForgejoConfig::from_lookup(lookup(&[("FORGEJO_ACCESS_TOKEN", "secret")])),
            Err(ConfigError::MissingEnv("FORGEJO_URL"))
        );
        assert_eq!(
            ForgejoConfig::from_lookup(lookup(&[("FORGEJO_URL", "https://git.example.com")])),
            Err(ConfigError::MissingEnv("FORGEJO_ACCESS_TOKEN"))
        );
        // Blank values count as missing.
        assert_eq!(
            ForgejoConfig::from_lookup(lookup(&[
                ("FORGEJO_URL", "  "),
                ("FORGEJO_ACCESS_TOKEN", "secret"),
            ])),
            Err(ConfigError::MissingEnv("FORGEJO_URL"))
        );
    }

    #[test]
    fn from_env_rejects_malformed_default_repo() {
        let result = ForgejoConfig::from_lookup(lookup(&[
            ("FORGEJO_URL", "https://git.example.com"),
            ("FORGEJO_ACCESS_TOKEN", "secret"),
            ("FORGEJO_DEFAULT_REPO", "no-slash"),
        ]));
        assert_eq!(
            result,
            Err(ConfigError::Invalid {
                name: "FORGEJO_DEFAULT_REPO",
                value: "no-slash".to_string(),
            })
        );
    }
}
