//! The one place LLM provider/model and **auth-mode** wiring lives.
//!
//! Everything model-, provider-, or credential-specific is confined here so
//! swapping the model, backend, or authentication later is a single-file change.
//! Three auth modes are supported:
//!
//! - **`ApiKey`** — the default. DeepSeek behind the SDK's **OpenAI-compatible**
//!   completions route: an unknown provider id (`deepseek`) plus
//!   `api = "openai-completions"` selects [`pi::providers::create_provider`]'s
//!   OpenAI path, which appends `chat/completions` to the configured base URL.
//!   The API key is read **at runtime** from a file (default
//!   `.cache/deepseek-api-key`, gitignored) or an env var. Behavior is unchanged
//!   for existing callers — [`ProviderConfig::new`] and
//!   [`ProviderConfig::deepseek_from_env`] still build this mode.
//!
//! - **`ChatGptOAuth`** — a ChatGPT (OpenAI Codex) OAuth subscription. Provider
//!   id `openai-codex` routes to the SDK's Codex Responses provider (base URL
//!   normalized by the SDK); the **OAuth access token is the Bearer**, resolved
//!   **fresh per decision** from the shared `~/.pi/agent/auth.json` both pi CLIs
//!   write (refreshing when near expiry). See [`oauth`] and
//!   [`ProviderConfig::chatgpt_oauth_from_env`].
//!
//! - **`AnthropicOAuth`** — an Anthropic OAuth subscription. Provider id
//!   `anthropic` routes to the SDK's `anthropic-messages` provider; the OAuth
//!   access token is resolved fresh per decision from the same shared auth file,
//!   and Claude Code-compatible identity headers are injected per request through
//!   `StreamOptions.headers`. See [`ProviderConfig::anthropic_oauth_from_env`].
//!
//! ## Selecting an auth mode
//!
//! [`ProviderConfig::from_auth`] is the selection entry point: it takes an
//! [`AuthChoice`] plus optional `codex_model` / `auth_file` overrides. Each
//! override resolves with precedence **CLI override > env var > built-in
//! default** ([`CODEX_MODEL_ENV`]/[`DEFAULT_CODEX_MODEL`] for the Codex model,
//! [`ANTHROPIC_MODEL_ENV`]/[`DEFAULT_ANTHROPIC_MODEL`] for the Anthropic model,
//! and [`AUTH_FILE_ENV`]/`~/.pi/agent/auth.json` for the auth file). The library
//! default choice is [`AuthChoice::DeepSeek`]; the worker/test surfaces select
//! [`AuthChoice::ChatGptOAuth`] (see the worker's `--auth` flag /
//! `HARNESS_AGENTS_AUTH`) unless explicitly overridden. `from_auth` runs an
//! eager credential preflight so a missing key or login fails at setup, before
//! any worker tick.
//!
//! No secret is ever hardcoded, logged, or committed; [`ProviderConfig`]'s
//! `Debug` redacts credentials and errors carry only the provider/path, never
//! token bytes.

mod anthropic_oauth;
mod oauth;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pi::model::ThinkingLevel;
use pi::provider::{InputType, Model, ModelCost, Provider};
use pi::sdk::ModelEntry;

pub use anthropic_oauth::{ANTHROPIC_MODEL_ENV, DEFAULT_ANTHROPIC_MODEL};
pub use oauth::{AUTH_FILE_ENV, CODEX_MODEL_ENV, DEFAULT_CODEX_MODEL, default_auth_path};

/// Which credential the real agents authenticate with.
///
/// The library default is [`AuthChoice::DeepSeek`] (so any production wiring is
/// explicit and stable); the **test/dev surfaces default to**
/// [`AuthChoice::ChatGptOAuth`] per the cost policy (a flat subscription instead
/// of pay-per-token). Resolve a [`ProviderConfig`] for a choice with
/// [`ProviderConfig::from_auth`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthChoice {
    /// DeepSeek API key (pay-per-token). The library default.
    DeepSeek,
    /// ChatGPT (OpenAI Codex) OAuth subscription. The test/dev default.
    ChatGptOAuth,
    /// Anthropic OAuth subscription targeting Claude through `anthropic-messages`.
    AnthropicOAuth,
}

/// Env var that overrides the DeepSeek API-key file path.
pub const API_KEY_PATH_ENV: &str = "HARNESS_DEEPSEEK_API_KEY_PATH";
/// Env var that supplies the DeepSeek API key directly (takes precedence over
/// the file path).
pub const API_KEY_ENV: &str = "HARNESS_DEEPSEEK_API_KEY";

/// Default file the key is read from when no env override is set.
const DEFAULT_KEY_PATH: &str = ".cache/deepseek-api-key";
/// DeepSeek's OpenAI-compatible base; the SDK appends `chat/completions`.
const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";
/// DeepSeek v4 Flash model id (per the Phase B contract).
const DEFAULT_MODEL_ID: &str = "deepseek-chat";
/// Unknown provider id that routes through the OpenAI-completions API path.
const PROVIDER_ID: &str = "deepseek";
/// The SDK API string selecting the OpenAI chat-completions route.
const OPENAI_COMPLETIONS_API: &str = "openai-completions";
/// Provider id that routes through the SDK's Codex Responses provider.
const CODEX_PROVIDER_ID: &str = "openai-codex";
/// The SDK API string for the Codex Responses route (the codex route is selected
/// by provider id regardless of this value; set for clarity).
const CODEX_RESPONSES_API: &str = "openai-codex-responses";
/// Provider id that routes through the SDK's Anthropic provider.
const ANTHROPIC_PROVIDER_ID: &str = "anthropic";
/// The SDK API string selecting the Anthropic Messages route.
const ANTHROPIC_MESSAGES_API: &str = "anthropic-messages";
/// Anthropic API base URL; the SDK normalizes it to `/v1/messages`.
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// How an agent decision authenticates to its LLM provider.
#[derive(Clone)]
enum AuthMode {
    /// A static API key carried as the per-request bearer (DeepSeek default).
    ApiKey { api_key: String },
    /// ChatGPT (OpenAI Codex) OAuth: the bearer is resolved fresh per decision
    /// from the shared auth file (load → refresh → access token).
    ChatGptOAuth { settings: oauth::OAuthSettings },
    /// Anthropic OAuth: the bearer and Claude Code-compatible request identity
    /// headers are resolved fresh per decision from the shared auth file.
    AnthropicOAuth {
        settings: anthropic_oauth::AnthropicOAuthSettings,
    },
}

/// Resolved provider/model/auth wiring.
///
/// Build with [`ProviderConfig::deepseek_from_env`] for the production default,
/// [`ProviderConfig::chatgpt_oauth_from_env`] for the ChatGPT OAuth subscription,
/// [`ProviderConfig::anthropic_oauth_from_env`] for the Anthropic OAuth
/// subscription, or [`ProviderConfig::new`] to point at another OpenAI-compatible endpoint.
#[derive(Clone)]
pub struct ProviderConfig {
    provider_id: String,
    model_id: String,
    base_url: String,
    auth: AuthMode,
}

impl ProviderConfig {
    /// Builds an API-key config for an arbitrary OpenAI-compatible endpoint.
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            base_url: base_url.into(),
            auth: AuthMode::ApiKey {
                api_key: api_key.into(),
            },
        }
    }

    /// Builds the default DeepSeek config, reading the key at runtime.
    ///
    /// Key resolution order: [`API_KEY_ENV`] (direct), else the file at
    /// [`API_KEY_PATH_ENV`], else [`DEFAULT_KEY_PATH`]. Uses DeepSeek v4 Flash.
    pub fn deepseek_from_env() -> Result<Self, ProviderError> {
        let api_key = load_api_key()?;
        Ok(Self::new(
            PROVIDER_ID,
            DEFAULT_MODEL_ID,
            DEFAULT_BASE_URL,
            api_key,
        ))
    }

    /// Builds a ChatGPT (OpenAI Codex) OAuth config with explicit overrides.
    ///
    /// `codex_model` and `auth_file` each take precedence over their env var
    /// ([`CODEX_MODEL_ENV`] / [`AUTH_FILE_ENV`]) and then the built-in default
    /// (resolution order **CLI override > env > default**). The base URL is left
    /// empty so the SDK normalizes it to the canonical Codex endpoint. The bearer
    /// is **not** read here — it is resolved fresh per decision from the shared
    /// auth file.
    pub fn chatgpt_oauth(codex_model: Option<String>, auth_file: Option<PathBuf>) -> Self {
        Self {
            provider_id: CODEX_PROVIDER_ID.to_string(),
            model_id: oauth::resolve_codex_model(codex_model),
            base_url: String::new(),
            auth: AuthMode::ChatGptOAuth {
                settings: oauth::OAuthSettings::new(auth_file),
            },
        }
    }

    /// Builds a ChatGPT OAuth config from env/defaults (no explicit overrides).
    pub fn chatgpt_oauth_from_env() -> Self {
        Self::chatgpt_oauth(None, None)
    }

    /// Builds an Anthropic OAuth config with explicit auth-file override.
    ///
    /// The model id is selected through [`ANTHROPIC_MODEL_ENV`], falling back to
    /// [`DEFAULT_ANTHROPIC_MODEL`]. The auth-file override takes precedence over
    /// [`AUTH_FILE_ENV`] and then the SDK default (`~/.pi/agent/auth.json`).
    pub fn anthropic_oauth(auth_file: Option<PathBuf>) -> Self {
        Self {
            provider_id: ANTHROPIC_PROVIDER_ID.to_string(),
            model_id: anthropic_oauth::anthropic_model_from_env(),
            base_url: ANTHROPIC_BASE_URL.to_string(),
            auth: AuthMode::AnthropicOAuth {
                settings: anthropic_oauth::AnthropicOAuthSettings::new(auth_file),
            },
        }
    }

    /// Builds an Anthropic OAuth config from env/defaults (no explicit override).
    pub fn anthropic_oauth_from_env() -> Self {
        Self::anthropic_oauth(None)
    }

    /// Builds the provider config for an [`AuthChoice`], applying optional
    /// `codex_model` / `auth_file` overrides (each CLI > env > default), and
    /// performs an **eager credential preflight** so a missing key or login fails
    /// here — before any worker tick — mirroring the DeepSeek key-unavailable
    /// behavior. OAuth preflight errors point the operator at the matching
    /// `pi /login ...` command when no login is found.
    pub fn from_auth(
        choice: AuthChoice,
        codex_model: Option<String>,
        auth_file: Option<PathBuf>,
    ) -> Result<Self, ProviderError> {
        let config = match choice {
            AuthChoice::DeepSeek => Self::deepseek_from_env()?,
            AuthChoice::ChatGptOAuth => Self::chatgpt_oauth(codex_model, auth_file),
            AuthChoice::AnthropicOAuth => Self::anthropic_oauth(auth_file),
        };
        config.preflight()?;
        Ok(config)
    }

    /// Eager credential preflight: a no-op for [`AuthMode::ApiKey`] (the key was
    /// already read when the config was built) and an auth-file presence check
    /// for OAuth modes.
    fn preflight(&self) -> Result<(), ProviderError> {
        match &self.auth {
            AuthMode::ApiKey { .. } => Ok(()),
            AuthMode::ChatGptOAuth { settings } => settings.preflight(),
            AuthMode::AnthropicOAuth { settings } => settings.preflight(),
        }
    }

    /// The (non-secret) model id this config targets.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Resolves the per-request bearer.
    ///
    /// For [`AuthMode::ApiKey`] this is the stored key. For OAuth modes it reads
    /// (and refreshes when near expiry) the shared auth file, so it must be
    /// called **each time** a decision runs. Callers must not log the result.
    pub(crate) async fn resolve_bearer(&self) -> Result<String, ProviderError> {
        match &self.auth {
            AuthMode::ApiKey { api_key } => Ok(api_key.clone()),
            AuthMode::ChatGptOAuth { settings } => settings.resolve_bearer().await,
            AuthMode::AnthropicOAuth { settings } => settings.resolve_bearer().await,
        }
    }

    /// Extra per-request headers for this mode.
    ///
    /// Anthropic OAuth injects Claude Code-compatible request identity headers;
    /// all other modes use the SDK defaults.
    pub(crate) fn request_headers(&self) -> HashMap<String, String> {
        match &self.auth {
            AuthMode::AnthropicOAuth { .. } => anthropic_oauth::request_headers(),
            AuthMode::ApiKey { .. } | AuthMode::ChatGptOAuth { .. } => HashMap::new(),
        }
    }

    /// The mandatory first `system` block for this mode, if any.
    ///
    /// `Some` only for Anthropic OAuth, whose Claude subscription path rejects
    /// any request whose first system block is not exactly the Claude Code
    /// identity (HTTP 429). Because the SDK sends `system` as a single string,
    /// the decision adapter sets this as the system prompt and folds the role
    /// prompt into the user turn. All other modes return `None` and keep the
    /// role prompt as the system prompt.
    pub(crate) fn required_system_identity(&self) -> Option<&'static str> {
        match &self.auth {
            AuthMode::AnthropicOAuth { .. } => Some(anthropic_oauth::CLAUDE_CODE_SYSTEM_IDENTITY),
            AuthMode::ApiKey { .. } | AuthMode::ChatGptOAuth { .. } => None,
        }
    }

    /// The request temperature for this mode. API-key (DeepSeek) pins `0.0` for
    /// deterministic decisions; Codex reasoning models and Anthropic OAuth leave
    /// temperature unset.
    pub(crate) fn temperature(&self) -> Option<f32> {
        match &self.auth {
            AuthMode::ApiKey { .. } => Some(0.0),
            AuthMode::ChatGptOAuth { .. } | AuthMode::AnthropicOAuth { .. } => None,
        }
    }

    /// The reasoning effort for this mode. API-key and Anthropic OAuth requests
    /// leave it unset; the codex reasoning models read effort from this field,
    /// so Codex OAuth requests the **lowest supported** effort to keep one-shot
    /// JSON decisions fast and cheap. A3 live validation: `gpt-5.3-codex`
    /// rejects `minimal` (supported values are `none`/`low`/`medium`/`high`/
    /// `xhigh`), so Codex OAuth uses [`ThinkingLevel::Low`], not `Minimal`.
    pub(crate) fn thinking_level(&self) -> Option<ThinkingLevel> {
        match &self.auth {
            AuthMode::ApiKey { .. } | AuthMode::AnthropicOAuth { .. } => None,
            AuthMode::ChatGptOAuth { .. } => Some(ThinkingLevel::Low),
        }
    }

    /// Builds an SDK [`Provider`] for this config.
    ///
    /// The returned provider authenticates per request from the bearer carried in
    /// the agent's `stream_options`; the credential is never baked into the
    /// provider object itself.
    pub fn build_provider(&self) -> Result<Arc<dyn Provider>, ProviderError> {
        let entry = self.model_entry();
        pi::providers::create_provider(&entry, None)
            .map_err(|error| ProviderError::Build(error.to_string()))
    }

    /// Builds the [`ModelEntry`] the SDK factory consumes.
    fn model_entry(&self) -> ModelEntry {
        let (api, reasoning, input, cost, context_window, max_tokens) = match &self.auth {
            AuthMode::ChatGptOAuth { .. } => {
                // Codex models are reasoning models; the codex route sends no
                // explicit `max_output_tokens` (the model decides) and reads the
                // reasoning effort from `stream_options.thinking_level`. A
                // generous context window matches the gpt-5.x context.
                (
                    CODEX_RESPONSES_API,
                    true,
                    vec![InputType::Text],
                    zero_cost(),
                    400_000,
                    0,
                )
            }
            AuthMode::AnthropicOAuth { .. } => {
                // Anthropic Opus 4.x is a reasoning-capable, multimodal model,
                // but the initial OAuth path deliberately sends no explicit
                // thinking level until live validation proves the SDK's legacy
                // thinking-body shape is compatible with this model.
                (
                    ANTHROPIC_MESSAGES_API,
                    true,
                    vec![InputType::Text, InputType::Image],
                    ModelCost {
                        input: 15.0,
                        output: 75.0,
                        cache_read: 1.5,
                        cache_write: 18.75,
                    },
                    1_000_000,
                    128_000,
                )
            }
            AuthMode::ApiKey { .. } => (
                OPENAI_COMPLETIONS_API,
                false,
                vec![InputType::Text],
                zero_cost(),
                64_000,
                8_192,
            ),
        };
        let model = Model {
            id: self.model_id.clone(),
            name: self.model_id.clone(),
            api: api.to_string(),
            provider: self.provider_id.clone(),
            base_url: self.base_url.clone(),
            reasoning,
            input,
            cost,
            context_window,
            max_tokens,
            headers: HashMap::new(),
        };
        ModelEntry {
            model,
            // The bearer flows through the agent's `stream_options.api_key`, not
            // the entry, so it is not duplicated here.
            api_key: None,
            headers: HashMap::new(),
            auth_header: true,
            compat: None,
            oauth_config: None,
        }
    }
}

fn zero_cost() -> ModelCost {
    ModelCost {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = match &self.auth {
            AuthMode::ApiKey { .. } => "api_key",
            AuthMode::ChatGptOAuth { .. } => "chatgpt_oauth",
            AuthMode::AnthropicOAuth { .. } => "anthropic_oauth",
        };
        formatter
            .debug_struct("ProviderConfig")
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("auth_mode", &mode)
            .field("credential", &"<redacted>")
            .finish()
    }
}

/// Failure building provider wiring or resolving a credential.
#[derive(Debug)]
pub enum ProviderError {
    /// The API key could not be read from env or the configured file.
    KeyUnavailable(String),
    /// The ChatGPT OAuth bearer could not be resolved or refreshed.
    OAuthUnavailable(String),
    /// The Anthropic OAuth bearer could not be resolved or refreshed.
    AnthropicOAuthUnavailable(String),
    /// The SDK provider factory rejected the model entry.
    Build(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::KeyUnavailable(message) => {
                write!(formatter, "DeepSeek API key unavailable: {message}")
            }
            ProviderError::OAuthUnavailable(message) => {
                write!(formatter, "ChatGPT OAuth unavailable: {message}")
            }
            ProviderError::AnthropicOAuthUnavailable(message) => {
                write!(formatter, "Anthropic OAuth unavailable: {message}")
            }
            ProviderError::Build(message) => {
                write!(formatter, "building LLM provider failed: {message}")
            }
        }
    }
}

impl std::error::Error for ProviderError {}

/// Reads the API key from env or the configured file, redacting the value on
/// error (only the path is ever surfaced).
fn load_api_key() -> Result<String, ProviderError> {
    if let Ok(key) = std::env::var(API_KEY_ENV) {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let path = std::env::var(API_KEY_PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_KEY_PATH));
    read_key_file(&path)
}

fn read_key_file(path: &Path) -> Result<String, ProviderError> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        ProviderError::KeyUnavailable(format!("reading {}: {error}", path.display()))
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::KeyUnavailable(format!(
            "{} is empty",
            path.display()
        )));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_mode_targets_openai_completions() {
        let config =
            ProviderConfig::new("deepseek", "deepseek-chat", DEFAULT_BASE_URL, "sk-secret");
        let entry = config.model_entry();
        assert_eq!(entry.model.provider, "deepseek");
        assert_eq!(entry.model.api, OPENAI_COMPLETIONS_API);
        assert!(!entry.model.reasoning);
        assert_eq!(config.temperature(), Some(0.0));
        assert!(config.thinking_level().is_none());
        // The SDK factory accepts the entry without network.
        assert!(config.build_provider().is_ok());
    }

    #[test]
    fn chatgpt_oauth_mode_targets_codex_route() {
        let config = ProviderConfig::chatgpt_oauth_from_env();
        let entry = config.model_entry();
        assert_eq!(entry.model.provider, CODEX_PROVIDER_ID);
        assert!(entry.model.reasoning);
        assert_eq!(config.model_id(), DEFAULT_CODEX_MODEL);
        assert!(config.temperature().is_none());
        assert_eq!(config.thinking_level(), Some(ThinkingLevel::Low));
        // The SDK factory accepts the codex entry without network.
        assert!(config.build_provider().is_ok());
    }

    #[test]
    fn chatgpt_oauth_applies_model_override() {
        let config = ProviderConfig::chatgpt_oauth(Some("gpt-5.9-codex".to_string()), None);
        assert_eq!(config.model_id(), "gpt-5.9-codex");
    }

    #[test]
    fn anthropic_oauth_requires_claude_code_system_identity() {
        let config = ProviderConfig::anthropic_oauth_from_env();
        assert_eq!(
            config.required_system_identity(),
            Some("You are Claude Code, Anthropic's official CLI for Claude.")
        );
        // Other modes keep the role prompt as the system prompt.
        let deepseek =
            ProviderConfig::new("deepseek", "deepseek-chat", DEFAULT_BASE_URL, "sk-secret");
        assert_eq!(deepseek.required_system_identity(), None);
        assert_eq!(
            ProviderConfig::chatgpt_oauth_from_env().required_system_identity(),
            None
        );
    }

    #[test]
    fn anthropic_oauth_mode_targets_anthropic_messages_route() {
        let config = ProviderConfig::anthropic_oauth_from_env();
        let entry = config.model_entry();
        assert_eq!(entry.model.provider, ANTHROPIC_PROVIDER_ID);
        assert_eq!(entry.model.api, ANTHROPIC_MESSAGES_API);
        assert_eq!(entry.model.base_url, ANTHROPIC_BASE_URL);
        assert!(entry.model.reasoning);
        assert_eq!(entry.model.input, vec![InputType::Text, InputType::Image]);
        assert_eq!(entry.model.context_window, 1_000_000);
        assert_eq!(entry.model.max_tokens, 128_000);
        assert_eq!(config.model_id(), DEFAULT_ANTHROPIC_MODEL);
        assert!(config.temperature().is_none());
        assert!(config.thinking_level().is_none());
        // The SDK factory accepts the Anthropic entry without network.
        assert!(config.build_provider().is_ok());
    }

    #[test]
    fn from_auth_oauth_preflights_missing_login() {
        // A non-existent auth file means no login: `from_auth` must fail at setup
        // with a clear OAuth error pointing at the login command, not defer to a
        // later worker tick.
        let missing = std::env::temp_dir().join(format!(
            "harness-agents-absent-{}-{}.json",
            std::process::id(),
            "from-auth"
        ));
        let _ = std::fs::remove_file(&missing);
        let error = ProviderConfig::from_auth(AuthChoice::ChatGptOAuth, None, Some(missing))
            .expect_err("missing login must fail the preflight");
        assert!(matches!(error, ProviderError::OAuthUnavailable(_)));
        assert!(format!("{error}").contains("openai-codex"));
    }

    #[test]
    fn from_auth_anthropic_oauth_preflights_missing_login() {
        let missing = std::env::temp_dir().join(format!(
            "harness-agents-absent-{}-{}.json",
            std::process::id(),
            "from-auth-anthropic"
        ));
        let _ = std::fs::remove_file(&missing);
        let error = ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, Some(missing))
            .expect_err("missing Anthropic login must fail the preflight");
        assert!(matches!(error, ProviderError::AnthropicOAuthUnavailable(_)));
        assert!(format!("{error}").contains("pi /login anthropic"));
    }

    #[test]
    fn from_auth_anthropic_oauth_preflights_present_login() {
        let path = std::env::temp_dir().join(format!(
            "harness-agents-present-{}-{}.json",
            std::process::id(),
            "from-auth-anthropic"
        ));
        let contents = serde_json::json!({
            "anthropic": {
                "type": "oauth",
                "access": "sk-ant-oat-access",
                "refresh": "refresh-token",
                "expires": 4_102_444_800_000_i64,
            }
        })
        .to_string();
        std::fs::write(&path, contents).expect("write auth fixture");
        let result =
            ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, Some(path.clone()));
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "present Anthropic login should preflight");
    }

    #[test]
    fn debug_redacts_api_key() {
        let config =
            ProviderConfig::new("deepseek", "deepseek-chat", DEFAULT_BASE_URL, "sk-secret");
        let rendered = format!("{config:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("sk-secret"));
        assert!(rendered.contains("api_key"));
    }

    #[test]
    fn debug_redacts_anthropic_oauth() {
        let config = ProviderConfig::anthropic_oauth(None);
        let rendered = format!("{config:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("anthropic_oauth"));
        assert!(!rendered.contains("sk-ant"));
    }
}
