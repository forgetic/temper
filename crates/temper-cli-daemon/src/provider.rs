// SPDX-License-Identifier: MPL-2.0

//! Builds the in-process [`ProviderConfig`] for the standalone agent from a
//! resolved deployment.
//!
//! The out-of-process worker injects the provider wiring into the agent
//! subprocess's environment (see [`temper_config::provider`]); the standalone
//! mode runs the agent in-process and so constructs the `ProviderConfig`
//! directly here, materializing inline OAuth tokens into an `auth.json` the
//! provider stack reads + refreshes.

use std::path::Path;

use temper_agent::{AuthChoice, ProviderConfig};
use temper_config::{ExposeSecret, ProviderCredential, ProviderKind, Resolved, provider};

/// Constructs the agent provider config from the resolved agent settings,
/// materializing OAuth credentials under `auth_dir` when given inline.
pub fn build_provider(resolved: &Resolved, auth_dir: &Path) -> Result<ProviderConfig, String> {
    let settings = &resolved.agent.provider;
    let auth_file = provider::materialize_auth_file(settings, auth_dir)
        .map_err(|error| format!("materialize agent auth file: {error}"))?;

    let config = match settings.kind {
        ProviderKind::DeepSeek => match &settings.credential {
            ProviderCredential::ApiKey(key) => ProviderConfig::deepseek_with_key(
                // I/O boundary: the key is handed to the in-process provider.
                key.expose_secret().to_string(),
                settings.main_model.clone(),
                settings.base_url.clone(),
            ),
            _ => ProviderConfig::from_auth(AuthChoice::DeepSeek, None, None)
                .map_err(|error| error.to_string())?,
        },
        ProviderKind::ChatGpt => ProviderConfig::from_auth(
            AuthChoice::ChatGptOAuth,
            settings.main_model.clone(),
            auth_file,
        )
        .map_err(|error| error.to_string())?,
        ProviderKind::Anthropic => {
            let mut config = ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, auth_file)
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
    Ok(config.apply_base_url_override_from_env())
}
