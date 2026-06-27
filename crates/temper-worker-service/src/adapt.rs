// SPDX-License-Identifier: MPL-2.0

//! Adapters from a [`Resolved`] config into the worker tier's runtime types and
//! the out-of-process agent invocation (command + injected environment).

use std::collections::BTreeMap;

use temper_config::provider;
use temper_config::{ExposeSecret, Resolved};
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
        role_identities: role_identities(resolved),
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

/// Role → git identity for the coding executor. The executor configures each
/// writable checkout's local `.git/config` (author identity + push credential)
/// before spawning the agent, so the agent's checkpoint commits/pushes use the
/// right identity without the push token ever crossing the agent boundary.
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

/// Assembles the agent command (the given program prefix plus the non-secret
/// provider/model/loop flags) and the one secret env var (the provider
/// credential JSON).
///
/// The context/result/workspace paths are set per-job by the
/// `OutOfProcessRunner` (it owns the temp files + the prepared checkout cwd), so
/// they are not added here. The git author identity + push credential are
/// configured by the coding executor in each writable checkout's local
/// `.git/config`, so no `TEMPER_FORGEJO_*` env crosses the boundary anymore.
pub fn agent_invocation(
    resolved: &Resolved,
    program: &[String],
) -> Result<AgentInvocation, String> {
    let agent = &resolved.agent;

    let mut command: Vec<String> = program.to_vec();
    command.extend(provider::provider_flags(&agent.provider));
    command.push("--max-iterations".to_string());
    command.push(agent.max_iterations.to_string());
    command.push("--subagents".to_string());
    command.push(if agent.enable_subagents { "on" } else { "off" }.to_string());
    if agent.enable_checkpoints {
        command.push("--checkpoints".to_string());
        command.push("on".to_string());
    }
    if let Some(capture_dir) = &agent.config_dir {
        command.push("--capture-dir".to_string());
        command.push(capture_dir.display().to_string());
    }

    let mut env = Vec::new();
    if let Some(json) = provider::provider_credentials_json(&agent.provider) {
        // I/O boundary: the one secret crosses into the spawned agent's env.
        env.push((provider::PROVIDER_CREDENTIALS_ENV.to_string(), json));
    }

    Ok(AgentInvocation { command, env })
}
