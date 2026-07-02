// SPDX-License-Identifier: MPL-2.0

//! Adapters from a [`Resolved`] config into the worker tier's runtime types and
//! the out-of-process agent invocation (command + injected environment).

use std::collections::BTreeMap;

use temper_config::provider;
use temper_config::{ExposeSecret, Resolved};
use temper_worker::config::{CapabilitySpec, ExecutorSelection, WorkerConfig};
use temper_worker::workspace::RoleGitIdentity;
use temper_worker::{
    AgentToolConfig, CodebaseMemoryIndex as ProtocolCodebaseMemoryIndex,
    CodebaseMemoryMode as ProtocolCodebaseMemoryMode,
    CodebaseMemoryToolConfig as ProtocolCodebaseMemoryToolConfig,
};

/// Builds the worker runtime config. The `executor` field is a placeholder
/// (`Stub`): [`run_worker`](temper_worker::run_worker) takes the real executor
/// separately and only reads identity/cadence fields from this config.
pub fn worker_config(resolved: &Resolved) -> Result<WorkerConfig, String> {
    let worker = &resolved.worker;
    if worker.worker_id.trim().is_empty() {
        return Err("worker identity must not be empty".to_string());
    }
    if let Some(pool) = worker.selected_pool.as_deref() {
        if pool.trim().is_empty() {
            return Err("selected worker pool name must not be empty".to_string());
        }
    }
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
        worker_pool: worker.selected_pool.clone(),
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
/// before spawning the agent; the worker owns the final branch push, so no push
/// token crosses the agent boundary.
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
    pub tool_config: Option<AgentToolConfig>,
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
    if let Some(capture_dir) = &agent.config_dir {
        command.push("--capture-dir".to_string());
        command.push(capture_dir.display().to_string());
    }

    let mut env = Vec::new();
    if let Some(json) = provider::provider_credentials_json(&agent.provider) {
        // I/O boundary: the one secret crosses into the spawned agent's env.
        env.push((provider::PROVIDER_CREDENTIALS_ENV.to_string(), json));
    }

    Ok(AgentInvocation {
        command,
        env,
        tool_config: agent_tool_config(resolved),
    })
}

/// Converts resolved non-secret agent tool settings into the worker→agent JSON
/// protocol shape. Returns `None` when no tool section is enabled.
pub fn agent_tool_config(resolved: &Resolved) -> Option<AgentToolConfig> {
    let codebase_memory = resolved.agent.tools.codebase_memory.as_ref().map(|tool| {
        ProtocolCodebaseMemoryToolConfig {
            mode: match tool.mode {
                temper_config::CodebaseMemoryMode::Auto => ProtocolCodebaseMemoryMode::Auto,
                temper_config::CodebaseMemoryMode::Required => ProtocolCodebaseMemoryMode::Required,
            },
            command: tool.command.clone(),
            args: tool.args.clone(),
            roles: tool.roles.clone(),
            index: match tool.index {
                temper_config::CodebaseMemoryIndex::Off => ProtocolCodebaseMemoryIndex::Off,
                temper_config::CodebaseMemoryIndex::Background => {
                    ProtocolCodebaseMemoryIndex::Background
                }
                temper_config::CodebaseMemoryIndex::Blocking => {
                    ProtocolCodebaseMemoryIndex::Blocking
                }
            },
            startup_timeout_secs: tool.startup_timeout_secs,
            index_timeout_secs: tool.index_timeout_secs,
        }
    });

    codebase_memory.map(|codebase_memory| AgentToolConfig {
        codebase_memory: Some(codebase_memory),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_config::{
        AgentConfig as FileAgentConfig, AgentToolsConfig, CodebaseMemoryToolConfig, Config,
        Credentials, EngineConfig, NoEnv, resolve,
    };
    use temper_worker::{CodebaseMemoryIndex, CodebaseMemoryMode};

    #[test]
    fn agent_invocation_carries_resolved_tool_config_when_enabled() {
        let resolved = resolved_with_codebase_memory(Some(CodebaseMemoryToolConfig {
            mode: Some("required".to_string()),
            command: Some(" codebase-memory-mcp ".to_string()),
            args: Some(vec![" --cache ".to_string(), "local".to_string()]),
            roles: Some(vec![" engineer ".to_string()]),
            index: Some("blocking".to_string()),
            startup_timeout_secs: Some(7),
            index_timeout_secs: Some(90),
        }));

        let invocation =
            agent_invocation(&resolved, &["temper-agent".to_string()]).expect("invocation builds");
        let tool_config = invocation.tool_config.expect("tool config present");
        let codebase_memory = tool_config.codebase_memory.expect("codebase memory config");
        assert_eq!(codebase_memory.mode, CodebaseMemoryMode::Required);
        assert_eq!(codebase_memory.command, "codebase-memory-mcp");
        assert_eq!(codebase_memory.args, vec!["--cache", "local"]);
        assert_eq!(codebase_memory.roles, vec!["engineer"]);
        assert_eq!(codebase_memory.index, CodebaseMemoryIndex::Blocking);
        assert_eq!(codebase_memory.startup_timeout_secs, 7);
        assert_eq!(codebase_memory.index_timeout_secs, 90);
    }

    #[test]
    fn agent_invocation_omits_tool_config_when_absent_or_off() {
        let absent = resolved_with_codebase_memory(None);
        assert!(agent_tool_config(&absent).is_none());
        assert!(
            agent_invocation(&absent, &["temper-agent".to_string()])
                .expect("invocation builds")
                .tool_config
                .is_none()
        );

        let off = resolved_with_codebase_memory(Some(CodebaseMemoryToolConfig {
            mode: Some("off".to_string()),
            ..Default::default()
        }));
        assert!(agent_tool_config(&off).is_none());
    }

    fn resolved_with_codebase_memory(tool: Option<CodebaseMemoryToolConfig>) -> Resolved {
        let config = Config {
            engine: EngineConfig {
                repos: Some(vec!["acme/widgets".to_string()]),
                roles: Some(vec!["engineer".to_string()]),
                ..Default::default()
            },
            agent: FileAgentConfig {
                tools: AgentToolsConfig {
                    codebase_memory: tool,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        resolve(&config, &Credentials::default(), &NoEnv).expect("config resolves")
    }
}
