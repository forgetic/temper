// SPDX-License-Identifier: MPL-2.0

//! Adapters from a [`Resolved`] config into the worker tier's runtime types and
//! the out-of-process agent invocation (command + injected environment).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use temper_config::provider;
use temper_config::{ExposeSecret, Resolved, env_role_key};
use temper_worker::config::{CapabilitySpec, ExecutorSelection, WorkerConfig};
use temper_worker::workspace::RoleGitIdentity;

/// Builds the worker runtime config. The `executor` field is a placeholder
/// (`Stub`): [`run_worker`](temper_worker::run_worker) takes the real executor
/// separately and only reads identity/cadence fields from this config.
pub fn worker_config(resolved: &Resolved) -> Result<WorkerConfig, String> {
    let worker = &resolved.worker;
    if worker.capabilities.is_empty() {
        return Err("no worker capabilities; set `[engine] repos`/`roles` (or \
                    `[worker] capabilities`)"
            .to_string());
    }
    let capabilities = worker
        .capabilities
        .iter()
        .map(|capability| CapabilitySpec {
            repo: capability.repo.clone(),
            role: capability.role.clone(),
        })
        .collect();
    Ok(WorkerConfig {
        daemon_url: worker.daemon_url.clone(),
        worker_id: worker.worker_id.clone(),
        capabilities,
        max_concurrent_jobs: worker.max_concurrent_jobs,
        poll_wait: worker.poll_wait,
        heartbeat_interval: worker.heartbeat_interval,
        executor: ExecutorSelection::Stub,
    })
}

/// The git base URL the agent pushes to: `[worker] git_base_url`, else the forge
/// URL.
pub fn git_base_url(resolved: &Resolved) -> Result<String, String> {
    if let Some(url) = &resolved.worker.git_base_url {
        return Ok(url.clone());
    }
    resolved
        .forge
        .require_url()
        .map(str::to_string)
        .map_err(|error| error.to_string())
}

/// Role → git identity for the coding executor (commit/push) and the agent's
/// per-role checkpoint identity.
pub fn role_identities(resolved: &Resolved) -> BTreeMap<String, RoleGitIdentity> {
    resolved
        .forge
        .role_identities
        .iter()
        .map(|(role, identity)| {
            (
                role.clone(),
                RoleGitIdentity {
                    user: identity.user.clone(),
                    email: identity.email.clone(),
                    // I/O boundary: the worker's git identity carries the raw
                    // push token (used to build the git auth header).
                    token: identity.token.expose_secret().to_string(),
                },
            )
        })
        .collect()
}

/// The agent invocation: the spawn command and the environment injected into it.
pub struct AgentInvocation {
    pub command: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Assembles the agent command (the given program prefix plus mode/limit flags)
/// and the environment to inject: every role's git identity plus the provider
/// wiring. OAuth tokens are materialized into a pi-format `auth.json` under
/// `auth_dir`, which the agent reads (and refreshes) via the injected
/// `TEMPER_AGENTS_AUTH_FILE`.
pub fn agent_invocation(
    resolved: &Resolved,
    program: &[String],
    auth_dir: &Path,
) -> Result<AgentInvocation, String> {
    let agent = &resolved.agent;

    let mut command: Vec<String> = program.to_vec();
    command.push("--auth".to_string());
    command.push(provider::auth_mode(agent.provider.kind).to_string());
    command.push("--max-iterations".to_string());
    command.push(agent.max_iterations.to_string());
    if let Some(config_dir) = &agent.config_dir {
        command.push("--config-dir".to_string());
        command.push(config_dir.display().to_string());
    }
    if agent.enable_subagents {
        command.push("--enable-subagents".to_string());
    }

    let auth_file = materialize_auth_file(resolved, auth_dir)?;
    let mut env = role_identity_env(resolved);
    env.extend(provider::provider_env(
        &agent.provider,
        auth_file.as_deref(),
    ));

    Ok(AgentInvocation { command, env })
}

/// Every role's git identity as `TEMPER_FORGEJO_{USER,EMAIL,TOKEN}_<ROLE>` so
/// the spawned agent's checkpoint commits use the right identity for its job's
/// role.
fn role_identity_env(resolved: &Resolved) -> Vec<(String, String)> {
    let mut env = Vec::new();
    for (role, identity) in &resolved.forge.role_identities {
        let key = env_role_key(role);
        env.push((format!("TEMPER_FORGEJO_USER_{key}"), identity.user.clone()));
        env.push((
            format!("TEMPER_FORGEJO_EMAIL_{key}"),
            identity.email.clone(),
        ));
        env.push((
            format!("TEMPER_FORGEJO_TOKEN_{key}"),
            // I/O boundary: the token crosses into the spawned agent's env.
            identity.token.expose_secret().to_string(),
        ));
    }
    env
}

/// Materializes the OAuth `auth.json` (for inline tokens) or returns the
/// configured external auth-file path, or `None` for api-key/ambient creds.
fn materialize_auth_file(resolved: &Resolved, auth_dir: &Path) -> Result<Option<PathBuf>, String> {
    provider::materialize_auth_file(&resolved.agent.provider, auth_dir).map_err(|error| {
        format!(
            "materialize agent auth file in {}: {error}",
            auth_dir.display()
        )
    })
}
