// SPDX-License-Identifier: MPL-2.0

//! Collapse the config file, credentials file, environment, and built-in
//! defaults into a [`Resolved`] deployment.
//!
//! Precedence is **environment override > file value > built-in default**, and
//! the new CLI exposes no per-field flags (the daemon takes only `--config`,
//! `--credentials`, and `--service`), so the file is the primary source.
//!
//! Per-role forge credentials resolve from `[forge.users.<role>]` in the
//! credentials file, falling back to the legacy `TEMPER_FORGEJO_{USER,EMAIL,
//! TOKEN}_<ROLE>` environment so existing provisioned `roles.env` files keep
//! working through the transition.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::env::EnvLookup;
use crate::error::ConfigError;
use crate::resolved::{
    AgentSettings, Capability, EngineSettings, ForgeKind, ForgeSettings, GitIdentity,
    ProviderCredential, ProviderKind, ProviderSettings, RepoPath, Resolved, WebUiCreds,
    WorkerSettings,
};
use crate::schema::{Config, Credentials};

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_POLL_CADENCE_SECS: u64 = 30;
const DEFAULT_LEASE_TTL_SECS: u64 = 300;
const DEFAULT_DAEMON_ID: &str = "temper-daemon-1";
const DEFAULT_WORKER_ID: &str = "temper-worker-1";
const DEFAULT_WORKSPACE_ROOT: &str = ".temper/workspaces";
const DEFAULT_MAX_CONCURRENT_JOBS: u32 = 1;
const DEFAULT_POLL_WAIT_MS: u64 = 30_000;
const DEFAULT_HEARTBEAT_MS: u64 = 10_000;
const DEFAULT_CI_USER: &str = "bot";
/// Mirrors `temper_agent::DEFAULT_MAX_ITERATIONS`; kept here so this crate stays
/// free of any runtime-crate dependency. A drift test in the binary asserts they
/// agree.
const DEFAULT_MAX_ITERATIONS: usize = 250;

/// Resolves a full deployment from the two files plus the environment.
pub fn resolve(
    config: &Config,
    credentials: &Credentials,
    env: &impl EnvLookup,
) -> Result<Resolved, ConfigError> {
    let engine = resolve_engine(config, env)?;
    let worker = resolve_worker(config, env, &engine)?;
    let roles = referenced_roles(&engine, &worker);
    let forge = resolve_forge(config, credentials, env, &roles);
    let agent = resolve_agent(config, credentials)?;
    Ok(Resolved {
        forge,
        engine,
        worker,
        agent,
    })
}

// ── forge ───────────────────────────────────────────────────────────────────

fn resolve_forge(
    config: &Config,
    credentials: &Credentials,
    env: &impl EnvLookup,
    roles: &BTreeSet<String>,
) -> ForgeSettings {
    let url = env
        .non_empty("TEMPER_FORGE_URL")
        .or_else(|| env.non_empty("FORGEJO_URL"))
        .or_else(|| trimmed(config.forge.url.as_deref()))
        .map(|url| url.trim_end_matches('/').to_string());

    let admin = trimmed(config.forge.admin.as_deref());
    let admin_token = env
        .non_empty("TEMPER_FORGE_TOKEN")
        .or_else(|| env.non_empty("FORGEJO_ACCESS_TOKEN"))
        .or_else(|| {
            admin
                .as_deref()
                .and_then(|name| credentials.forge.users.get(name))
                .and_then(|user| trimmed(user.token.as_deref()))
        });

    let ci_user = trimmed(config.forge.ci_user.as_deref())
        .unwrap_or_else(|| DEFAULT_CI_USER.to_string());
    let web_ui = resolve_web_ui(credentials, env, &ci_user);

    let mut role_tokens = BTreeMap::new();
    let mut role_identities = BTreeMap::new();
    for role in roles {
        if let Some(token) = role_token(credentials, env, role) {
            role_tokens.insert(role.clone(), token.clone());
            let user = role_user(credentials, env, role);
            let email = role_email(credentials, env, role, &user);
            role_identities.insert(
                role.clone(),
                GitIdentity {
                    user,
                    email,
                    token,
                },
            );
        }
    }

    ForgeSettings {
        kind: ForgeKind::Forgejo,
        url,
        admin_token,
        web_ui,
        role_tokens,
        role_identities,
    }
}

fn resolve_web_ui(
    credentials: &Credentials,
    env: &impl EnvLookup,
    ci_user: &str,
) -> Option<WebUiCreds> {
    let user = credentials.forge.users.get(ci_user);
    let username = env
        .non_empty("FORGEJO_USERNAME")
        .or_else(|| user.and_then(|u| trimmed(u.user.as_deref())))
        .unwrap_or_else(|| ci_user.to_string());
    let password = env
        .non_empty("FORGEJO_PASSWORD")
        .or_else(|| user.and_then(|u| trimmed(u.password.as_deref())))?;
    Some(WebUiCreds { username, password })
}

fn role_token(credentials: &Credentials, env: &impl EnvLookup, role: &str) -> Option<String> {
    credentials
        .forge
        .users
        .get(role)
        .and_then(|user| trimmed(user.token.as_deref()))
        .or_else(|| env.non_empty(&format!("TEMPER_FORGEJO_TOKEN_{}", env_role_key(role))))
}

fn role_user(credentials: &Credentials, env: &impl EnvLookup, role: &str) -> String {
    credentials
        .forge
        .users
        .get(role)
        .and_then(|user| trimmed(user.user.as_deref()))
        .or_else(|| env.non_empty(&format!("TEMPER_FORGEJO_USER_{}", env_role_key(role))))
        .unwrap_or_else(|| role.to_string())
}

fn role_email(
    credentials: &Credentials,
    env: &impl EnvLookup,
    role: &str,
    user: &str,
) -> String {
    credentials
        .forge
        .users
        .get(role)
        .and_then(|u| trimmed(u.email.as_deref()))
        .or_else(|| env.non_empty(&format!("TEMPER_FORGEJO_EMAIL_{}", env_role_key(role))))
        .unwrap_or_else(|| format!("{user}@noreply.localhost"))
}

/// The env-var suffix for a role: uppercased, with every non-`[A-Z0-9]`
/// character replaced by `_` (matching the legacy provisioning convention).
pub fn env_role_key(role: &str) -> String {
    role.chars()
        .flat_map(char::to_uppercase)
        .map(|ch| {
            if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

// ── engine ──────────────────────────────────────────────────────────────────

fn resolve_engine(config: &Config, env: &impl EnvLookup) -> Result<EngineSettings, ConfigError> {
    let bind = resolve_bind(config, env)?;

    let repos = config
        .engine
        .repos
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|raw| RepoPath::parse(raw.trim()))
        .collect::<Result<Vec<_>, _>>()?;
    let repos = dedup_by(repos, |a, b| a.owner == b.owner && a.name == b.name);

    let roles = dedup_strings(
        config
            .engine
            .roles
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|role| role.trim().to_string())
            .filter(|role| !role.is_empty()),
    );

    let workflow_file = env
        .non_empty("TEMPER_WORKFLOW")
        .or_else(|| trimmed(config.engine.workflow.as_deref()))
        .map(PathBuf::from);

    let poll_cadence = positive_duration_secs(
        config.engine.poll_cadence_secs.unwrap_or(DEFAULT_POLL_CADENCE_SECS),
        "engine.poll_cadence_secs",
    )?;
    let mechanical_cadence = config
        .engine
        .mechanical_cadence_secs
        .map(|secs| positive_duration_secs(secs, "engine.mechanical_cadence_secs"))
        .transpose()?;
    let lease_ttl = positive_duration_secs(
        config.engine.lease_ttl_secs.unwrap_or(DEFAULT_LEASE_TTL_SECS),
        "engine.lease_ttl_secs",
    )?;

    let daemon_id = trimmed(config.engine.daemon_id.as_deref())
        .unwrap_or_else(|| DEFAULT_DAEMON_ID.to_string());

    let webhook_secret_file = trimmed(config.engine.webhook_secret_file.as_deref()).map(PathBuf::from);

    Ok(EngineSettings {
        bind,
        repos,
        roles,
        workflow_file,
        poll_cadence,
        mechanical_cadence,
        lease_ttl,
        webhook_secret_file,
        daemon_id,
    })
}

fn resolve_bind(config: &Config, env: &impl EnvLookup) -> Result<SocketAddr, ConfigError> {
    if let Some(bind) = env.non_empty("TEMPER_ENGINE_BIND") {
        return parse_bind(&bind);
    }
    if let Some(port) = env.non_empty("TEMPER_ENGINE_PORT") {
        return parse_port(&port);
    }
    if let Some(bind) = trimmed(config.engine.bind.as_deref()) {
        return parse_bind(&bind);
    }
    if let Some(port) = config.engine.port {
        return parse_bind(&format!("127.0.0.1:{port}"));
    }
    parse_bind(DEFAULT_BIND)
}

fn parse_bind(raw: &str) -> Result<SocketAddr, ConfigError> {
    raw.parse::<SocketAddr>()
        .map_err(|error| ConfigError::invalid(format!("invalid engine bind `{raw}`: {error}")))
}

fn parse_port(raw: &str) -> Result<SocketAddr, ConfigError> {
    let port: u16 = raw
        .parse()
        .map_err(|error| ConfigError::invalid(format!("invalid engine port `{raw}`: {error}")))?;
    parse_bind(&format!("127.0.0.1:{port}"))
}

// ── worker ──────────────────────────────────────────────────────────────────

fn resolve_worker(
    config: &Config,
    env: &impl EnvLookup,
    engine: &EngineSettings,
) -> Result<WorkerSettings, ConfigError> {
    let worker_id = trimmed(config.worker.worker_id.as_deref())
        .unwrap_or_else(|| DEFAULT_WORKER_ID.to_string());

    let daemon_url = env
        .non_empty("TEMPER_DAEMON_URL")
        .or_else(|| trimmed(config.worker.daemon_url.as_deref()))
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", engine.bind.port()));

    let workspace_root = env
        .non_empty("TEMPER_WORKSPACE")
        .or_else(|| trimmed(config.worker.workspace.as_deref()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_ROOT));

    let git_base_url = trimmed(config.worker.git_base_url.as_deref());

    let max_concurrent_jobs = config
        .worker
        .max_concurrent_jobs
        .unwrap_or(DEFAULT_MAX_CONCURRENT_JOBS);
    if max_concurrent_jobs == 0 {
        return Err(ConfigError::invalid(
            "worker.max_concurrent_jobs must be greater than zero",
        ));
    }
    let poll_wait = Duration::from_millis(config.worker.poll_wait_ms.unwrap_or(DEFAULT_POLL_WAIT_MS));
    let heartbeat_interval =
        Duration::from_millis(config.worker.heartbeat_interval_ms.unwrap_or(DEFAULT_HEARTBEAT_MS));

    let capabilities = match &config.worker.capabilities {
        Some(raw) => raw
            .iter()
            .map(|capability| parse_capability(capability.trim()))
            .collect::<Result<Vec<_>, _>>()?,
        None => default_capabilities(engine),
    };
    let capabilities = dedup_by(capabilities, |a, b| a.repo == b.repo && a.role == b.role);

    Ok(WorkerSettings {
        worker_id,
        daemon_url,
        workspace_root,
        git_base_url,
        max_concurrent_jobs,
        poll_wait,
        heartbeat_interval,
        capabilities,
    })
}

/// Default capabilities: the cross-product of the engine's repos and roles, so a
/// single standalone worker covers the whole feed.
fn default_capabilities(engine: &EngineSettings) -> Vec<Capability> {
    let mut capabilities = Vec::new();
    for repo in &engine.repos {
        for role in &engine.roles {
            capabilities.push(Capability {
                repo: repo.display(),
                role: role.clone(),
            });
        }
    }
    capabilities
}

fn parse_capability(raw: &str) -> Result<Capability, ConfigError> {
    let (repo, role) = raw.split_once(':').ok_or_else(|| {
        ConfigError::invalid(format!(
            "capability `{raw}` must be `owner/name:role`"
        ))
    })?;
    let repo = repo.trim();
    let role = role.trim();
    // Validate the repo half is owner/name.
    RepoPath::parse(repo).map_err(|_| {
        ConfigError::invalid(format!(
            "capability `{raw}`: repo must be `owner/name`"
        ))
    })?;
    if role.is_empty() {
        return Err(ConfigError::invalid(format!(
            "capability `{raw}`: role must not be empty"
        )));
    }
    Ok(Capability {
        repo: repo.to_string(),
        role: role.to_string(),
    })
}

// ── agent ───────────────────────────────────────────────────────────────────

fn resolve_agent(config: &Config, credentials: &Credentials) -> Result<AgentSettings, ConfigError> {
    let provider_name = trimmed(config.agent.provider.as_deref())
        .unwrap_or_else(|| ProviderKind::Anthropic.as_str().to_string());
    let kind = parse_provider_kind(&provider_name)?;

    let profile = config.agent.providers.get(&provider_name);
    let main_model = profile
        .and_then(|p| p.models.as_ref())
        .and_then(|m| trimmed(m.main.as_deref()));
    let investigate_model = profile
        .and_then(|p| p.models.as_ref())
        .and_then(|m| trimmed(m.investigate.as_deref()));
    let base_url = profile.and_then(|p| trimmed(p.url.as_deref()));

    let credential = resolve_provider_credential(credentials, &provider_name);

    let provider = ProviderSettings {
        kind,
        main_model,
        investigate_model,
        base_url,
        credential,
    };

    Ok(AgentSettings {
        provider,
        max_iterations: config.agent.max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS),
        enable_subagents: config.agent.enable_subagents.unwrap_or(false),
        config_dir: trimmed(config.agent.config_dir.as_deref()).map(PathBuf::from),
    })
}

fn parse_provider_kind(name: &str) -> Result<ProviderKind, ConfigError> {
    match name {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "deepseek" => Ok(ProviderKind::DeepSeek),
        "chatgpt" | "chatgpt-oauth" | "codex" => Ok(ProviderKind::ChatGpt),
        other => Err(ConfigError::invalid(format!(
            "unknown agent provider `{other}` (expected anthropic, deepseek, or chatgpt)"
        ))),
    }
}

fn resolve_provider_credential(credentials: &Credentials, provider_name: &str) -> ProviderCredential {
    let Some(cred) = credentials.agent.providers.get(provider_name) else {
        return ProviderCredential::Ambient;
    };
    if let Some(path) = trimmed(cred.auth_file.as_deref()) {
        return ProviderCredential::OAuthFile(PathBuf::from(path));
    }
    let kind = trimmed(cred.kind.as_deref()).unwrap_or_default();
    if (kind == "api-key" || kind == "api_key")
        && let Some(key) = trimmed(cred.key.as_deref()) {
            return ProviderCredential::ApiKey(key);
        }
    if let Some(access) = trimmed(cred.access.as_deref()) {
        return ProviderCredential::OAuthInline {
            access,
            refresh: trimmed(cred.refresh.as_deref()),
            expires: cred.expires.unwrap_or(0),
        };
    }
    if let Some(key) = trimmed(cred.key.as_deref()) {
        return ProviderCredential::ApiKey(key);
    }
    ProviderCredential::Ambient
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// The roles whose credentials we need to resolve: the engine's roles plus any
/// role named only in worker capabilities.
fn referenced_roles(engine: &EngineSettings, worker: &WorkerSettings) -> BTreeSet<String> {
    let mut roles: BTreeSet<String> = engine.roles.iter().cloned().collect();
    for capability in &worker.capabilities {
        roles.insert(capability.role.clone());
    }
    roles
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn positive_duration_secs(secs: u64, field: &str) -> Result<Duration, ConfigError> {
    if secs == 0 {
        return Err(ConfigError::invalid(format!("{field} must be greater than zero")));
    }
    Ok(Duration::from_secs(secs))
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
