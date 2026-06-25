// SPDX-License-Identifier: MPL-2.0

//! Collapse the config file, credentials file, and built-in defaults into a
//! [`Resolved`] deployment.
//!
//! Deployment shape comes only from the config file (with built-in defaults as
//! the fallback): the new CLI exposes no per-field flags (the daemon takes only
//! `--config`, `--secrets`, and `--service`), and no environment variable
//! overrides deployment config. The injected [`EnvLookup`] is consulted solely
//! for `$HOME` / `$XDG_*` when expanding a leading `~` in a path value.
//!
//! Per-role forge credentials resolve from `[forge.users.<role>]` in the
//! credentials file; there is no per-role environment fallback.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use secrecy::SecretString;

use crate::env::EnvLookup;
use crate::error::ConfigError;
use crate::resolved::{
    AgentSettings, Capability, EngineSettings, ForgeKind, ForgeSettings, GitIdentity,
    ProviderCredential, ProviderKind, ProviderSettings, RepoPath, Resolved, WebUiCreds,
    WorkerSettings,
};
use crate::schema::{Config, Credentials};

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_POLL_CADENCE_SECS: u64 = 300;
/// Mechanical backstop runs by default. It is the level-triggered safety net
/// (webhooks accelerate it), so the cadence is conservative rather than
/// aggressive: a slow idle backstop. Set `mechanical_cadence_secs = 0` to
/// disable the mechanical worker entirely.
const DEFAULT_MECHANICAL_CADENCE_SECS: u64 = 120;
const DEFAULT_LEASE_TTL_SECS: u64 = 300;
const DEFAULT_DAEMON_ID: &str = "temper-daemon-1";
const DEFAULT_WORKER_ID: &str = "temper-worker-1";
/// Last-resort relative workspace root used only when neither `$XDG_STATE_HOME`
/// nor `$HOME` is set (so [`crate::paths::default_workspace_root`] returns
/// `None`). The normal default is the XDG state path
/// `~/.local/state/temper/workspace`.
const DEFAULT_WORKSPACE_ROOT_FALLBACK: &str = ".temper/workspace";
const DEFAULT_MAX_CONCURRENT_JOBS: u32 = 1;
const DEFAULT_POLL_WAIT_MS: u64 = 30_000;
const DEFAULT_HEARTBEAT_MS: u64 = 10_000;
const DEFAULT_CI_USER: &str = "bot";
/// Mirrors `temper_agent::DEFAULT_MAX_ITERATIONS`; kept here so this crate stays
/// free of any runtime-crate dependency. A drift test in the binary asserts they
/// agree.
const DEFAULT_MAX_ITERATIONS: usize = 250;

/// Context for resolving path values that came from an on-disk config file.
///
/// Direct callers that build a [`Config`] in memory can keep using [`resolve`],
/// which preserves the historical behavior: relative path strings stay relative
/// to the caller's process. Loaders that know which `config.toml` supplied the
/// values pass its parent directory here so relative config-file paths are
/// interpreted beside that file.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ResolveOptions {
    /// Directory containing the loaded config file, if there was one.
    pub config_base_dir: Option<PathBuf>,
}

impl ResolveOptions {
    /// Builds options that resolve relative config-file paths against `dir`.
    pub fn from_config_base_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            config_base_dir: Some(dir.into()),
        }
    }
}

/// Resolves a full deployment from the config and credentials files. The
/// injected environment is consulted only for `$HOME` / `$XDG_*` path expansion,
/// never to override a deployment value.
///
/// Relative path strings remain relative in this direct API. Use
/// [`resolve_with_options`] when the config came from a known file and should
/// resolve relative paths against that file's parent directory.
pub fn resolve(
    config: &Config,
    credentials: &Credentials,
    env: &impl EnvLookup,
) -> Result<Resolved, ConfigError> {
    resolve_with_options(config, credentials, env, &ResolveOptions::default())
}

/// Resolves a deployment with file-location context for config path fields.
pub fn resolve_with_options(
    config: &Config,
    credentials: &Credentials,
    env: &impl EnvLookup,
    options: &ResolveOptions,
) -> Result<Resolved, ConfigError> {
    let engine = resolve_engine(config, env, options)?;
    let worker = resolve_worker(config, env, &engine, options)?;
    let roles = referenced_roles(&engine, &worker);
    let forge = resolve_forge(config, credentials, &roles);
    let agent = resolve_agent(config, credentials, env, options)?;
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
    roles: &BTreeSet<String>,
) -> ForgeSettings {
    let url = trimmed(config.forge.url.as_deref()).map(|url| url.trim_end_matches('/').to_string());

    let admin = trimmed(config.forge.admin.as_deref());
    let admin_token = admin
        .as_deref()
        .and_then(|name| credentials.forge.users.get(name))
        .and_then(|user| trimmed(user.token.as_deref()))
        .map(SecretString::from);

    let ci_user =
        trimmed(config.forge.ci_user.as_deref()).unwrap_or_else(|| DEFAULT_CI_USER.to_string());
    let web_ui = resolve_web_ui(credentials, &ci_user);

    let mut role_tokens = BTreeMap::new();
    let mut role_identities = BTreeMap::new();
    for role in roles {
        if let Some(token) = role_token(credentials, role) {
            let user = role_user(credentials, role);
            let email = role_email(credentials, role, &user);
            role_tokens.insert(role.clone(), SecretString::from(token.clone()));
            role_identities.insert(
                role.clone(),
                GitIdentity {
                    user,
                    email,
                    token: SecretString::from(token),
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

fn resolve_web_ui(credentials: &Credentials, ci_user: &str) -> Option<WebUiCreds> {
    let user = credentials.forge.users.get(ci_user);
    let username = user
        .and_then(|u| trimmed(u.user.as_deref()))
        .unwrap_or_else(|| ci_user.to_string());
    let password = user.and_then(|u| trimmed(u.password.as_deref()))?;
    Some(WebUiCreds {
        username,
        password: SecretString::from(password),
    })
}

fn role_token(credentials: &Credentials, role: &str) -> Option<String> {
    credentials
        .forge
        .users
        .get(role)
        .and_then(|user| trimmed(user.token.as_deref()))
}

fn role_user(credentials: &Credentials, role: &str) -> String {
    credentials
        .forge
        .users
        .get(role)
        .and_then(|user| trimmed(user.user.as_deref()))
        .unwrap_or_else(|| role.to_string())
}

fn role_email(credentials: &Credentials, role: &str, user: &str) -> String {
    credentials
        .forge
        .users
        .get(role)
        .and_then(|u| trimmed(u.email.as_deref()))
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

fn resolve_engine(
    config: &Config,
    env: &impl EnvLookup,
    options: &ResolveOptions,
) -> Result<EngineSettings, ConfigError> {
    let bind = resolve_bind(config)?;

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

    let workflow_file = trimmed(config.engine.workflow.as_deref())
        .map(|value| resolve_config_path(&value, env, options));

    let poll_cadence = positive_duration_secs(
        config
            .engine
            .poll_cadence_secs
            .unwrap_or(DEFAULT_POLL_CADENCE_SECS),
        "engine.poll_cadence_secs",
    )?;
    // The mechanical backstop is on by default; webhooks are the primary
    // reaction path and this backstop is the level-triggered safety net. An
    // explicit `mechanical_cadence_secs = 0` disables the mechanical worker.
    let mechanical_cadence = match config
        .engine
        .mechanical_cadence_secs
        .unwrap_or(DEFAULT_MECHANICAL_CADENCE_SECS)
    {
        0 => None,
        secs => Some(positive_duration_secs(
            secs,
            "engine.mechanical_cadence_secs",
        )?),
    };
    let lease_ttl = positive_duration_secs(
        config
            .engine
            .lease_ttl_secs
            .unwrap_or(DEFAULT_LEASE_TTL_SECS),
        "engine.lease_ttl_secs",
    )?;

    let daemon_id = trimmed(config.engine.daemon_id.as_deref())
        .unwrap_or_else(|| DEFAULT_DAEMON_ID.to_string());

    let webhook_secret_file = trimmed(config.engine.webhook_secret_file.as_deref())
        .map(|value| resolve_config_path(&value, env, options));

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

fn resolve_bind(config: &Config) -> Result<SocketAddr, ConfigError> {
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

// ── worker ──────────────────────────────────────────────────────────────────

fn resolve_worker(
    config: &Config,
    env: &impl EnvLookup,
    engine: &EngineSettings,
    options: &ResolveOptions,
) -> Result<WorkerSettings, ConfigError> {
    let worker_id = trimmed(config.worker.worker_id.as_deref())
        .unwrap_or_else(|| DEFAULT_WORKER_ID.to_string());

    let daemon_url = trimmed(config.worker.daemon_url.as_deref())
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", engine.bind.port()));

    let workspace_root = trimmed(config.worker.workspace.as_deref())
        .map(|value| resolve_config_path(&value, env, options))
        .unwrap_or_else(|| {
            default_workspace_root(env)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_ROOT_FALLBACK))
        });

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
    let poll_wait =
        Duration::from_millis(config.worker.poll_wait_ms.unwrap_or(DEFAULT_POLL_WAIT_MS));
    let heartbeat_interval = Duration::from_millis(
        config
            .worker
            .heartbeat_interval_ms
            .unwrap_or(DEFAULT_HEARTBEAT_MS),
    );

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
        ConfigError::invalid(format!("capability `{raw}` must be `owner/name:role`"))
    })?;
    let repo = repo.trim();
    let role = role.trim();
    // Validate the repo half is owner/name.
    RepoPath::parse(repo).map_err(|_| {
        ConfigError::invalid(format!("capability `{raw}`: repo must be `owner/name`"))
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

fn resolve_agent(
    config: &Config,
    credentials: &Credentials,
    env: &impl EnvLookup,
    options: &ResolveOptions,
) -> Result<AgentSettings, ConfigError> {
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

    let credential = resolve_provider_credential(credentials, &provider_name, env);

    let provider = ProviderSettings {
        kind,
        main_model,
        investigate_model,
        base_url,
        credential,
    };

    Ok(AgentSettings {
        provider,
        max_iterations: config
            .agent
            .max_iterations
            .unwrap_or(DEFAULT_MAX_ITERATIONS),
        enable_subagents: config.agent.enable_subagents.unwrap_or(false),
        config_dir: trimmed(config.agent.config_dir.as_deref())
            .map(|value| resolve_config_path(&value, env, options)),
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

fn resolve_provider_credential(
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
    if (kind == "api-key" || kind == "api_key")
        && let Some(key) = trimmed(cred.key.as_deref())
    {
        return ProviderCredential::ApiKey(SecretString::from(key));
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

/// Resolves a path-valued field from `config.toml`.
///
/// Absolute paths and any `~`-prefixed path keep the long-standing behavior
/// (absolute paths are used verbatim; bare `~` / `~/…` expand through the
/// injected HOME; `~user` forms stay verbatim). Only plain relative paths use
/// the optional config-file parent context.
fn resolve_config_path(value: &str, env: &impl EnvLookup, options: &ResolveOptions) -> PathBuf {
    if is_tilde_prefixed(value) {
        return expand_tilde(value, env);
    }

    let path = PathBuf::from(value);
    if path.is_absolute() {
        return path;
    }

    options
        .config_base_dir
        .as_ref()
        .map(|base| base.join(&path))
        .unwrap_or(path)
}

fn is_tilde_prefixed(value: &str) -> bool {
    value.starts_with('~')
}

/// Expands a leading `~` / `~/…` in a path value against `$HOME`, so a
/// hand-written `workspace = "~/.local/state/temper/workspace"` resolves to the
/// service user's home regardless of who wrote it.
///
/// Only a bare `~` or a `~/` prefix is expanded (the common shell forms);
/// `~user` and a `~` anywhere but the start are left verbatim, and when `$HOME`
/// is unset the original value is returned unchanged.
fn expand_tilde(value: &str, env: &impl EnvLookup) -> PathBuf {
    if (value == "~" || value.starts_with("~/"))
        && let Some(home) = env.non_empty("HOME")
    {
        let rest = value.strip_prefix("~/").or_else(|| value.strip_prefix('~'));
        return match rest {
            Some(rest) if !rest.is_empty() => PathBuf::from(home).join(rest),
            _ => PathBuf::from(home),
        };
    }
    PathBuf::from(value)
}

/// The base `…/temper` state directory, computed from the injected environment
/// (mirrors [`crate::paths::state_dir`], which reads the process environment;
/// kept env-aware here so resolution stays unit-testable).
fn state_dir(env: &impl EnvLookup) -> Option<PathBuf> {
    if let Some(xdg) = env.non_empty("XDG_STATE_HOME") {
        return Some(PathBuf::from(xdg).join("temper"));
    }
    env.non_empty("HOME").map(|home| {
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("temper")
    })
}

/// The default worker workspace root (`<state-dir>/workspace`) from the injected
/// environment. `None` only when neither `$XDG_STATE_HOME` nor `$HOME` is set.
fn default_workspace_root(env: &impl EnvLookup) -> Option<PathBuf> {
    state_dir(env).map(|dir| dir.join("workspace"))
}

fn positive_duration_secs(secs: u64, field: &str) -> Result<Duration, ConfigError> {
    if secs == 0 {
        return Err(ConfigError::invalid(format!(
            "{field} must be greater than zero"
        )));
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
