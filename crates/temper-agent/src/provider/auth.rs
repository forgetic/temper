//! Auth-mode selection types: the credential family a provider config uses.
//!
//! [`AuthChoice`] is the caller-facing selector; [`AuthMode`] is the resolved,
//! credential-carrying variant the config stores.

use secrecy::SecretString;

use super::anthropic_oauth;
use super::oauth;

/// Which credential the real agents authenticate with.
///
/// The library default is [`AuthChoice::DeepSeek`] (so callers choose concrete
/// production wiring explicitly); anvil's CLI/preflight and responder binaries
/// default to [`AuthChoice::ChatGptOAuth`] per the local cost policy (a flat
/// subscription instead of pay-per-token). Resolve a [`super::ProviderConfig`]
/// for a choice with [`super::ProviderConfig::from_auth`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthChoice {
    /// DeepSeek API key (pay-per-token). The library default.
    DeepSeek,
    /// ChatGPT (OpenAI Codex) OAuth subscription. The test/dev default.
    ChatGptOAuth,
    /// Anthropic OAuth subscription targeting Claude through `anthropic-messages`.
    AnthropicOAuth,
}

/// How an agent decision authenticates to its LLM provider.
#[derive(Clone)]
pub(super) enum AuthMode {
    /// A static API key carried as the per-request bearer (DeepSeek default).
    ApiKey { api_key: SecretString },
    /// ChatGPT (OpenAI Codex) OAuth: the bearer is resolved fresh per decision
    /// from the shared auth file (load → refresh → access token).
    ChatGptOAuth { settings: oauth::OAuthSettings },
    /// Anthropic OAuth: the bearer and Claude Code-compatible request identity
    /// headers are resolved fresh per decision from the shared auth file.
    AnthropicOAuth {
        settings: anthropic_oauth::AnthropicOAuthSettings,
    },
}

impl AuthMode {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::ApiKey { .. } => "api_key",
            Self::ChatGptOAuth { .. } => "chatgpt_oauth",
            Self::AnthropicOAuth { .. } => "anthropic_oauth",
        }
    }
}
