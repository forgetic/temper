// SPDX-License-Identifier: MPL-2.0

//! Resolution helpers for active legacy agent provider settings.

use std::path::PathBuf;

use secrecy::SecretString;

use crate::env::EnvLookup;
use crate::error::ConfigError;
use crate::resolved::{ProviderCredential, ProviderKind};
use crate::schema::Credentials;

pub(crate) fn parse_provider_kind(name: &str) -> Result<ProviderKind, ConfigError> {
    match name {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "deepseek" => Ok(ProviderKind::DeepSeek),
        "chatgpt" | "chatgpt-oauth" | "codex" => Ok(ProviderKind::ChatGpt),
        other => Err(ConfigError::invalid(format!(
            "unknown agent provider `{other}` (expected anthropic, deepseek, or chatgpt)"
        ))),
    }
}

pub(crate) fn resolve_provider_credential(
    credentials: &Credentials,
    provider_name: &str,
    env: &impl EnvLookup,
) -> ProviderCredential {
    let Some(cred) = credentials.agent.providers.get(provider_name) else {
        return ProviderCredential::Ambient;
    };
    if let Some(path) = trimmed(cred.auth_file.as_deref()) {
        return ProviderCredential::OAuthFile(expand_tilde(&path, env));
    }
    let kind = trimmed(cred.kind.as_deref()).unwrap_or_default();
    if kind == "api-key" || kind == "api_key" {
        if let Some(key) = trimmed(cred.key.as_deref()) {
            return ProviderCredential::ApiKey(SecretString::from(key));
        }
    }
    if let Some(access) = trimmed(cred.access.as_deref()) {
        return ProviderCredential::OAuthInline {
            access: SecretString::from(access),
            refresh: trimmed(cred.refresh.as_deref()).map(SecretString::from),
            expires: cred.expires.unwrap_or(0),
        };
    }
    if let Some(key) = trimmed(cred.key.as_deref()) {
        return ProviderCredential::ApiKey(SecretString::from(key));
    }
    ProviderCredential::Ambient
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn expand_tilde(value: &str, env: &impl EnvLookup) -> PathBuf {
    if value == "~" || value.starts_with("~/") {
        if let Some(home) = env.non_empty("HOME") {
            let rest = value.strip_prefix("~/").or_else(|| value.strip_prefix('~'));
            return match rest {
                Some(rest) if !rest.is_empty() => PathBuf::from(home).join(rest),
                _ => PathBuf::from(home),
            };
        }
    }
    PathBuf::from(value)
}
