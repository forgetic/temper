// SPDX-License-Identifier: MPL-2.0

//! Builds the in-process [`ProviderConfig`] for the standalone agent from a
//! resolved deployment.
//!
//! The out-of-process worker injects the provider wiring into the agent
//! subprocess's environment (see [`temper_config::provider`]); the standalone
//! mode runs the agent in-process and so constructs the `ProviderConfig`
//! directly here, materializing inline OAuth tokens into an `auth.json` the
//! provider stack reads + refreshes.

use std::path::{Path, PathBuf};

use temper_agent::{
    ANTHROPIC_MODEL_ENV, ANTHROPIC_SUBAGENT_MODEL_ENV, ANTHROPIC_TOKEN_URL_ENV, API_KEY_ENV,
    API_KEY_PATH_ENV, AUTH_FILE_ENV, AuthChoice, CODEX_MODEL_ENV, CODEX_TOKEN_URL_ENV,
    PROVIDER_BASE_URL_ENV, ProviderConfig, ProviderEnv,
};
use temper_config::{ExposeSecret, ProviderCredential, ProviderKind, Resolved, Secret, provider};

/// Constructs the agent provider config from the resolved agent settings,
/// materializing OAuth credentials under `auth_dir` when given inline.
pub fn build_provider(resolved: &Resolved, auth_dir: &Path) -> Result<ProviderConfig, String> {
    let settings = &resolved.agent.provider;
    let auth_file = provider::materialize_auth_file(settings, auth_dir)
        .map_err(|error| format!("materialize agent auth file: {error}"))?;
    // Standalone mode runs the agent in-process, so it reads the same provider
    // env the out-of-process agent's `entry` reads (and that the worker injects)
    // and threads it into the value-taking provider factory.
    let env = read_provider_env();

    let config = match settings.kind {
        ProviderKind::DeepSeek => match &settings.credential {
            ProviderCredential::ApiKey(key) => ProviderConfig::deepseek_with_key(
                // I/O boundary: the key is handed to the in-process provider.
                key.expose_secret().to_string(),
                settings.main_model.clone(),
                settings.base_url.clone(),
            ),
            _ => ProviderConfig::from_auth(AuthChoice::DeepSeek, None, None, &env)
                .map_err(|error| error.to_string())?,
        },
        ProviderKind::ChatGpt => ProviderConfig::from_auth(
            AuthChoice::ChatGptOAuth,
            settings.main_model.clone(),
            auth_file,
            &env,
        )
        .map_err(|error| error.to_string())?,
        ProviderKind::Anthropic => {
            let mut config =
                ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, auth_file, &env)
                    .map_err(|error| error.to_string())?;
            if let Some(model) = &settings.main_model {
                config = config.with_model_id(model.clone());
            }
            config.with_subagent_model_id(settings.investigate_model.clone())
        }
    };

    let config = match &settings.base_url {
        Some(url) => config.with_base_url_override(url.clone()),
        None => config,
    };
    // The ANVIL_TEST_PROVIDER_BASE_URL env redirect wins over the config URL
    // (env overrides file), matching the out-of-process agent and the responders.
    Ok(config.apply_base_url_override(env.base_url_override.as_deref()))
}

/// Reads the worker→agent provider env into a [`ProviderEnv`]. The standalone
/// daemon is the in-process host, so reading env here is the legitimate analogue
/// of the out-of-process agent's `entry`.
fn read_provider_env() -> ProviderEnv {
    ProviderEnv {
        deepseek_api_key: var(API_KEY_ENV).map(Secret::from),
        deepseek_api_key_path: path_var(API_KEY_PATH_ENV),
        auth_file: path_var(AUTH_FILE_ENV),
        codex_model: var(CODEX_MODEL_ENV),
        codex_token_url: var(CODEX_TOKEN_URL_ENV),
        anthropic_model: var(ANTHROPIC_MODEL_ENV),
        anthropic_subagent_model: var(ANTHROPIC_SUBAGENT_MODEL_ENV),
        anthropic_token_url: var(ANTHROPIC_TOKEN_URL_ENV),
        base_url_override: var(PROVIDER_BASE_URL_ENV),
    }
}

/// Reads an env var, trimming and treating empty as unset.
fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Reads an env var as a path, treating empty as unset.
fn path_var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
