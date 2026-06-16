//! Per-request knobs derived from the resolved auth mode.
//!
//! These [`ProviderConfig`] accessors translate the auth mode into the bearer,
//! identity headers, mandatory system block, temperature, and reasoning effort
//! each provider call needs. They are grouped here so the auth-mode → request
//! mapping lives in one place, separate from config construction.

use std::collections::HashMap;

use secrecy::ExposeSecret;
use tongs::model::ThinkingLevel;

use super::auth::AuthMode;
use super::{ProviderConfig, ProviderError, anthropic_oauth};

impl ProviderConfig {
    /// Resolves the per-request bearer.
    ///
    /// For [`AuthMode::ApiKey`] this is the stored key. For OAuth modes it reads
    /// (and refreshes when near expiry) the shared auth file, so it must be
    /// called **each time** a decision runs. Callers must not log the result.
    pub(crate) async fn resolve_bearer(&self) -> Result<String, ProviderError> {
        match self.auth() {
            // I/O boundary: the static key becomes the per-request bearer.
            AuthMode::ApiKey { api_key } => Ok(api_key.expose_secret().to_string()),
            AuthMode::ChatGptOAuth { settings } => settings.resolve_bearer().await,
            AuthMode::AnthropicOAuth { settings } => settings.resolve_bearer().await,
        }
    }

    /// Extra per-request headers for this mode.
    ///
    /// Anthropic OAuth injects Claude Code-compatible request identity headers;
    /// all other modes use the SDK defaults.
    pub(crate) fn request_headers(&self) -> HashMap<String, String> {
        match self.auth() {
            // The beta-flag set depends on the model: the 1M-context beta is not
            // granted to every model/tier (Haiku rejects it with a 400 on the
            // standard subscription), so the headers are model-aware.
            AuthMode::AnthropicOAuth { .. } => anthropic_oauth::request_headers(self.model_id()),
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
        match self.auth() {
            AuthMode::AnthropicOAuth { .. } => Some(anthropic_oauth::CLAUDE_CODE_SYSTEM_IDENTITY),
            AuthMode::ApiKey { .. } | AuthMode::ChatGptOAuth { .. } => None,
        }
    }

    /// The request temperature for this mode. API-key (DeepSeek) pins `0.0` for
    /// deterministic decisions; Codex reasoning models and Anthropic OAuth leave
    /// temperature unset.
    pub(crate) fn temperature(&self) -> Option<f32> {
        match self.auth() {
            AuthMode::ApiKey { .. } => Some(0.0),
            AuthMode::ChatGptOAuth { .. } | AuthMode::AnthropicOAuth { .. } => None,
        }
    }

    /// The reasoning effort for this mode. API-key and Anthropic OAuth requests
    /// leave it unset; the codex reasoning models read effort from this field,
    /// so Codex OAuth requests the **lowest supported** effort to keep one-shot
    /// JSON decisions fast and cheap. Live validation found Codex models reject
    /// `minimal` (supported values are `none`/`low`/`medium`/`high`/
    /// `xhigh`), so Codex OAuth uses [`ThinkingLevel::Low`], not `Minimal`.
    pub(crate) fn thinking_level(&self) -> Option<ThinkingLevel> {
        match self.auth() {
            AuthMode::ApiKey { .. } | AuthMode::AnthropicOAuth { .. } => None,
            AuthMode::ChatGptOAuth { .. } => Some(ThinkingLevel::Low),
        }
    }

    /// The reasoning effort for the **coding/triage workspace** agent, which does
    /// real multi-step work (read/edit/verify) and benefits from maximal
    /// reasoning — distinct from [`thinking_level`](Self::thinking_level), used by
    /// the lightweight one-shot role-decision path. The codex reasoning models
    /// read effort from `stream_options.thinking_level`, so under Codex OAuth the
    /// coding agent requests the **highest** supported effort (`xhigh`). Non-codex
    /// providers (DeepSeek API key, Anthropic OAuth) leave it unset, matching
    /// `thinking_level`.
    pub(crate) fn coding_thinking_level(&self) -> Option<ThinkingLevel> {
        match self.auth() {
            AuthMode::ApiKey { .. } | AuthMode::AnthropicOAuth { .. } => None,
            AuthMode::ChatGptOAuth { .. } => Some(ThinkingLevel::XHigh),
        }
    }
}
