//! Provider/credential failures and DeepSeek API-key loading.
//!
//! The error type every provider entry point surfaces, plus the runtime
//! API-key resolution for the DeepSeek (`ApiKey`) auth mode. Key bytes are never
//! logged: errors carry only the env-var name or file path, never the value.

use std::fmt;
use std::path::{Path, PathBuf};

/// Env var that overrides the DeepSeek API-key file path.
pub const API_KEY_PATH_ENV: &str = "TEMPER_DEEPSEEK_API_KEY_PATH";
/// Env var that supplies the DeepSeek API key directly (takes precedence over
/// the file path).
pub const API_KEY_ENV: &str = "TEMPER_DEEPSEEK_API_KEY";

/// Default file the key is read from when no env override is set.
const DEFAULT_KEY_PATH: &str = ".cache/deepseek-api-key";

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
pub(super) fn load_api_key() -> Result<String, ProviderError> {
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
