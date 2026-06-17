//! The one place LLM provider/model and **auth-mode** wiring lives.
//!
//! Everything model-, provider-, or credential-specific is confined here so
//! swapping the model, backend, or authentication later is a single-file change.
//! Three auth modes are supported:
//!
//! - **`ApiKey`** — the default. DeepSeek behind the SDK's **OpenAI-compatible**
//!   completions route: an unknown provider id (`deepseek`) plus
//!   `api = "openai-completions"` selects [`tongs::providers::create_provider`]'s
//!   OpenAI path, which appends `chat/completions` to the configured base URL.
//!   The API key is read **at runtime** from a file (default
//!   `.cache/deepseek-api-key`, gitignored) or supplied by the host through a
//!   [`ProviderEnv`]. Behavior is unchanged for existing callers —
//!   [`ProviderConfig::new`] and [`ProviderConfig::deepseek_with_env`] still build
//!   this mode.
//!
//! - **`ChatGptOAuth`** — a ChatGPT (OpenAI Codex) OAuth subscription. Provider
//!   id `openai-codex` routes to the SDK's Codex Responses provider (base URL
//!   normalized by the SDK); the **OAuth access token is the Bearer**, resolved
//!   **fresh per decision** from the shared `~/.pi/agent/auth.json` both pi CLIs
//!   write (refreshing when near expiry). See [`oauth`] and
//!   [`ProviderConfig::chatgpt_oauth_with_env`].
//!
//! - **`AnthropicOAuth`** — an Anthropic OAuth subscription. Provider id
//!   `anthropic` routes to the SDK's `anthropic-messages` provider; the OAuth
//!   access token is resolved fresh per decision from the same shared auth file,
//!   and Claude Code-compatible identity headers are injected per request through
//!   `StreamOptions.headers`. See [`ProviderConfig::anthropic_oauth_with_env`].
//!
//! ## Selecting an auth mode
//!
//! [`ProviderConfig::from_auth`] is the selection entry point: it takes an
//! [`AuthChoice`], optional `codex_model` / `auth_file` overrides, and the
//! host-read [`ProviderEnv`]. **This module reads no environment** — every
//! env-derived input arrives through `ProviderEnv`, populated by the agent's
//! `entry`. Each override resolves with precedence **CLI override > the matching
//! `ProviderEnv` value > built-in default** ([`CODEX_MODEL_ENV`]/
//! [`DEFAULT_CODEX_MODEL`] for the Codex model, [`ANTHROPIC_MODEL_ENV`]/
//! [`DEFAULT_ANTHROPIC_MODEL`] for the Anthropic model, and [`AUTH_FILE_ENV`]/
//! `~/.pi/agent/auth.json` for the auth file). The library default choice is
//! [`AuthChoice::DeepSeek`]; anvil's CLI/preflight and responder binaries default
//! to [`AuthChoice::ChatGptOAuth`] unless their `--auth` flag explicitly chooses
//! another mode. `from_auth` runs an eager credential preflight so a missing key
//! or login fails at setup, before any responder work begins.
//!
//! No secret is ever hardcoded, logged, or committed; [`ProviderConfig`]'s
//! `Debug` redacts credentials and errors carry only the provider/path, never
//! token bytes.

mod anthropic_model;
mod anthropic_oauth;
mod auth;
mod env;
mod error;
mod model_entry;
mod oauth;
mod request_options;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use secrecy::SecretString;
use tongs::provider::Provider;

use auth::AuthMode;

pub use anthropic_model::ANTHROPIC_SUBAGENT_MODEL_ENV;
pub use anthropic_oauth::{ANTHROPIC_MODEL_ENV, ANTHROPIC_TOKEN_URL_ENV, DEFAULT_ANTHROPIC_MODEL};
pub use auth::AuthChoice;
pub use env::ProviderEnv;
pub use error::{API_KEY_ENV, API_KEY_PATH_ENV, ProviderError};
pub use oauth::{
    AUTH_FILE_ENV, CODEX_MODEL_ENV, CODEX_TOKEN_URL_ENV, DEFAULT_CODEX_MODEL, default_auth_path,
};

/// Env var that redirects outbound provider traffic to an alternate base URL.
///
/// The agent's `entry` reads it and passes the value to
/// [`ProviderConfig::apply_base_url_override`], which rewrites the provider base
/// URL — used to point the agent at a local fake LLM. Honored unconditionally for
/// every auth mode, so only set it in environments you control. The name lives
/// here so the env reader and the worker's env-injection map agree; nothing in
/// this crate reads it.
pub const PROVIDER_BASE_URL_ENV: &str = "ANVIL_TEST_PROVIDER_BASE_URL";

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

/// Resolved provider/model/auth wiring.
///
/// Build with [`ProviderConfig::from_auth`] (passing the host-read
/// [`ProviderEnv`]) for the production default, or [`ProviderConfig::new`] to
/// point at another OpenAI-compatible endpoint. The `*_with_env` constructors
/// build a single mode directly.
#[derive(Clone)]
pub struct ProviderConfig {
    provider_id: String,
    model_id: String,
    base_url: String,
    auth: AuthMode,
    /// Explicit sub-agent (investigate) model override. When set, it takes
    /// precedence over the env/default sub-agent model — the config-file driven
    /// path uses this so the in-process agent honors `models.investigate`
    /// without relying on ambient environment.
    subagent_model_override: Option<String>,
}

impl ProviderConfig {
    /// Builds an API-key config for an arbitrary OpenAI-compatible endpoint.
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self::with_secret_key(
            provider_id,
            model_id,
            base_url,
            SecretString::from(api_key.into()),
        )
    }

    /// Builds an API-key config from an already-wrapped [`SecretString`] key —
    /// the secret-preserving counterpart to [`new`](Self::new) used by the
    /// credential loaders so the key never round-trips through a bare `String`.
    pub fn with_secret_key(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: SecretString,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            base_url: base_url.into(),
            auth: AuthMode::ApiKey { api_key },
            subagent_model_override: None,
        }
    }

    /// Builds a DeepSeek (OpenAI-compatible) config from an explicit API key,
    /// with optional model and base-URL overrides — the config-file driven
    /// counterpart to [`deepseek_with_env`](Self::deepseek_with_env).
    pub fn deepseek_with_key(
        api_key: impl Into<String>,
        model: Option<String>,
        base_url: Option<String>,
    ) -> Self {
        Self::new(
            PROVIDER_ID,
            model.unwrap_or_else(|| DEFAULT_MODEL_ID.to_string()),
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key,
        )
    }

    /// Builds the default DeepSeek config, resolving the key from the supplied
    /// [`ProviderEnv`].
    ///
    /// Key resolution order: the host-read [`ProviderEnv::deepseek_api_key`]
    /// (direct), else the file at [`ProviderEnv::deepseek_api_key_path`], else the
    /// default `.cache/deepseek-api-key` file. Reads no environment; the host
    /// supplies the inputs. Uses DeepSeek v4 Flash.
    pub fn deepseek_with_env(env: &ProviderEnv) -> Result<Self, ProviderError> {
        let api_key = error::load_api_key(
            env.deepseek_api_key.as_ref(),
            env.deepseek_api_key_path.as_deref(),
        )?;
        Ok(Self::with_secret_key(
            PROVIDER_ID,
            DEFAULT_MODEL_ID,
            DEFAULT_BASE_URL,
            api_key,
        ))
    }

    /// Builds a ChatGPT (OpenAI Codex) OAuth config from explicit overrides only.
    ///
    /// `codex_model` and `auth_file` take precedence over the compiled-in
    /// defaults. The env-derived overrides (model/auth-file/token-url) default to
    /// absent; [`chatgpt_oauth_with_env`](Self::chatgpt_oauth_with_env) is the
    /// env-aware counterpart `from_auth` uses. The base URL is left empty so the
    /// SDK normalizes it to the canonical Codex endpoint; the bearer is resolved
    /// fresh per decision from the shared auth file.
    pub fn chatgpt_oauth(codex_model: Option<String>, auth_file: Option<PathBuf>) -> Self {
        Self::chatgpt_oauth_with_env(codex_model, auth_file, &ProviderEnv::empty())
    }

    /// Builds a ChatGPT OAuth config from the CLI overrides plus the host-read
    /// [`ProviderEnv`].
    ///
    /// Precedence per knob: CLI override > the matching `env` value > compiled-in
    /// default ([`CODEX_MODEL_ENV`] for the model, [`AUTH_FILE_ENV`] for the auth
    /// file, [`CODEX_TOKEN_URL_ENV`] for the refresh endpoint). Reads no
    /// environment itself.
    pub fn chatgpt_oauth_with_env(
        codex_model: Option<String>,
        auth_file: Option<PathBuf>,
        env: &ProviderEnv,
    ) -> Self {
        let auth_file = auth_file.or_else(|| env.auth_file.clone());
        Self {
            provider_id: CODEX_PROVIDER_ID.to_string(),
            model_id: oauth::resolve_codex_model(codex_model, env.codex_model.clone()),
            base_url: String::new(),
            auth: AuthMode::ChatGptOAuth {
                settings: oauth::OAuthSettings::new(auth_file, env.codex_token_url.clone()),
            },
            subagent_model_override: None,
        }
    }

    /// Builds a ChatGPT OAuth config from compiled-in defaults (no overrides).
    pub fn chatgpt_oauth_from_env() -> Self {
        Self::chatgpt_oauth(None, None)
    }

    /// Builds an Anthropic OAuth config from an explicit auth-file override only.
    ///
    /// The model id falls back to [`DEFAULT_ANTHROPIC_MODEL`]; the env-derived
    /// overrides default to absent.
    /// [`anthropic_oauth_with_env`](Self::anthropic_oauth_with_env) is the
    /// env-aware counterpart `from_auth` uses.
    pub fn anthropic_oauth(auth_file: Option<PathBuf>) -> Self {
        Self::anthropic_oauth_with_env(auth_file, &ProviderEnv::empty())
    }

    /// Builds an Anthropic OAuth config from the auth-file override plus the
    /// host-read [`ProviderEnv`].
    ///
    /// The model id is the host-read [`ANTHROPIC_MODEL_ENV`] value, falling back
    /// to [`DEFAULT_ANTHROPIC_MODEL`]; the sub-agent override
    /// ([`ANTHROPIC_SUBAGENT_MODEL_ENV`]) and the token-url
    /// ([`ANTHROPIC_TOKEN_URL_ENV`]) are likewise carried through. The auth-file
    /// override (with any [`AUTH_FILE_ENV`] value folded in) takes precedence over
    /// the SDK default. Reads no environment itself.
    pub fn anthropic_oauth_with_env(auth_file: Option<PathBuf>, env: &ProviderEnv) -> Self {
        let auth_file = auth_file.or_else(|| env.auth_file.clone());
        Self {
            provider_id: ANTHROPIC_PROVIDER_ID.to_string(),
            model_id: anthropic_oauth::resolve_anthropic_model(env.anthropic_model.clone()),
            base_url: ANTHROPIC_BASE_URL.to_string(),
            auth: AuthMode::AnthropicOAuth {
                settings: anthropic_oauth::AnthropicOAuthSettings::new(
                    auth_file,
                    env.anthropic_token_url.clone(),
                ),
            },
            subagent_model_override: env
                .anthropic_subagent_model
                .clone()
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty()),
        }
    }

    /// Builds an Anthropic OAuth config from compiled-in defaults (no override).
    pub fn anthropic_oauth_from_env() -> Self {
        Self::anthropic_oauth(None)
    }

    /// Overrides the provider base URL, redirecting outbound provider traffic to
    /// the given endpoint (e.g. a local fake LLM server in hermetic tests).
    ///
    /// This is honored unconditionally, including for OAuth auth modes — a caller
    /// that can supply a base URL can therefore redirect credentialed traffic, so
    /// only supply it (from [`ProviderEnv::base_url_override`], read in the agent's
    /// `entry`) in environments you control.
    pub fn with_base_url_override(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The configured provider base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Applies the host-read base-URL override ([`ProviderEnv::base_url_override`],
    /// the [`PROVIDER_BASE_URL_ENV`] value) when it is set to a non-empty value;
    /// otherwise returns the config unchanged. Reads no environment itself.
    pub fn apply_base_url_override(self, override_url: Option<&str>) -> Self {
        match override_url {
            Some(base_url) if !base_url.trim().is_empty() => self.with_base_url_override(base_url),
            _ => self,
        }
    }

    /// Builds the provider config for an [`AuthChoice`], applying the CLI
    /// `codex_model` / `auth_file` overrides plus the host-read [`ProviderEnv`]
    /// (each knob CLI > env > default), and performs an **eager credential
    /// preflight** so a missing key or login fails here — before any worker tick.
    /// OAuth preflight errors point the operator at the matching `pi /login ...`
    /// command when no login is found. Reads no environment itself.
    pub fn from_auth(
        choice: AuthChoice,
        codex_model: Option<String>,
        auth_file: Option<PathBuf>,
        env: &ProviderEnv,
    ) -> Result<Self, ProviderError> {
        let config = match choice {
            AuthChoice::DeepSeek => Self::deepseek_with_env(env)?,
            AuthChoice::ChatGptOAuth => Self::chatgpt_oauth_with_env(codex_model, auth_file, env),
            AuthChoice::AnthropicOAuth => Self::anthropic_oauth_with_env(auth_file, env),
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

    /// Returns a clone of this config retargeted at a different model id, keeping
    /// the same provider, endpoint, and credential. Used to run sub-agents on a
    /// cheaper/faster model than the main agent (the parent's auth still applies,
    /// since the bearer is shared across models on the same provider).
    #[must_use]
    pub fn with_model_id(&self, model_id: impl Into<String>) -> Self {
        let mut clone = self.clone();
        clone.model_id = model_id.into();
        clone
    }

    /// Sets an explicit sub-agent (investigate) model override (config-file
    /// driven). `None` clears it, restoring env/default resolution.
    #[must_use]
    pub fn with_subagent_model_id(mut self, model_id: Option<String>) -> Self {
        self.subagent_model_override = model_id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        self
    }

    /// The model id to run read-only `investigate` sub-agents on.
    ///
    /// For the Anthropic OAuth mode this is the (cheaper, overridable) sub-agent
    /// tier — mirroring Claude Code routing its investigation sub-agents to a
    /// smaller model. For other modes (Codex, DeepSeek) there is no separate
    /// cheap tier wired up, so sub-agents stay on the main model.
    pub fn subagent_model_id(&self) -> String {
        if let Some(override_id) = &self.subagent_model_override {
            return override_id.clone();
        }
        match &self.auth {
            // No override carried (none from config-file or the host-read
            // sub-agent env): use the compiled-in cheaper tier.
            AuthMode::AnthropicOAuth { .. } => {
                anthropic_oauth::resolve_anthropic_subagent_model(None)
            }
            _ => self.model_id.clone(),
        }
    }

    /// Builds an SDK [`Provider`] for this config.
    ///
    /// The returned provider authenticates per request from the bearer carried in
    /// the agent's `stream_options`; the credential is never baked into the
    /// provider object itself.
    pub fn build_provider(&self) -> Result<Arc<dyn Provider>, ProviderError> {
        let entry = self.model_entry();
        tongs::providers::create_provider(&entry, None)
            .map_err(|error| ProviderError::Build(error.to_string()))
    }

    /// Builds the SDK [`tongs::provider::ModelEntry`] the factory consumes.
    fn model_entry(&self) -> tongs::provider::ModelEntry {
        model_entry::build_model_entry(self)
    }

    /// The (non-secret) provider id this config routes through.
    pub(in crate::provider) fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// The resolved auth mode (read by the model-entry and bearer helpers).
    pub(in crate::provider) fn auth(&self) -> &AuthMode {
        &self.auth
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderConfig")
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("auth_mode", &self.auth.label())
            .field("credential", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
