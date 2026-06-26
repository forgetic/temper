// SPDX-License-Identifier: MPL-2.0

//! Secret-name reference validation and lookup helpers.
//!
//! Config sections store target-era secret references as names only. This module
//! validates those names and, for runtime-compatible engine fields, resolves the
//! named payload through the selected secret source without exposing it to
//! inspection output.

use std::path::PathBuf;

use secrecy::SecretString;

use crate::error::ConfigError;
use crate::resolved::SecretReference;
use crate::schema::Credentials;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSecretReference {
    pub reference: SecretReference,
    pub value: Option<SecretString>,
    pub source_path: Option<PathBuf>,
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
            source_path: None,
        }));
    };

    Ok(Some(ResolvedSecretReference {
        reference: SecretReference {
            name,
            available: true,
        },
        value: secret.value.map(SecretString::from),
        source_path: secret.source_path,
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
    if name.contains('\0') {
        return Err(ConfigError::invalid(format!(
            "{field} contains an invalid NUL byte"
        )));
    }
    Ok(name.to_string())
}
