//! Forgejo backend configuration.
//!
//! This crate is a library, not a process boundary: it never reads the process
//! environment. Callers (the wiring/service layer and binaries) build a
//! [`ForgejoConfig`] from explicit values — see `temper-engine-service`'s
//! `forgejo_config` adapter, which translates a resolved config into one of
//! these.

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

/// Optional password-authenticated web-UI credentials.
///
/// Forgejo 7.0.x does not serve Actions runs/tasks over REST, so CI status is
/// read through the password-authenticated web UI (ADR 0019). Only the CI read
/// path needs these; every REST operation needs only the token. The credentials
/// never appear in `Debug` output or in error messages.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct WebUiCredentials {
    /// Web-UI login user name.
    pub username: String,
    /// Web-UI login password.
    pub password: String,
}

impl std::fmt::Debug for WebUiCredentials {
    /// Redacts the password (and user name) so credentials never leak via logs.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebUiCredentials")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
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
    /// Optional web-UI credentials used only for the CI read fallback (ADR 0019).
    pub web_ui: Option<WebUiCredentials>,
    /// When set, web-UI CI fallback reads are logged to stderr. The wiring layer
    /// sets this from its diagnostics flag; the backend never reads the
    /// environment to decide it.
    pub ci_diagnostics: bool,
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
            web_ui: None,
            ci_diagnostics: false,
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

    /// Sets the web-UI credentials used for the CI read fallback (ADR 0019).
    ///
    /// Only the CI read path uses these; every REST operation needs only the
    /// token. Blank user name or password is treated as "no credentials".
    pub fn with_web_ui_credentials(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        let username = username.into();
        let password = password.into();
        self.web_ui = if username.trim().is_empty() || password.trim().is_empty() {
            None
        } else {
            Some(WebUiCredentials { username, password })
        };
        self
    }

    /// Enables (or disables) stderr logging of web-UI CI fallback reads.
    ///
    /// The wiring layer flips this from its own diagnostics flag; the backend
    /// never inspects the environment to decide it.
    pub fn with_ci_diagnostics(mut self, enabled: bool) -> Self {
        self.ci_diagnostics = enabled;
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
        assert!(!config.ci_diagnostics);
    }

    #[test]
    fn builders_set_explicit_values() {
        let config = ForgejoConfig::new("https://git.example.com/", "secret")
            .with_default_repo("acme", "widgets")
            .with_ci_diagnostics(true);

        assert_eq!(config.base_url, "https://git.example.com");
        assert_eq!(config.token, "secret");
        assert_eq!(config.default_owner.as_deref(), Some("acme"));
        assert_eq!(config.default_name.as_deref(), Some("widgets"));
        assert!(config.ci_diagnostics);
    }

    #[test]
    fn ci_diagnostics_defaults_off_and_toggles() {
        let config = ForgejoConfig::new("https://git.example.com", "tok");
        assert!(!config.ci_diagnostics);
        assert!(config.with_ci_diagnostics(true).ci_diagnostics);
    }

    #[test]
    fn web_ui_credentials_are_optional_and_blank_safe() {
        let none = ForgejoConfig::new("https://git.example.com", "tok");
        assert_eq!(none.web_ui, None);

        let set = none.clone().with_web_ui_credentials("ci-reader", "s3cret");
        assert_eq!(
            set.web_ui,
            Some(WebUiCredentials {
                username: "ci-reader".to_string(),
                password: "s3cret".to_string(),
            })
        );

        // A blank user name or password yields no credentials.
        assert_eq!(
            none.clone().with_web_ui_credentials("", "s3cret").web_ui,
            None
        );
        assert_eq!(none.with_web_ui_credentials("ci-reader", "  ").web_ui, None);
    }

    #[test]
    fn web_ui_credentials_redact_in_debug() {
        let creds = WebUiCredentials {
            username: "ci-reader".to_string(),
            password: "super-secret".to_string(),
        };
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("ci-reader"));
        assert!(rendered.contains("redacted"));
    }
}
