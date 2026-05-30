//! The one place LLM provider/model wiring lives.
//!
//! Everything model- or provider-specific is confined here so swapping the model
//! or backend later is a single-file change. The default is **DeepSeek** behind
//! the SDK's **OpenAI-compatible** completions route: an unknown provider id
//! (`deepseek`) plus `api = "openai-completions"` selects
//! [`pi::providers::create_provider`]'s OpenAI path, which appends
//! `chat/completions` to the configured base URL.
//!
//! The API key is read **at runtime** from a file (default
//! `.cache/deepseek-api-key`, gitignored) or an env var. It is never hardcoded,
//! logged, or committed; [`ProviderConfig`]'s `Debug` redacts it and errors
//! carry only the path, never the key bytes.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pi::provider::{InputType, Model, ModelCost, Provider};
use pi::sdk::ModelEntry;

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

/// Resolved provider/model wiring, including the (redacted) API key.
///
/// Build with [`ProviderConfig::deepseek_from_env`] for the production default,
/// or [`ProviderConfig::new`] to point at another OpenAI-compatible endpoint.
#[derive(Clone)]
pub struct ProviderConfig {
    provider_id: String,
    model_id: String,
    base_url: String,
    api_key: String,
}

impl ProviderConfig {
    /// Builds a config for an arbitrary OpenAI-compatible endpoint.
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
            api_key: api_key.into(),
        }
    }

    /// Builds the default DeepSeek config, reading the key at runtime.
    ///
    /// Key resolution order: [`API_KEY_ENV`] (direct), else the file at
    /// [`API_KEY_PATH_ENV`], else [`DEFAULT_KEY_PATH`]. The model id and base URL
    /// can be overridden through the optional env vars documented on the
    /// constants but default to DeepSeek v4 Flash.
    pub fn deepseek_from_env() -> Result<Self, ProviderError> {
        let api_key = load_api_key()?;
        Ok(Self::new(
            PROVIDER_ID,
            DEFAULT_MODEL_ID,
            DEFAULT_BASE_URL,
            api_key,
        ))
    }

    /// The (non-secret) model id this config targets.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Borrows the resolved API key. Callers must not log it.
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Builds an SDK [`Provider`] for this config.
    ///
    /// The returned provider authenticates per request from the API key carried
    /// in the agent's `stream_options` (wired by the engineer adapter); the key
    /// is never baked into the provider object itself.
    pub fn build_provider(&self) -> Result<Arc<dyn Provider>, ProviderError> {
        let entry = self.model_entry();
        pi::providers::create_provider(&entry, None)
            .map_err(|error| ProviderError::Build(error.to_string()))
    }

    /// Builds the [`ModelEntry`] the SDK factory consumes.
    fn model_entry(&self) -> ModelEntry {
        let model = Model {
            id: self.model_id.clone(),
            name: self.model_id.clone(),
            api: OPENAI_COMPLETIONS_API.to_string(),
            provider: self.provider_id.clone(),
            base_url: self.base_url.clone(),
            reasoning: false,
            input: vec![InputType::Text],
            cost: ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 64_000,
            max_tokens: 8_192,
            headers: HashMap::new(),
        };
        ModelEntry {
            model,
            // The key flows through the agent's `stream_options.api_key`, not the
            // entry, so it is not duplicated here.
            api_key: None,
            headers: HashMap::new(),
            auth_header: true,
            compat: None,
            oauth_config: None,
        }
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderConfig")
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

/// Failure building provider wiring or loading the API key.
#[derive(Debug)]
pub enum ProviderError {
    /// The API key could not be read from env or the configured file.
    KeyUnavailable(String),
    /// The SDK provider factory rejected the model entry.
    Build(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::KeyUnavailable(message) => {
                write!(formatter, "DeepSeek API key unavailable: {message}")
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
