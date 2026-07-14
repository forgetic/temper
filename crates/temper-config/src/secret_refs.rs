// SPDX-License-Identifier: MPL-2.0

//! Secret-name reference validation and lookup helpers.
//!
//! Config sections store target-era secret references as names only. This module
//! validates those names and, for runtime-compatible engine fields, resolves the
//! named payload through the selected secret source without exposing it to
//! inspection output.

use secrecy::SecretString;

use crate::error::ConfigError;
use crate::resolve_options::ResolveOptions;
use crate::resolved::SecretReference;
use crate::schema::{Config, Credentials};

#[derive(Debug, Clone, Default)]
pub(crate) struct EngineSecretReferences {
    pub forge_token: Option<SecretReference>,
    pub forge_token_value: Option<SecretString>,
    pub webhook_secret: Option<SecretReference>,
    pub webhook_secret_value: Option<SecretString>,
}

pub(crate) fn resolve_engine_secret_references(
    config: &Config,
    credentials: &Credentials,
    options: &ResolveOptions,
) -> Result<EngineSecretReferences, ConfigError> {
    let forge_token = resolve_secret_reference(
        "engine.forge_token",
        config.engine.forge_token.as_deref(),
        credentials,
        options.validate_secret_references,
    )?;
    let webhook_secret = resolve_secret_reference(
        "engine.webhook_secret",
        config.engine.webhook_secret.as_deref(),
        credentials,
        options.validate_secret_references,
    )?;

    let forge_token_value = forge_token
        .as_ref()
        .filter(|resolved| resolved.reference.available)
        .map(|resolved| require_secret_payload("engine.forge_token", resolved))
        .transpose()?;
    let webhook_secret_value = webhook_secret
        .as_ref()
        .filter(|resolved| resolved.reference.available)
        .map(|resolved| require_secret_payload("engine.webhook_secret", resolved))
        .transpose()?;

    Ok(EngineSecretReferences {
        forge_token: forge_token.map(|resolved| resolved.reference),
        forge_token_value,
        webhook_secret: webhook_secret.map(|resolved| resolved.reference),
        webhook_secret_value,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSecretReference {
    pub reference: SecretReference,
    pub value: Option<SecretString>,
}

pub(crate) fn resolve_secret_reference(
    field: &str,
    raw: Option<&str>,
    credentials: &Credentials,
    validate_exists: bool,
) -> Result<Option<ResolvedSecretReference>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let name = validate_secret_name(field, raw)?;
    let Some(secret) = credentials.named_secret(&name) else {
        if validate_exists {
            return Err(ConfigError::invalid(format!(
                "{field} references missing secret `{name}`"
            )));
        }
        return Ok(Some(ResolvedSecretReference {
            reference: SecretReference {
                name,
                available: false,
            },
            value: None,
        }));
    };

    let value = secret.value.map(SecretString::from);
    let available = validate_exists || value.is_some();
    Ok(Some(ResolvedSecretReference {
        reference: SecretReference { name, available },
        value,
    }))
}

pub(crate) fn require_secret_payload(
    field: &str,
    resolved: &ResolvedSecretReference,
) -> Result<SecretString, ConfigError> {
    resolved.value.clone().ok_or_else(|| {
        ConfigError::invalid(format!(
            "{field} references secret `{}` but it has no non-empty text value",
            resolved.reference.name
        ))
    })
}

fn validate_secret_name(field: &str, raw: &str) -> Result<String, ConfigError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ConfigError::invalid(format!("{field} must not be empty")));
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(ConfigError::invalid(format!(
            "{field} must be a secret name, not a path (`{name}`)"
        )));
    }
    if name.len() > 255
        || !name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(ConfigError::invalid(format!(
            "{field} must be a safe secret name using only ASCII letters, digits, `.`, `_`, or `-`"
        )));
    }
    Ok(name.to_string())
}
