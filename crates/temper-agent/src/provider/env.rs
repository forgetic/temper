//! The provider's env-derived inputs, carried as plain values.
//!
//! Everything the provider/credential wiring used to read from the process
//! environment is gathered here into one struct so the env reads can live in a
//! single host module ([`temper-agent-session`'s `entry`][entry]) and be passed
//! inward. No method on this struct touches `std::env`: it is constructed from
//! already-read values (or [`ProviderEnv::empty`] for tests/defaults).
//!
//! The field names mirror the worker→agent env channel
//! ([`temper_config::provider::provider_env`]): the worker injects these, the
//! agent's `entry` reads them back into this struct, and the provider factory
//! ([`super::ProviderConfig::from_auth`]) consumes them.
//!
//! [entry]: ../../../temper_agent_session/entry/index.html

use std::path::PathBuf;

use secrecy::SecretString;

/// The provider/credential inputs the host environment supplies, as values.
///
/// Built once in the agent's `entry` from the injected env (and CLI flags), then
/// handed to [`super::ProviderConfig::from_auth`]. Every field is optional: an
/// absent field falls back to the compiled-in default (model ids, token URLs) or
/// the default key file path (DeepSeek). Constructed empty in tests via
/// [`ProviderEnv::empty`].
#[derive(Clone, Default)]
pub struct ProviderEnv {
    /// The DeepSeek API key supplied directly (`TEMPER_DEEPSEEK_API_KEY`); takes
    /// precedence over [`Self::deepseek_api_key_path`].
    pub deepseek_api_key: Option<SecretString>,
    /// An override for the DeepSeek API-key file path
    /// (`TEMPER_DEEPSEEK_API_KEY_PATH`); used only when [`Self::deepseek_api_key`]
    /// is absent.
    pub deepseek_api_key_path: Option<PathBuf>,
    /// The shared OAuth `auth.json` path (`TEMPER_AGENTS_AUTH_FILE`); an env-level
    /// fallback below any explicit `--auth-file`.
    pub auth_file: Option<PathBuf>,
    /// The Codex (ChatGPT) model id (`TEMPER_AGENTS_CODEX_MODEL`).
    pub codex_model: Option<String>,
    /// The Codex OAuth token endpoint override (`TEMPER_AGENTS_CODEX_TOKEN_URL`,
    /// used by tests to point refresh at a local server).
    pub codex_token_url: Option<String>,
    /// The Anthropic model id (`TEMPER_AGENTS_ANTHROPIC_MODEL`).
    pub anthropic_model: Option<String>,
    /// The Anthropic sub-agent (investigate) model id
    /// (`TEMPER_AGENTS_ANTHROPIC_SUBAGENT_MODEL`).
    pub anthropic_subagent_model: Option<String>,
    /// The Anthropic OAuth token endpoint override
    /// (`TEMPER_AGENTS_ANTHROPIC_TOKEN_URL`, used by tests).
    pub anthropic_token_url: Option<String>,
    /// A base-URL redirect for outbound provider traffic
    /// (`ANVIL_TEST_PROVIDER_BASE_URL`); points the agent at a local fake LLM.
    pub base_url_override: Option<String>,
}

impl ProviderEnv {
    /// An all-absent provider env: every field falls back to its compiled-in
    /// default. The shape unit tests and in-process callers use when they pass
    /// the provider inputs explicitly rather than through the environment.
    pub fn empty() -> Self {
        Self::default()
    }
}
