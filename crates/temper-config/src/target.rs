// SPDX-License-Identifier: MPL-2.0

//! Resolution for target-era pools/profiles. Pools remain metadata at plain
//! config resolution time; runtime adapters select one later for worker shape.

use std::collections::{BTreeMap, BTreeSet};

use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::error::ConfigError;
use crate::resolved::{AgentProfileSettings, ProviderKind, RepoPath, WorkerPoolSettings};
use crate::schema::{Config, Credentials};
use crate::secret_refs::{require_secret_payload, resolve_secret_reference};

pub(crate) struct ResolvedWorkerPools {
    pub pools: Vec<WorkerPoolSettings>,
    pub token_values: BTreeMap<String, SecretString>,
}

pub(crate) fn resolve_worker_pools(
    config: &Config,
    agent_profiles: &BTreeMap<String, AgentProfileSettings>,
    credentials: &Credentials,
    validate_secret_references: bool,
) -> Result<ResolvedWorkerPools, ConfigError> {
    let mut seen_names = BTreeSet::new();
    let mut pools = Vec::with_capacity(config.worker.pools.len());
    let mut token_values = BTreeMap::new();

    for (index, pool) in config.worker.pools.iter().enumerate() {
        let field = format!("worker.pools[{index}]");
        let name = trimmed(pool.name.as_deref())
            .ok_or_else(|| ConfigError::invalid(format!("{field}.name must not be empty")))?;
        if !seen_names.insert(name.clone()) {
            return Err(ConfigError::invalid(format!(
                "worker.pools.name contains duplicate pool `{name}`"
            )));
        }

        let roles = resolve_pool_roles(pool.roles.as_deref().unwrap_or(&[]), &field)?;
        let repos = resolve_pool_repos(pool.repos.as_deref().unwrap_or(&[]), &field)?;

        if pool.max_concurrent_jobs == Some(0) {
            return Err(ConfigError::invalid(format!(
                "{field}.max_concurrent_jobs must be greater than zero"
            )));
        }

        let agent_profile = trimmed(pool.agent_profile.as_deref());
        if let Some(profile) = &agent_profile {
            if !agent_profiles.is_empty() && !agent_profiles.contains_key(profile) {
                return Err(ConfigError::invalid(format!(
                    "{field}.agent_profile references unknown agent.profiles `{profile}`"
                )));
            }
        }

        let resolved_worker_token = resolve_secret_reference(
            &format!("{field}.worker_token"),
            pool.worker_token.as_deref(),
            credentials,
            validate_secret_references,
        )?;
        let worker_token = resolved_worker_token
            .as_ref()
            .map(|resolved| resolved.reference.clone());
        if let Some(value) = resolved_worker_token.and_then(|resolved| resolved.value) {
            token_values.insert(name.clone(), value);
        }

        pools.push(WorkerPoolSettings {
            name,
            roles,
            repos,
            max_concurrent_jobs: pool.max_concurrent_jobs,
            agent_profile,
            worker_token,
        });
    }

    Ok(ResolvedWorkerPools {
        pools,
        token_values,
    })
}

pub(crate) fn resolve_agent_profiles(
    config: &Config,
    credentials: &Credentials,
    validate_secret_references: bool,
) -> Result<BTreeMap<String, AgentProfileSettings>, ConfigError> {
    let mut profiles = BTreeMap::new();
    let mut seen_names = BTreeSet::new();

    for (raw_name, profile) in &config.agent.profiles {
        let name = trimmed(Some(raw_name.as_str()))
            .ok_or_else(|| ConfigError::invalid("agent.profiles profile name must not be empty"))?;
        if !seen_names.insert(name.clone()) {
            return Err(ConfigError::invalid(format!(
                "agent.profiles contains duplicate profile `{name}`"
            )));
        }

        let field = format!("agent.profiles.{name}");
        if profile.max_iterations == Some(0) {
            return Err(ConfigError::invalid(format!(
                "{field}.max_iterations must be greater than zero"
            )));
        }

        let provider = match profile.provider.as_deref() {
            Some(raw) if raw.trim().is_empty() => {
                return Err(ConfigError::invalid(format!(
                    "{field}.provider must not be empty"
                )));
            }
            Some(raw) => Some(parse_agent_profile_provider_kind(raw.trim(), &field)?),
            None => None,
        };

        let command = profile
            .command
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|part| part.trim().to_string())
            .collect();

        let credential = resolve_secret_reference(
            &format!("{field}.credential"),
            profile.credential.as_deref(),
            credentials,
            validate_secret_references,
        )?;
        let credential_json = credential
            .as_ref()
            .filter(|resolved| resolved.reference.available)
            .map(|resolved| {
                let secret = require_secret_payload(&format!("{field}.credential"), resolved)?;
                provider_credential_json_from_secret(&format!("{field}.credential"), &secret)
            })
            .transpose()?;

        profiles.insert(
            name,
            AgentProfileSettings {
                command,
                provider,
                model: trimmed(profile.model.as_deref()),
                investigate_model: trimmed(profile.investigate_model.as_deref()),
                provider_url: trimmed(profile.provider_url.as_deref()),
                max_iterations: profile.max_iterations,
                subagents: profile.subagents,
                credential: credential.map(|resolved| resolved.reference),
                credential_json,
            },
        );
    }

    Ok(profiles)
}

fn provider_credential_json_from_secret(
    field: &str,
    secret: &SecretString,
) -> Result<SecretString, ConfigError> {
    let raw = secret.expose_secret().trim();
    if raw.is_empty() {
        return Err(ConfigError::invalid(format!(
            "{field} references a secret with an empty provider credential payload"
        )));
    }

    let json = if raw.starts_with('{') {
        validate_provider_credential_json(field, raw)?
    } else {
        serde_json::to_string(&serde_json::json!({
            "type": "api-key",
            "api_key": raw,
        }))
        .map_err(|error| {
            ConfigError::invalid(format!("{field} provider credential JSON error: {error}"))
        })?
    };
    Ok(SecretString::from(json))
}

fn validate_provider_credential_json(field: &str, raw: &str) -> Result<String, ConfigError> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        ConfigError::invalid(format!(
            "{field} provider credential JSON is invalid: {error}"
        ))
    })?;
    match value.get("type").and_then(Value::as_str) {
        Some("api-key") => {
            let api_key = value.get("api_key").and_then(Value::as_str).unwrap_or("");
            if api_key.trim().is_empty() {
                return Err(ConfigError::invalid(format!(
                    "{field} provider credential JSON api_key must not be empty"
                )));
            }
        }
        Some("oauth") => {
            let access = value
                .get("access_token")
                .and_then(Value::as_str)
                .unwrap_or("");
            if access.trim().is_empty() {
                return Err(ConfigError::invalid(format!(
                    "{field} provider credential JSON access_token must not be empty"
                )));
            }
            if value
                .get("expires_at_unix_seconds")
                .and_then(Value::as_i64)
                .is_none()
            {
                return Err(ConfigError::invalid(format!(
                    "{field} provider credential JSON expires_at_unix_seconds must be an integer"
                )));
            }
            if let Some(refresh) = value.get("refresh_token") {
                if !(refresh.is_null() || refresh.is_string()) {
                    return Err(ConfigError::invalid(format!(
                        "{field} provider credential JSON refresh_token must be a string when set"
                    )));
                }
            }
        }
        Some(other) => {
            return Err(ConfigError::invalid(format!(
                "{field} provider credential JSON type `{other}` is unsupported (expected api-key or oauth)"
            )));
        }
        None => {
            return Err(ConfigError::invalid(format!(
                "{field} provider credential JSON must include a type"
            )));
        }
    }
    serde_json::to_string(&value).map_err(|error| {
        ConfigError::invalid(format!("{field} provider credential JSON error: {error}"))
    })
}

fn resolve_pool_roles(raw_roles: &[String], field: &str) -> Result<Vec<String>, ConfigError> {
    if raw_roles.is_empty() {
        return Err(ConfigError::invalid(format!(
            "{field}.roles must not be empty"
        )));
    }

    let mut roles = Vec::with_capacity(raw_roles.len());
    for (index, raw) in raw_roles.iter().enumerate() {
        let role = raw.trim();
        if role.is_empty() {
            return Err(ConfigError::invalid(format!(
                "{field}.roles[{index}] must not be empty"
            )));
        }
        roles.push(role.to_string());
    }
    Ok(dedup_strings(roles))
}

fn resolve_pool_repos(raw_repos: &[String], field: &str) -> Result<Vec<RepoPath>, ConfigError> {
    let mut repos = Vec::with_capacity(raw_repos.len());
    for (index, raw) in raw_repos.iter().enumerate() {
        let repo = raw.trim();
        let parsed = RepoPath::parse(repo).map_err(|_| {
            ConfigError::invalid(format!(
                "{field}.repos[{index}] must be `owner/name` with non-empty parts"
            ))
        })?;
        repos.push(parsed);
    }
    Ok(dedup_by(repos, |a, b| {
        a.owner == b.owner && a.name == b.name
    }))
}

fn parse_agent_profile_provider_kind(name: &str, field: &str) -> Result<ProviderKind, ConfigError> {
    match name {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "deepseek" => Ok(ProviderKind::DeepSeek),
        "chatgpt" => Ok(ProviderKind::ChatGpt),
        other => Err(ConfigError::invalid(format!(
            "{field}.provider has invalid provider `{other}` (expected anthropic, deepseek, or chatgpt)"
        ))),
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn dedup_strings(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

fn dedup_by<T>(items: Vec<T>, eq: impl Fn(&T, &T) -> bool) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(items.len());
    for item in items {
        if !out.iter().any(|existing| eq(existing, &item)) {
            out.push(item);
        }
    }
    out
}
