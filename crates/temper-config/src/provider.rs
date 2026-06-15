// SPDX-License-Identifier: MPL-2.0

//! How a resolved provider maps onto the agent process's environment.
//!
//! The coding agent reads its provider/model/credential wiring from a fixed set
//! of environment variables (and, for OAuth, a pi-format `auth.json`). Both the
//! out-of-process worker (which spawns the agent and injects these per spawn)
//! and the in-process standalone mode share this mapping, so it lives here in
//! the tier-agnostic config crate rather than being duplicated.
//!
//! This module is pure: it produces the env pairs and the `auth.json` *content*
//! to write; the caller owns the actual file I/O.

use std::path::{Path, PathBuf};

use secrecy::ExposeSecret;

use crate::resolved::{ProviderCredential, ProviderKind, ProviderSettings};

/// Env var the agent reads for the OAuth `auth.json` path.
pub const AUTH_FILE_ENV: &str = "TEMPER_AGENTS_AUTH_FILE";
/// Env var the agent reads for the Codex (ChatGPT) model id.
pub const CODEX_MODEL_ENV: &str = "TEMPER_AGENTS_CODEX_MODEL";
/// Env var the agent reads for the Anthropic model id.
pub const ANTHROPIC_MODEL_ENV: &str = "TEMPER_AGENTS_ANTHROPIC_MODEL";
/// Env var the agent reads for the Anthropic sub-agent (investigate) model id.
pub const ANTHROPIC_SUBAGENT_MODEL_ENV: &str = "TEMPER_AGENTS_ANTHROPIC_SUBAGENT_MODEL";
/// Env var the agent reads for the DeepSeek API key.
pub const DEEPSEEK_API_KEY_ENV: &str = "TEMPER_DEEPSEEK_API_KEY";
/// Env var that redirects provider traffic to an alternate base URL.
pub const PROVIDER_BASE_URL_ENV: &str = "ANVIL_TEST_PROVIDER_BASE_URL";

/// The agent's `--auth` flag value for a provider kind.
pub fn auth_mode(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::DeepSeek => "deepseek",
        ProviderKind::ChatGpt => "chatgpt-oauth",
        ProviderKind::Anthropic => "anthropic-oauth",
    }
}

/// The `auth.json` provider key an OAuth credential is stored under.
pub fn oauth_provider_key(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::ChatGpt => "openai-codex",
        // DeepSeek never uses OAuth; the key is unused for it.
        ProviderKind::Anthropic | ProviderKind::DeepSeek => "anthropic",
    }
}

/// The environment to inject into an agent process so it builds the configured
/// provider. `auth_file` is the path the OAuth credential lives at (materialized
/// from inline tokens, or an explicit `auth_file`), if any.
pub fn provider_env(
    provider: &ProviderSettings,
    auth_file: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Some(model) = &provider.main_model {
        match provider.kind {
            ProviderKind::ChatGpt => env.push((CODEX_MODEL_ENV.to_string(), model.clone())),
            ProviderKind::Anthropic => env.push((ANTHROPIC_MODEL_ENV.to_string(), model.clone())),
            // DeepSeek's model is not env-overridable in the agent today.
            ProviderKind::DeepSeek => {}
        }
    }
    if let Some(sub) = &provider.investigate_model
        && provider.kind == ProviderKind::Anthropic
    {
        env.push((ANTHROPIC_SUBAGENT_MODEL_ENV.to_string(), sub.clone()));
    }
    if let Some(url) = &provider.base_url {
        env.push((PROVIDER_BASE_URL_ENV.to_string(), url.clone()));
    }
    if let ProviderCredential::ApiKey(key) = &provider.credential {
        // I/O boundary: the key crosses into the spawned agent's environment.
        env.push((
            DEEPSEEK_API_KEY_ENV.to_string(),
            key.expose_secret().to_string(),
        ));
    }
    if let Some(path) = auth_file {
        env.push((AUTH_FILE_ENV.to_string(), path.display().to_string()));
    }
    env
}

/// For an inline-OAuth credential, the pi-format `auth.json` content to write so
/// the agent's OAuth loader can read (and refresh) it. `None` for any other
/// credential (api-key, an external `auth_file`, or ambient).
pub fn oauth_auth_json(provider: &ProviderSettings) -> Option<String> {
    let ProviderCredential::OAuthInline {
        access,
        refresh,
        expires,
    } = &provider.credential
    else {
        return None;
    };
    let key = oauth_provider_key(provider.kind);
    // I/O boundary: the OAuth tokens are written to the agent's `auth.json`.
    let refresh = refresh.as_ref().map(|token| token.expose_secret());
    // The nodejs pi `auth.json` schema: `{ <key>: { type, access, refresh,
    // expires } }`, with `expires` in unix milliseconds (see the agent's
    // tolerant OAuth reader).
    let entry = serde_json::json!({
        key: {
            "type": "oauth",
            "access": access.expose_secret(),
            "refresh": refresh,
            "expires": expires,
        }
    });
    serde_json::to_string_pretty(&entry).ok()
}

/// The path of an explicit (already-on-disk) OAuth `auth.json`, when the
/// credential points at one rather than carrying inline tokens.
pub fn external_auth_file(provider: &ProviderSettings) -> Option<&Path> {
    match &provider.credential {
        ProviderCredential::OAuthFile(path) => Some(path.as_path()),
        _ => None,
    }
}

/// Resolves the OAuth `auth.json` path the agent should read, materializing
/// inline tokens into `dir` (created if needed, `0600` on Unix) or returning a
/// configured external path. Returns `None` for api-key/ambient credentials.
pub fn materialize_auth_file(
    provider: &ProviderSettings,
    dir: &Path,
) -> std::io::Result<Option<PathBuf>> {
    if let Some(json) = oauth_auth_json(provider) {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}-auth.json", provider.kind.as_str()));
        std::fs::write(&path, json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        return Ok(Some(path));
    }
    Ok(external_auth_file(provider).map(Path::to_path_buf))
}
