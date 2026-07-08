// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use temper_cli_common::{EnvLookup, EnvMap, PathResolver, expand_tilde};
use temper_config::{
    AgentProfileSettings, ExposeSecret, ProviderCredential, ProviderKind, Resolved,
    SecretReference, WorkerPoolSettings,
};

use super::super::finding::{CheckCategory, CheckFinding};
use super::validate_http_base_url;

const DEEPSEEK_API_KEY_ENV: &str = "TEMPER_DEEPSEEK_API_KEY";
const DEEPSEEK_API_KEY_PATH_ENV: &str = "TEMPER_DEEPSEEK_API_KEY_PATH";
const AGENT_AUTH_FILE_ENV: &str = "TEMPER_AGENTS_AUTH_FILE";
const DEFAULT_DEEPSEEK_KEY_PATH: &str = ".cache/deepseek-api-key";

pub(super) fn add_standalone_provider_checks(
    resolved: &Resolved,
    env: &EnvMap,
    paths: &PathResolver,
    findings: &mut Vec<CheckFinding>,
) {
    let mut checked = BTreeSet::new();
    add_active_provider_check(resolved, env, paths, &mut checked, findings);
    for (name, profile) in &resolved.agent.profiles {
        add_profile_provider_check(resolved, env, paths, name, profile, &mut checked, findings);
    }
}

pub(super) fn add_worker_provider_checks(
    resolved: &Resolved,
    env: &EnvMap,
    paths: &PathResolver,
    pool_name: Option<&str>,
    findings: &mut Vec<CheckFinding>,
) {
    let mut checked = BTreeSet::new();
    match pool_name {
        Some(name) => {
            if let Some(pool) = resolved.worker.pools.iter().find(|pool| pool.name == name) {
                add_pool_provider_check(resolved, env, paths, pool, &mut checked, findings);
            }
        }
        None if !resolved.worker.pools.is_empty() => {
            for pool in &resolved.worker.pools {
                add_pool_provider_check(resolved, env, paths, pool, &mut checked, findings);
            }
        }
        None => add_active_provider_check(resolved, env, paths, &mut checked, findings),
    }
}

fn add_pool_provider_check(
    resolved: &Resolved,
    env: &EnvMap,
    paths: &PathResolver,
    pool: &WorkerPoolSettings,
    checked: &mut BTreeSet<String>,
    findings: &mut Vec<CheckFinding>,
) {
    match pool.agent_profile.as_deref() {
        Some(profile_name) => {
            if let Some(profile) = resolved.agent.profiles.get(profile_name) {
                add_profile_provider_check(
                    resolved,
                    env,
                    paths,
                    profile_name,
                    profile,
                    checked,
                    findings,
                );
            }
        }
        None => add_active_provider_check(resolved, env, paths, checked, findings),
    }
}

fn add_active_provider_check(
    resolved: &Resolved,
    env: &EnvMap,
    paths: &PathResolver,
    checked: &mut BTreeSet<String>,
    findings: &mut Vec<CheckFinding>,
) {
    if !checked.insert("active".to_string()) {
        return;
    }
    let provider = &resolved.agent.provider;
    validate_provider_url(
        provider.kind,
        provider.base_url.as_deref(),
        "agent provider",
        "provider",
        findings,
    );
    validate_provider_credential(
        provider.kind,
        &provider.credential,
        env,
        paths,
        "agent provider",
        "provider",
        findings,
    );
}

fn add_profile_provider_check(
    resolved: &Resolved,
    _env: &EnvMap,
    _paths: &PathResolver,
    name: &str,
    profile: &AgentProfileSettings,
    checked: &mut BTreeSet<String>,
    findings: &mut Vec<CheckFinding>,
) {
    if !checked.insert(format!("profile:{name}")) {
        return;
    }
    let kind = profile.provider.unwrap_or(resolved.agent.provider.kind);
    let url = profile
        .provider_url
        .as_deref()
        .or(resolved.agent.provider.base_url.as_deref());
    let label = format!("agent profile `{name}`");
    let scope = format!("profile:{name}");
    validate_provider_url(kind, url, &label, &scope, findings);
    validate_profile_credential(name, profile.credential.as_ref(), &scope, findings);
}

fn validate_provider_url(
    _kind: ProviderKind,
    url: Option<&str>,
    label: &str,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    if let Some(url) = url {
        if let Err(message) = validate_http_base_url(url, &format!("{label} provider URL")) {
            findings.push(CheckFinding::online_error(
                scope.to_string(),
                CheckCategory::Provider,
                message,
            ));
        }
    }
}

fn validate_profile_credential(
    name: &str,
    credential: Option<&SecretReference>,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    match credential {
        Some(reference) if reference.available => {}
        Some(reference) => findings.push(CheckFinding::online_error(
            scope.to_string(),
            CheckCategory::Provider,
            format!(
                "agent profile `{name}` credential secret `{}` is unavailable for online provider validation",
                reference.name
            ),
        )),
        None => findings.push(CheckFinding::online_error(
            scope.to_string(),
            CheckCategory::Provider,
            format!("agent profile `{name}` has no credential for online provider validation"),
        )),
    }
}

fn validate_provider_credential(
    kind: ProviderKind,
    credential: &ProviderCredential,
    env: &EnvMap,
    paths: &PathResolver,
    label: &str,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    match credential {
        ProviderCredential::ApiKey(key) => {
            if key.expose_secret().trim().is_empty() {
                findings.push(provider_error(
                    scope,
                    format!("{label} API key credential is empty"),
                ));
            }
        }
        ProviderCredential::OAuthInline {
            access,
            refresh,
            expires,
        } => validate_inline_oauth(
            access.expose_secret(),
            refresh,
            *expires,
            label,
            scope,
            findings,
        ),
        ProviderCredential::OAuthFile(path) => validate_oauth_file(path, label, scope, findings),
        ProviderCredential::Ambient => {
            validate_ambient_credential(kind, env, paths, label, scope, findings);
        }
    }
}

fn validate_inline_oauth(
    access: &str,
    refresh: &Option<temper_config::Secret>,
    expires: i64,
    label: &str,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    if access.trim().is_empty() {
        findings.push(provider_error(
            scope,
            format!("{label} OAuth access token is empty"),
        ));
        return;
    }
    if expires > 0 && expires > now_ms() {
        return;
    }
    let has_refresh = refresh
        .as_ref()
        .is_some_and(|token| !token.expose_secret().trim().is_empty());
    if has_refresh {
        findings.push(CheckFinding::online_note(
            scope.to_string(),
            CheckCategory::Provider,
            format!("{label} OAuth access token is expired; a refresh token is configured"),
        ));
    } else if expires != 0 {
        findings.push(provider_error(
            scope,
            format!("{label} OAuth access token is expired and no refresh token is configured"),
        ));
    }
}

fn validate_oauth_file(path: &Path, label: &str, scope: &str, findings: &mut Vec<CheckFinding>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            findings.push(provider_error(
                scope,
                format!("{label} OAuth auth file {} does not exist", path.display()),
            ));
            return;
        }
        Err(error) => {
            findings.push(provider_error(
                scope,
                format!(
                    "failed to read {label} OAuth auth file {}: {error}",
                    path.display()
                ),
            ));
            return;
        }
    };
    validate_oauth_json_text(&text, label, path, scope, findings);
}

fn validate_ambient_credential(
    kind: ProviderKind,
    env: &EnvMap,
    paths: &PathResolver,
    label: &str,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    match kind {
        ProviderKind::DeepSeek => {
            if env.non_empty(DEEPSEEK_API_KEY_ENV).is_some() {
                return;
            }
            let path = env
                .non_empty(DEEPSEEK_API_KEY_PATH_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_DEEPSEEK_KEY_PATH));
            validate_non_empty_file(&path, label, scope, findings);
        }
        ProviderKind::ChatGpt | ProviderKind::Anthropic => {
            let path = ambient_auth_file_path(env, paths);
            let Some(path) = path else {
                findings.push(provider_error(
                    scope,
                    format!("{label} ambient OAuth auth file is unavailable"),
                ));
                return;
            };
            validate_oauth_file(&path, label, scope, findings);
        }
    }
}

fn ambient_auth_file_path(env: &EnvMap, paths: &PathResolver) -> Option<PathBuf> {
    env.non_empty(AGENT_AUTH_FILE_ENV)
        .map(|raw| expand_tilde(&raw, paths.home.as_deref()))
        .or_else(|| {
            paths
                .home
                .as_ref()
                .map(|home| home.join(".pi/agent/auth.json"))
        })
}

fn validate_non_empty_file(
    path: &Path,
    label: &str,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {}
        Ok(_) => findings.push(provider_error(
            scope,
            format!("{label} credential file {} is empty", path.display()),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => findings.push(provider_error(
            scope,
            format!("{label} credential file {} does not exist", path.display()),
        )),
        Err(error) => findings.push(provider_error(
            scope,
            format!(
                "failed to read {label} credential file {}: {error}",
                path.display()
            ),
        )),
    }
}

fn validate_oauth_json_text(
    text: &str,
    label: &str,
    path: &Path,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    if text.trim().is_empty() {
        findings.push(provider_error(
            scope,
            format!("{label} OAuth auth file {} is empty", path.display()),
        ));
        return;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) if oauth_value_has_token_material(&value) => {}
        Ok(Value::Object(_)) => findings.push(provider_error(
            scope,
            format!(
                "{label} OAuth auth file {} does not contain recognizable token fields",
                path.display()
            ),
        )),
        Ok(_) => findings.push(provider_error(
            scope,
            format!(
                "{label} OAuth auth file {} is not a JSON object",
                path.display()
            ),
        )),
        Err(error) => findings.push(provider_error(
            scope,
            format!(
                "failed to parse {label} OAuth auth file {} as JSON: {error}",
                path.display()
            ),
        )),
    }
}

fn oauth_value_has_token_material(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.iter().any(|(key, value)| {
        matches!(
            key.as_str(),
            "access" | "access_token" | "refresh" | "refresh_token" | "id_token"
        ) || oauth_value_has_token_material(value)
    })
}

fn provider_error(scope: &str, message: impl Into<String>) -> CheckFinding {
    CheckFinding::online_error(scope.to_string(), CheckCategory::Provider, message)
}

fn now_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    i64::try_from(millis).unwrap_or(i64::MAX)
}
