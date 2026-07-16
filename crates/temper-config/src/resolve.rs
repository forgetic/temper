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

use crate::agent_resolve::{parse_provider_kind, resolve_provider_credential};
use crate::agent_trace_resolve::resolve_observability;
use crate::deadline_resolve::{
    positive_duration_millis, resolve_agent_operation_limits, resolve_worker_liveness_limits,
    validate_liveness_ordering,
};
use crate::env::EnvLookup;
use crate::error::ConfigError;
use crate::resolved::{
    AgentSettings, AgentToolSettings, Capability, CodebaseMemoryIndex, CodebaseMemoryMode,
    CodebaseMemoryToolSettings, DeploymentSettings, DeploymentTopology, EngineSettings, ForgeKind,
    ForgeSettings, GitIdentity, PathSettings, ProviderKind, ProviderSettings, RepoPath, Resolved,
    WebUiCreds, WorkerSettings,
};
use crate::schema::{Config, Credentials};
use crate::secret_refs::{EngineSecretReferences, resolve_engine_secret_references};
use crate::target::{resolve_agent_profiles, resolve_worker_pools};

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
const DEFAULT_CODEBASE_MEMORY_MODE: &str = "auto";
const DEFAULT_CODEBASE_MEMORY_COMMAND: &str = "codebase-memory-mcp";
const DEFAULT_CODEBASE_MEMORY_INDEX: &str = "background";
const DEFAULT_CODEBASE_MEMORY_STARTUP_TIMEOUT_SECS: u64 = 5;
const DEFAULT_CODEBASE_MEMORY_INDEX_TIMEOUT_SECS: u64 = 30;

pub use crate::resolve_options::ResolveOptions;

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
    let deployment = resolve_deployment(config)?;
    let state_dir = resolve_state_dir(config, env, options);
    let engine_secrets = resolve_engine_secret_references(config, credentials, options)?;
    let engine = resolve_engine(config, env, options, &engine_secrets)?;
    let agent = resolve_agent(config, credentials, env, options)?;
    let worker = resolve_worker(
        config,
        credentials,
        env,
        &engine,
        state_dir.as_ref(),
        options,
        &agent,
    )?;
    let observability = resolve_observability(
        config,
        credentials,
        state_dir.as_deref(),
        &worker.workspace_root,
        options,
    )?;
    let paths = PathSettings {
        state_dir,
        workspace_dir: worker.workspace_root.clone(),
        workflow_file: engine.workflow_file.clone(),
    };
    let roles = referenced_roles(&engine, &worker);
    let forge = resolve_forge(
        config,
        credentials,
        &roles,
        engine_secrets.forge_token_value.clone(),
    );
    Ok(Resolved {
        deployment,
        paths,
        observability,
        forge,
        engine,
        worker,
        agent,
    })
}

// ── deployment ──────────────────────────────────────────────────────────────

fn resolve_deployment(config: &Config) -> Result<DeploymentSettings, ConfigError> {
    let name = trimmed(config.deployment.name.as_deref());
    let topology = trimmed(config.deployment.topology.as_deref())
        .map(|topology| parse_deployment_topology(&topology))
        .transpose()?;

    Ok(DeploymentSettings { name, topology })
}

fn parse_deployment_topology(raw: &str) -> Result<DeploymentTopology, ConfigError> {
    match raw {
        "standalone" => Ok(DeploymentTopology::Standalone),
        "distributed" => Ok(DeploymentTopology::Distributed),
        other => Err(ConfigError::invalid(format!(
            "invalid deployment.topology `{other}` (expected `standalone` or `distributed`)"
        ))),
    }
}

fn resolve_state_dir(
    config: &Config,
    env: &impl EnvLookup,
    options: &ResolveOptions,
) -> Option<PathBuf> {
    trimmed(config.paths.state_dir.as_deref())
        .map(|value| resolve_config_path(&value, env, options))
        .or_else(|| default_state_dir(env))
}

// ── forge ───────────────────────────────────────────────────────────────────

fn resolve_forge(
    config: &Config,
    credentials: &Credentials,
    roles: &BTreeSet<String>,
    named_admin_token: Option<SecretString>,
) -> ForgeSettings {
    let url = trimmed(config.forge.url.as_deref()).map(|url| url.trim_end_matches('/').to_string());

    let admin = trimmed(config.forge.admin.as_deref());
    let legacy_admin_token = admin
        .as_deref()
        .and_then(|name| credentials.forge.users.get(name))
        .and_then(|user| trimmed(user.token.as_deref()))
        .map(SecretString::from);
    let admin_token = named_admin_token.or(legacy_admin_token);

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
    secrets: &EngineSecretReferences,
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

    let workflow_file = resolve_preferred_config_path(
        "workflow.file",
        config.workflow.file.as_deref(),
        "engine.workflow",
        config.engine.workflow.as_deref(),
        env,
        options,
    )?;

    let poll_cadence = positive_duration_secs(
        config
            .engine
            .poll_cadence_secs
            .unwrap_or(DEFAULT_POLL_CADENCE_SECS),
        "engine.poll_cadence_secs",
    )?;
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

    let daemon_id = resolve_identity(
        config.engine.daemon_id.as_deref(),
        "engine.daemon_id",
        DEFAULT_DAEMON_ID,
    )?;

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
        forge_token: secrets.forge_token.clone(),
        webhook_secret: secrets.webhook_secret.clone(),
        webhook_secret_value: secrets.webhook_secret_value.clone(),
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
    credentials: &Credentials,
    env: &impl EnvLookup,
    engine: &EngineSettings,
    state_dir: Option<&PathBuf>,
    options: &ResolveOptions,
    agent: &AgentSettings,
) -> Result<WorkerSettings, ConfigError> {
    let worker_id = resolve_identity(
        config.worker.worker_id.as_deref(),
        "worker.worker_id",
        DEFAULT_WORKER_ID,
    )?;

    let daemon_url = trimmed(config.worker.daemon_url.as_deref())
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", engine.bind.port()));

    let workspace_root = resolve_preferred_config_path(
        "paths.workspace_dir",
        config.paths.workspace_dir.as_deref(),
        "worker.workspace",
        config.worker.workspace.as_deref(),
        env,
        options,
    )?
    .unwrap_or_else(|| {
        state_dir
            .map(|dir| dir.join("workspace"))
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
    let heartbeat_interval = positive_duration_millis(
        config
            .worker
            .heartbeat_interval_ms
            .unwrap_or(DEFAULT_HEARTBEAT_MS),
        "worker.heartbeat_interval_ms",
    )?;
    let liveness_limits = resolve_worker_liveness_limits(config)?;
    validate_liveness_ordering(heartbeat_interval, liveness_limits, agent)?;

    let capabilities = match &config.worker.capabilities {
        Some(raw) => raw
            .iter()
            .map(|capability| parse_capability(capability.trim()))
            .collect::<Result<Vec<_>, _>>()?,
        None => default_capabilities(engine),
    };
    let capabilities = dedup_by(capabilities, |a, b| a.repo == b.repo && a.role == b.role);
    let resolved_pools = resolve_worker_pools(
        config,
        &agent.profiles,
        credentials,
        options.validate_secret_references,
    )?;
    let result_root = state_dir
        .map(|dir| dir.join("worker-results"))
        .unwrap_or_else(|| workspace_root.join(".temper").join("worker-results"));

    Ok(WorkerSettings {
        worker_id,
        daemon_url,
        workspace_root,
        result_root,
        git_base_url,
        max_concurrent_jobs,
        poll_wait,
        heartbeat_interval,
        liveness_limits,
        capabilities,
        pools: resolved_pools.pools,
        worker_pool_tokens: resolved_pools.token_values,
        selected_pool: None,
    })
}

fn resolve_identity(
    raw: Option<&str>,
    field: &str,
    default_value: &str,
) -> Result<String, ConfigError> {
    match raw {
        Some(value) if value.trim().is_empty() => {
            Err(ConfigError::invalid(format!("{field} must not be empty")))
        }
        Some(value) => Ok(value.trim().to_string()),
        None => Ok(default_value.to_string()),
    }
}

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
    let tools = resolve_agent_tools(config)?;
    let operation_limits =
        resolve_agent_operation_limits(&config.agent.deadlines, None, "agent.deadlines")?;

    let provider = ProviderSettings {
        kind,
        main_model,
        investigate_model,
        base_url,
        credential,
    };
    let profiles = resolve_agent_profiles(
        config,
        credentials,
        options.validate_secret_references,
        operation_limits,
    )?;

    Ok(AgentSettings {
        provider,
        max_iterations: config
            .agent
            .max_iterations
            .unwrap_or(DEFAULT_MAX_ITERATIONS),
        enable_subagents: config.agent.enable_subagents.unwrap_or(false),
        config_dir: trimmed(config.agent.config_dir.as_deref())
            .map(|value| resolve_config_path(&value, env, options)),
        operation_limits,
        tools,
        profiles,
    })
}

fn resolve_agent_tools(config: &Config) -> Result<AgentToolSettings, ConfigError> {
    let codebase_memory = config
        .agent
        .tools
        .codebase_memory
        .as_ref()
        .map(resolve_codebase_memory_tool)
        .transpose()?
        .flatten();
    Ok(AgentToolSettings { codebase_memory })
}

fn resolve_codebase_memory_tool(
    raw: &crate::schema::CodebaseMemoryToolConfig,
) -> Result<Option<CodebaseMemoryToolSettings>, ConfigError> {
    let mode = parse_codebase_memory_mode(
        &trimmed(raw.mode.as_deref()).unwrap_or_else(|| DEFAULT_CODEBASE_MEMORY_MODE.to_string()),
    )?;
    let Some(mode) = mode else {
        return Ok(None);
    };

    let command = match raw.command.as_deref() {
        Some(command) if command.trim().is_empty() => {
            return Err(ConfigError::invalid(
                "agent.tools.codebase_memory.command must not be empty when enabled",
            ));
        }
        Some(command) => command.trim().to_string(),
        None => DEFAULT_CODEBASE_MEMORY_COMMAND.to_string(),
    };

    let args = raw
        .args
        .as_deref()
        .map(|args| dedup_strings(args.iter().filter_map(|arg| trimmed(Some(arg.as_str())))))
        .unwrap_or_default();
    let roles = match raw.roles.as_deref() {
        Some(roles) => resolve_codebase_memory_roles(roles)?,
        None => vec!["*".to_string()],
    };
    let index = parse_codebase_memory_index(
        &trimmed(raw.index.as_deref()).unwrap_or_else(|| DEFAULT_CODEBASE_MEMORY_INDEX.to_string()),
    )?;
    let startup_timeout_secs = positive_secs_value(
        raw.startup_timeout_secs
            .unwrap_or(DEFAULT_CODEBASE_MEMORY_STARTUP_TIMEOUT_SECS),
        "agent.tools.codebase_memory.startup_timeout_secs",
    )?;
    let index_timeout_secs = positive_secs_value(
        raw.index_timeout_secs
            .unwrap_or(DEFAULT_CODEBASE_MEMORY_INDEX_TIMEOUT_SECS),
        "agent.tools.codebase_memory.index_timeout_secs",
    )?;

    Ok(Some(CodebaseMemoryToolSettings {
        mode,
        command,
        args,
        roles,
        index,
        startup_timeout_secs,
        index_timeout_secs,
    }))
}

fn parse_codebase_memory_mode(raw: &str) -> Result<Option<CodebaseMemoryMode>, ConfigError> {
    match raw {
        "off" => Ok(None),
        "auto" => Ok(Some(CodebaseMemoryMode::Auto)),
        "required" => Ok(Some(CodebaseMemoryMode::Required)),
        other => Err(ConfigError::invalid(format!(
            "invalid agent.tools.codebase_memory.mode `{other}` (expected `off`, `auto`, or `required`)"
        ))),
    }
}

fn parse_codebase_memory_index(raw: &str) -> Result<CodebaseMemoryIndex, ConfigError> {
    match raw {
        "off" => Ok(CodebaseMemoryIndex::Off),
        "background" => Ok(CodebaseMemoryIndex::Background),
        "blocking" => Ok(CodebaseMemoryIndex::Blocking),
        other => Err(ConfigError::invalid(format!(
            "invalid agent.tools.codebase_memory.index `{other}` (expected `off`, `background`, or `blocking`)"
        ))),
    }
}

fn resolve_codebase_memory_roles(raw: &[String]) -> Result<Vec<String>, ConfigError> {
    let mut roles = Vec::with_capacity(raw.len());
    for role in raw {
        let role = role.trim();
        if role.is_empty() {
            return Err(ConfigError::invalid(
                "agent.tools.codebase_memory.roles entries must not be empty",
            ));
        }
        roles.push(role.to_string());
    }
    Ok(dedup_strings(roles))
}

fn positive_secs_value(value: u64, field: &str) -> Result<u64, ConfigError> {
    if value == 0 {
        return Err(ConfigError::invalid(format!(
            "{field} must be greater than zero"
        )));
    }
    Ok(value)
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// The roles whose credentials we need to resolve: the engine's roles, any
/// role named only in worker capabilities, plus target-era worker pool roles.
fn referenced_roles(engine: &EngineSettings, worker: &WorkerSettings) -> BTreeSet<String> {
    let mut roles: BTreeSet<String> = engine.roles.iter().cloned().collect();
    for capability in &worker.capabilities {
        roles.insert(capability.role.clone());
    }
    for pool in &worker.pools {
        roles.extend(pool.roles.iter().cloned());
    }
    roles
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn resolve_preferred_config_path(
    preferred_field: &str,
    preferred_raw: Option<&str>,
    legacy_field: &str,
    legacy_raw: Option<&str>,
    env: &impl EnvLookup,
    options: &ResolveOptions,
) -> Result<Option<PathBuf>, ConfigError> {
    let preferred = trimmed(preferred_raw);
    let legacy = trimmed(legacy_raw);

    match (preferred, legacy) {
        (Some(preferred), Some(legacy)) if preferred != legacy => {
            Err(ConfigError::invalid(format!(
                "conflicting config values: `{preferred_field}` and `{legacy_field}` are both set but differ (`{preferred}` vs `{legacy}`); set only one or make them match"
            )))
        }
        (Some(value), _) | (None, Some(value)) => {
            Ok(Some(resolve_config_path(&value, env, options)))
        }
        (None, None) => Ok(None),
    }
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

/// The base `…/temper` state directory, computed from the injected environment
/// (mirrors [`crate::paths::state_dir`], which reads the process environment;
/// kept env-aware here so resolution stays unit-testable).
fn default_state_dir(env: &impl EnvLookup) -> Option<PathBuf> {
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
