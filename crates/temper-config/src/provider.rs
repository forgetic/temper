// SPDX-License-Identifier: MPL-2.0

//! How a resolved provider maps onto the agent process's command line + the one
//! secret env var.
//!
//! The coding agent takes every non-secret input as a CLI flag (`--provider`,
//! `--model`, `--investigate-model`, `--provider-url`, …) and reads exactly one
//! secret from the environment: the provider credential JSON
//! (`TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`). Both the out-of-process worker
//! (which spawns the agent and injects this per spawn) and the in-process
//! standalone mode share this mapping, so it lives here in the tier-agnostic
//! config crate rather than being duplicated.
//!
//! This module is pure: it produces the flag strings and the credential JSON
//! *value*; the caller owns spawning the process and injecting the env. The
//! credential JSON shape matches `temper_protocol_agent::ProviderCredentialJson`
//! (the agent's parse target); it is emitted here with `serde_json` directly so
//! the tier-agnostic config crate keeps its zero-internal-dependency footprint.

use std::path::{Path, PathBuf};

use secrecy::ExposeSecret;

use crate::resolved::{ProviderCredential, ProviderKind, ProviderSettings};

/// The env var carrying the provider credential JSON. The agent reads it under
/// the same name (`temper_protocol_agent::PROVIDER_CREDENTIALS_ENV`); duplicated
/// here as a plain string so the config crate needs no protocol dependency.
pub const PROVIDER_CREDENTIALS_ENV: &str = "TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON";

/// The agent's `--provider` flag value for a provider kind.
pub fn provider_flag(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::DeepSeek => "deepseek",
        ProviderKind::ChatGpt => "chatgpt",
        ProviderKind::Anthropic => "anthropic",
    }
}

/// The agent CLI flags that carry the (non-secret) provider/model wiring:
/// `--provider`, plus `--model`/`--investigate-model`/`--provider-url` when the
/// resolved settings supply them.
///
/// The credential is **not** here — it crosses as the one secret env var (see
/// [`provider_credentials_json`]).
pub fn provider_flags(provider: &ProviderSettings) -> Vec<String> {
    let mut flags = vec![
        "--provider".to_string(),
        provider_flag(provider.kind).to_string(),
    ];
    if let Some(model) = &provider.main_model {
        flags.push("--model".to_string());
        flags.push(model.clone());
    }
    if let Some(sub) = &provider.investigate_model {
        flags.push("--investigate-model".to_string());
        flags.push(sub.clone());
    }
    if let Some(url) = &provider.base_url {
        flags.push("--provider-url".to_string());
        flags.push(url.clone());
    }
    flags
}

/// The provider credential JSON the worker injects via
/// [`PROVIDER_CREDENTIALS_ENV`], built from the resolved credential.
///
/// Returns `None` for an `Ambient` credential (no secret to forward) and for an
/// `OAuthFile` reference (the worker has only a path, not inline tokens — the
/// caller reads the file when it needs to forward it; the in-tree worker always
/// resolves inline tokens). The `expires` field in the credentials file is in
/// unix **milliseconds**; the JSON contract carries unix **seconds**, so it is
/// converted here (the agent converts back when it materializes its `auth.json`).
pub fn provider_credentials_json(provider: &ProviderSettings) -> Option<String> {
    // I/O boundary: the key/tokens cross into the agent's env as JSON. The shape
    // mirrors `temper_protocol_agent::ProviderCredentialJson`.
    let value = match &provider.credential {
        ProviderCredential::ApiKey(key) => serde_json::json!({
            "type": "api-key",
            "api_key": key.expose_secret(),
        }),
        ProviderCredential::OAuthInline {
            access,
            refresh,
            expires,
        } => serde_json::json!({
            "type": "oauth",
            "access_token": access.expose_secret(),
            "refresh_token": refresh.as_ref().map(ExposeSecret::expose_secret),
            "expires_at_unix_seconds": expires / 1_000,
        }),
        ProviderCredential::OAuthFile(_) | ProviderCredential::Ambient => return None,
    };
    serde_json::to_string(&value).ok()
}

/// Builds the credential JSON forwarded to an out-of-process first-party agent.
///
/// Inline credentials use [`provider_credentials_json`]. An `OAuthFile` is a
/// pi-format `auth.json`, so this reads only the configured provider entry and
/// converts its millisecond expiry to the child protocol's seconds.
pub fn provider_credentials_json_for_child(
    provider: &ProviderSettings,
) -> Result<Option<String>, String> {
    let ProviderCredential::OAuthFile(path) = &provider.credential else {
        return Ok(provider_credentials_json(provider));
    };
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("read provider OAuth file `{}`: {error}", path.display()))?;
    let document: serde_json::Value = serde_json::from_str(&source)
        .map_err(|error| format!("parse provider OAuth file `{}`: {error}", path.display()))?;
    let key = oauth_provider_key(provider.kind);
    let entry = document
        .get(key)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            format!(
                "provider OAuth file `{}` has no object entry `{key}`",
                path.display()
            )
        })?;
    let access = entry
        .get("access")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "provider OAuth file `{}` has no access token",
                path.display()
            )
        })?;
    let expires = entry
        .get("expires")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            format!(
                "provider OAuth file `{}` has no integer expiry",
                path.display()
            )
        })?;
    let value = serde_json::json!({
        "type": "oauth",
        "access_token": access,
        "refresh_token": entry.get("refresh").and_then(serde_json::Value::as_str),
        "expires_at_unix_seconds": expires / 1_000,
    });
    serde_json::to_string(&value)
        .map(Some)
        .map_err(|error| format!("serialize provider OAuth credential: {error}"))
}

/// The `auth.json` provider key an OAuth credential is stored under.
pub fn oauth_provider_key(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::ChatGpt => "openai-codex",
        // DeepSeek never uses OAuth; the key is unused for it.
        ProviderKind::Anthropic | ProviderKind::DeepSeek => "anthropic",
    }
}

/// Materializes an inline OAuth credential into a pi-format `auth.json` under
/// `dir` (created if needed, `0600` on Unix) for the resolved provider kind, or
/// returns a configured external `auth_file` path. `None` for api-key/ambient
/// credentials (the OAuth loader then uses its default file).
///
/// Used by the **in-process** standalone host, which builds the provider config
/// directly from the resolved values. (The out-of-process agent receives the
/// tokens via the credential JSON env and materializes its own file.)
pub fn materialize_auth_file(
    provider: &ProviderSettings,
    dir: &Path,
) -> std::io::Result<Option<PathBuf>> {
    match &provider.credential {
        ProviderCredential::OAuthInline {
            access,
            refresh,
            expires,
        } => {
            std::fs::create_dir_all(dir)?;
            // The nodejs pi schema: `{ <key>: { type, access, refresh, expires } }`
            // with `expires` in unix milliseconds (matching the resolved field).
            let entry = serde_json::json!({
                oauth_provider_key(provider.kind): {
                    "type": "oauth",
                    "access": access.expose_secret(),
                    "refresh": refresh.as_ref().map(ExposeSecret::expose_secret),
                    "expires": expires,
                }
            });
            let json = serde_json::to_string_pretty(&entry).unwrap_or_default();
            let path = dir.join(format!("{}-auth.json", provider.kind.as_str()));
            std::fs::write(&path, json)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(Some(path))
        }
        ProviderCredential::OAuthFile(path) => Ok(Some(path.clone())),
        ProviderCredential::ApiKey(_) | ProviderCredential::Ambient => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Secret;

    fn settings(kind: ProviderKind, credential: ProviderCredential) -> ProviderSettings {
        ProviderSettings {
            kind,
            main_model: None,
            investigate_model: None,
            base_url: None,
            credential,
        }
    }

    #[test]
    fn provider_flags_carry_models_and_url() {
        let provider = ProviderSettings {
            kind: ProviderKind::Anthropic,
            main_model: Some("claude-opus-4-8".to_string()),
            investigate_model: Some("claude-haiku-4-5".to_string()),
            base_url: Some("http://fake-llm".to_string()),
            credential: ProviderCredential::Ambient,
        };
        let flags = provider_flags(&provider);
        assert_eq!(
            flags,
            vec![
                "--provider",
                "anthropic",
                "--model",
                "claude-opus-4-8",
                "--investigate-model",
                "claude-haiku-4-5",
                "--provider-url",
                "http://fake-llm",
            ]
        );
    }

    #[test]
    fn api_key_credential_json() {
        let provider = settings(
            ProviderKind::DeepSeek,
            ProviderCredential::ApiKey(Secret::from("sk-x".to_string())),
        );
        let json = provider_credentials_json(&provider).expect("json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(value["type"], "api-key");
        assert_eq!(value["api_key"], "sk-x");
    }

    #[test]
    fn oauth_inline_credential_json_converts_expiry_to_seconds() {
        let provider = settings(
            ProviderKind::Anthropic,
            ProviderCredential::OAuthInline {
                access: Secret::from("acc".to_string()),
                refresh: Some(Secret::from("ref".to_string())),
                expires: 1_781_371_005_373,
            },
        );
        let json = provider_credentials_json(&provider).expect("json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(value["type"], "oauth");
        assert_eq!(value["access_token"], "acc");
        assert_eq!(value["refresh_token"], "ref");
        assert_eq!(value["expires_at_unix_seconds"], 1_781_371_005_i64);
    }

    #[test]
    fn ambient_and_oauth_file_have_no_credential_json() {
        assert!(
            provider_credentials_json(&settings(
                ProviderKind::Anthropic,
                ProviderCredential::Ambient
            ))
            .is_none()
        );
        assert!(
            provider_credentials_json(&settings(
                ProviderKind::ChatGpt,
                ProviderCredential::OAuthFile("/tmp/auth.json".into())
            ))
            .is_none()
        );
    }

    #[test]
    fn oauth_file_credential_json_selects_provider_and_converts_expiry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai-codex":{"type":"oauth","access":"acc","refresh":"ref","expires":1781371005373},"anthropic":{"type":"oauth","access":"other","expires":1}}"#,
        )
        .unwrap();
        let provider = settings(ProviderKind::ChatGpt, ProviderCredential::OAuthFile(path));

        let json = provider_credentials_json_for_child(&provider)
            .unwrap()
            .expect("credential JSON");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "oauth");
        assert_eq!(value["access_token"], "acc");
        assert_eq!(value["refresh_token"], "ref");
        assert_eq!(value["expires_at_unix_seconds"], 1_781_371_005_i64);
    }
}
