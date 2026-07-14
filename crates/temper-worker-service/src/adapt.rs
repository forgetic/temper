// SPDX-License-Identifier: MPL-2.0

//! Adapters from a [`Resolved`] config into the worker tier's runtime types and
//! the out-of-process agent invocation (command + injected environment).

use std::collections::BTreeMap;

use temper_config::provider;
use temper_config::{
    AgentProfileSettings, ExposeSecret, ProviderKind, Resolved, WorkerPoolSettings, WorkerSettings,
};
use temper_worker::config::{CapabilitySpec, ExecutorSelection, WorkerConfig};
use temper_worker::workspace::RoleGitIdentity;
use temper_worker::{
    AgentToolConfig, CodebaseMemoryIndex as ProtocolCodebaseMemoryIndex,
    CodebaseMemoryMode as ProtocolCodebaseMemoryMode,
    CodebaseMemoryToolConfig as ProtocolCodebaseMemoryToolConfig, WorkerAgentTraceConfig,
    WorkerAuth,
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
        worker_auth: selected_worker_auth(worker)?,
        capabilities,
        role_identities: role_identities(resolved),
        max_concurrent_jobs: worker.max_concurrent_jobs,
        poll_wait: worker.poll_wait,
        heartbeat_interval: worker.heartbeat_interval,
        agent_traces: worker_agent_trace_config(resolved),
        executor: ExecutorSelection::Stub,
    })
}

/// Projects resolved trace policy and the durable spool root into the worker
/// subsystem config. Missing durable state produces an effective `off` policy;
/// the service runtime reports that degradation without failing product work.
pub fn worker_agent_trace_config(resolved: &Resolved) -> WorkerAgentTraceConfig {
    let traces = &resolved.observability.agent_traces;
    WorkerAgentTraceConfig {
        policy: traces.policy_for_storage(traces.worker_spool_root.as_deref()),
        spool_root: traces.worker_spool_root.clone(),
    }
}

/// The selected worker pool's bearer credential, if its policy declares a
/// `worker_token` secret reference.
pub fn selected_worker_auth(worker: &WorkerSettings) -> Result<Option<WorkerAuth>, String> {
    let Some(selected_pool) = worker.selected_pool.as_deref() else {
        return Ok(None);
    };
    let pool = worker
        .pools
        .iter()
        .find(|pool| pool.name == selected_pool)
        .ok_or_else(|| format!("selected worker pool `{selected_pool}` is not configured"))?;
    let Some(reference) = pool.worker_token.as_ref() else {
        return Ok(None);
    };
    let token = worker.worker_pool_tokens.get(selected_pool).ok_or_else(|| {
        format!(
            "worker pool `{}` worker_token references secret `{}` but it has no non-empty text value",
            pool.name, reference.name
        )
    })?;
    Ok(Some(WorkerAuth::bearer(token.expose_secret().to_string())))
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

type AgentCommandEnv = (Vec<String>, Vec<(String, String)>);

/// The agent invocation: the spawn command and the environment injected into it.
pub struct AgentInvocation {
    pub command: Vec<String>,
    pub env: Vec<(String, String)>,
    pub tool_config: Option<AgentToolConfig>,
    /// Effective shared capture policy for a known first-party agent command.
    /// `None` keeps explicit third-party profile commands flag-compatible.
    pub trace_policy: Option<temper_config::AgentActivityCapturePolicyV1>,
}

/// Assembles the agent command and the environment injected into it.
///
/// If runtime shaping selected a worker pool that names `agent_profile`, the
/// command/provider/model/loop flags and credential env come from that profile.
/// Pools without `agent_profile` and legacy workers keep the historical active
/// `[agent]` provider behavior.
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
    let (command, env, first_party) =
        if let Some((pool, profile)) = selected_agent_profile(resolved)? {
            let first_party =
                profile.command.is_empty() || is_first_party_agent_command(&profile.command);
            let (command, env) = profile_agent_command_and_env(pool, profile, program)?;
            (command, env, first_party)
        } else {
            let (command, env) = legacy_agent_command_and_env(resolved, program);
            (command, env, true)
        };

    Ok(AgentInvocation {
        command,
        env,
        tool_config: agent_tool_config(resolved),
        trace_policy: first_party.then(|| {
            let traces = &resolved.observability.agent_traces;
            traces.policy_for_storage(traces.worker_spool_root.as_deref())
        }),
    })
}

fn legacy_agent_command_and_env(resolved: &Resolved, program: &[String]) -> AgentCommandEnv {
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

    (command, env)
}

fn selected_agent_profile(
    resolved: &Resolved,
) -> Result<Option<(&WorkerPoolSettings, &AgentProfileSettings)>, String> {
    let Some(pool_name) = resolved.worker.selected_pool.as_deref() else {
        return Ok(None);
    };
    let pool = resolved
        .worker
        .pools
        .iter()
        .find(|pool| pool.name == pool_name)
        .ok_or_else(|| format!("selected worker pool `{pool_name}` is not configured"))?;
    let Some(profile_name) = pool.agent_profile.as_deref() else {
        return Ok(None);
    };
    let profile = resolved.agent.profiles.get(profile_name).ok_or_else(|| {
        format!(
            "worker pool `{}` references missing agent profile `{profile_name}`",
            pool.name
        )
    })?;
    Ok(Some((pool, profile)))
}

fn profile_agent_command_and_env(
    pool: &WorkerPoolSettings,
    profile: &AgentProfileSettings,
    program: &[String],
) -> Result<AgentCommandEnv, String> {
    let mut command = if profile.command.is_empty() {
        program.to_vec()
    } else {
        profile.command.clone()
    };
    if command.is_empty() {
        return Err(format!(
            "worker pool `{}` selected an agent profile with an empty command and no default agent program was supplied",
            pool.name
        ));
    }

    if let Some(provider) = profile.provider {
        command.push("--provider".to_string());
        command.push(provider_flag(provider).to_string());
    }
    if let Some(model) = &profile.model {
        command.push("--model".to_string());
        command.push(model.clone());
    }
    if let Some(model) = &profile.investigate_model {
        command.push("--investigate-model".to_string());
        command.push(model.clone());
    }
    if let Some(url) = &profile.provider_url {
        command.push("--provider-url".to_string());
        command.push(url.clone());
    }
    if let Some(max_iterations) = profile.max_iterations {
        command.push("--max-iterations".to_string());
        command.push(max_iterations.to_string());
    }
    if let Some(subagents) = profile.subagents {
        command.push("--subagents".to_string());
        command.push(if subagents { "on" } else { "off" }.to_string());
    }

    let mut env = Vec::new();
    if let Some(json) = &profile.credential_json {
        // I/O boundary: profile credentials are resolved from the selected
        // secret source and cross into the spawned agent only as the one
        // provider-credentials env var, never as argv.
        env.push((
            provider::PROVIDER_CREDENTIALS_ENV.to_string(),
            json.expose_secret().to_string(),
        ));
    }
    Ok((command, env))
}

fn provider_flag(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::DeepSeek => "deepseek",
        ProviderKind::ChatGpt => "chatgpt",
        ProviderKind::Anthropic => "anthropic",
    }
}

fn is_first_party_agent_command(command: &[String]) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let executable = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    executable == "temper-agent"
        || (executable == "temper" && command.get(1).is_some_and(|arg| arg == "agent"))
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
    use std::collections::BTreeMap;

    use super::*;
    use temper_config::{
        AgentConfig as FileAgentConfig, AgentProfileConfig, AgentProviderConfig, AgentToolsConfig,
        CodebaseMemoryToolConfig, Config, Credentials, EngineConfig, ModelMap, NamedSecret,
        NamedSecretEntry, NoEnv, ProviderCredentialFile, WorkerFileConfig, WorkerPoolConfig,
        resolve,
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

    #[test]
    fn selected_pool_agent_profile_controls_command_and_env() {
        let mut resolved = resolved_with_profile_pool();
        resolved.worker.selected_pool = Some("engineers".to_string());

        let invocation =
            agent_invocation(&resolved, &["default-agent".to_string()]).expect("invocation");

        assert_eq!(
            invocation.command,
            vec![
                "temper",
                "agent",
                "--provider",
                "anthropic",
                "--model",
                "claude-opus-profile",
                "--investigate-model",
                "claude-haiku-profile",
                "--provider-url",
                "http://profile-llm",
                "--max-iterations",
                "123",
                "--subagents",
                "on",
            ]
        );
        assert_eq!(invocation.env.len(), 1);
        assert_eq!(invocation.env[0].0, provider::PROVIDER_CREDENTIALS_ENV);
        assert!(
            invocation.env[0].1.contains("sk-profile"),
            "profile credential JSON missing secret payload"
        );
        assert!(
            !invocation
                .command
                .iter()
                .any(|part| part.contains("sk-profile")),
            "profile credential must not be passed on argv"
        );
        assert!(
            invocation.trace_policy.is_some(),
            "explicit first-party profile commands receive capture policy"
        );
    }

    #[test]
    fn explicit_third_party_profile_command_omits_first_party_trace_flag() {
        let mut resolved = resolved_with_profile_pool();
        resolved.worker.selected_pool = Some("engineers".to_string());
        resolved
            .agent
            .profiles
            .get_mut("profiled")
            .expect("profiled profile")
            .command = vec!["vendor-agent".to_string()];

        let invocation =
            agent_invocation(&resolved, &["default-agent".to_string()]).expect("invocation");

        assert_eq!(
            invocation.command.first().map(String::as_str),
            Some("vendor-agent")
        );
        assert!(invocation.trace_policy.is_none());
    }

    #[test]
    fn pool_without_agent_profile_uses_legacy_provider_fallback() {
        let mut resolved = resolved_with_profile_pool();
        resolved.worker.selected_pool = Some("legacy".to_string());

        let invocation =
            agent_invocation(&resolved, &["temper-agent".to_string()]).expect("invocation");

        assert_eq!(
            invocation.command,
            vec![
                "temper-agent",
                "--provider",
                "deepseek",
                "--model",
                "deepseek-main",
                "--investigate-model",
                "deepseek-investigate",
                "--provider-url",
                "http://legacy-llm",
                "--max-iterations",
                "77",
                "--subagents",
                "off",
                "--capture-dir",
                "/legacy-capture",
            ]
        );
        assert_eq!(invocation.env.len(), 1);
        assert_eq!(invocation.env[0].0, provider::PROVIDER_CREDENTIALS_ENV);
        assert!(invocation.env[0].1.contains("sk-legacy"));
        assert!(invocation.trace_policy.is_some());
        assert!(
            !invocation
                .command
                .iter()
                .any(|part| part.contains("sk-legacy")),
            "legacy credential must not be passed on argv"
        );
    }

    fn resolved_with_profile_pool() -> Resolved {
        let config = Config {
            engine: EngineConfig {
                repos: Some(vec!["acme/widgets".to_string()]),
                roles: Some(vec!["engineer".to_string()]),
                ..Default::default()
            },
            worker: WorkerFileConfig {
                pools: vec![
                    WorkerPoolConfig {
                        name: Some("engineers".to_string()),
                        roles: Some(vec!["engineer".to_string()]),
                        repos: Some(vec!["acme/widgets".to_string()]),
                        max_concurrent_jobs: Some(1),
                        agent_profile: Some("profiled".to_string()),
                        ..Default::default()
                    },
                    WorkerPoolConfig {
                        name: Some("legacy".to_string()),
                        roles: Some(vec!["engineer".to_string()]),
                        repos: Some(vec!["acme/widgets".to_string()]),
                        max_concurrent_jobs: Some(1),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            agent: FileAgentConfig {
                provider: Some("deepseek".to_string()),
                max_iterations: Some(77),
                enable_subagents: Some(false),
                config_dir: Some("/legacy-capture".to_string()),
                providers: BTreeMap::from([(
                    "deepseek".to_string(),
                    AgentProviderConfig {
                        url: Some("http://legacy-llm".to_string()),
                        models: Some(ModelMap {
                            main: Some("deepseek-main".to_string()),
                            investigate: Some("deepseek-investigate".to_string()),
                        }),
                    },
                )]),
                profiles: BTreeMap::from([(
                    "profiled".to_string(),
                    AgentProfileConfig {
                        command: Some(vec!["temper".to_string(), "agent".to_string()]),
                        provider: Some("anthropic".to_string()),
                        model: Some("claude-opus-profile".to_string()),
                        investigate_model: Some("claude-haiku-profile".to_string()),
                        provider_url: Some("http://profile-llm".to_string()),
                        max_iterations: Some(123),
                        subagents: Some(true),
                        credential: Some("profile-secret".to_string()),
                    },
                )]),
                ..Default::default()
            },
            ..Default::default()
        };
        let credentials = Credentials {
            agent: temper_config::AgentCredentials {
                providers: BTreeMap::from([(
                    "deepseek".to_string(),
                    ProviderCredentialFile {
                        kind: Some("api-key".to_string()),
                        key: Some("sk-legacy".to_string()),
                        ..Default::default()
                    },
                )]),
            },
            secrets: BTreeMap::from([(
                "profile-secret".to_string(),
                NamedSecret::Structured(NamedSecretEntry {
                    kind: Some("provider-credentials".to_string()),
                    provider: Some("anthropic".to_string()),
                    auth: Some("api-key".to_string()),
                    api_key: Some("sk-profile".to_string()),
                    ..Default::default()
                }),
            )]),
            ..Default::default()
        };
        resolve(&config, &credentials, &NoEnv).expect("config resolves")
    }

    #[test]
    fn worker_config_selected_pool_requires_non_empty_worker_token() {
        let config = Config {
            engine: EngineConfig {
                repos: Some(vec!["acme/widgets".to_string()]),
                roles: Some(vec!["engineer".to_string()]),
                ..Default::default()
            },
            worker: WorkerFileConfig {
                pools: vec![WorkerPoolConfig {
                    name: Some("builders".to_string()),
                    roles: Some(vec!["engineer".to_string()]),
                    repos: Some(vec!["acme/widgets".to_string()]),
                    max_concurrent_jobs: Some(1),
                    worker_token: Some("pool-token".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut credentials = Credentials::default();
        credentials
            .secrets
            .insert("pool-token".to_string(), NamedSecret::Raw(" ".to_string()));
        let mut resolved = resolve(&config, &credentials, &NoEnv).expect("config resolves");
        resolved.worker.selected_pool = Some("builders".to_string());

        let error = worker_config(&resolved).expect_err("empty pool token should fail");
        assert!(error.contains("builders"), "{error}");
        assert!(error.contains("pool-token"), "{error}");
        assert!(error.contains("no non-empty text value"), "{error}");
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
