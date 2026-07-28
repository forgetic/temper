use std::collections::BTreeMap;

use super::*;
use temper_config::{
    AgentConfig as FileAgentConfig, AgentDeadlineConfig, AgentProfileConfig, AgentProviderConfig,
    AgentToolsConfig, CodebaseMemoryToolConfig, Config, Credentials, EngineConfig, ModelMap,
    NamedSecret, NamedSecretEntry, NoEnv, ProviderCredentialFile, WorkerFileConfig,
    WorkerPoolConfig, resolve,
};
use temper_worker::{CodebaseMemoryIndex, CodebaseMemoryMode};

#[test]
fn worker_config_projects_liveness_and_creates_private_result_root() {
    let mut resolved = resolved_with_codebase_memory(None);
    let temp = tempfile::tempdir().expect("tempdir");
    resolved.worker.result_root = temp.path().join("results");
    resolved.worker.liveness_limits.max_no_progress = std::time::Duration::from_secs(44);
    resolved.worker.liveness_limits.max_run = Some(std::time::Duration::from_secs(99));

    let config = worker_config(&resolved).expect("worker config");
    assert_eq!(
        config.liveness_limits.max_no_progress,
        std::time::Duration::from_secs(44)
    );
    assert_eq!(
        config.liveness_limits.max_run,
        Some(std::time::Duration::from_secs(99))
    );
    assert_eq!(config.result_root, resolved.worker.result_root);
    assert!(config.result_root.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&config.result_root)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
}

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
    assert_eq!(invocation.supervision, AgentSupervisionKind::FirstParty);
    assert_eq!(
        invocation.runtime_limits,
        Some(temper_worker::AgentRuntimeLimitsV1 {
            tool_timeout_secs: 321,
            model_connect_timeout_secs: 120,
            model_idle_timeout_secs: 22,
            ..temper_worker::AgentRuntimeLimitsV1::default()
        })
    );
}

#[test]
fn first_party_program_override_preserves_profile_invocation_settings() {
    let mut resolved = resolved_with_profile_pool();
    resolved.worker.selected_pool = Some("engineers".to_string());

    let configured =
        agent_invocation(&resolved, &["default-agent".to_string()]).expect("configured invocation");
    let supplied_program = vec!["/tmp/benchmark-agent".to_string()];
    let overridden = agent_invocation_with_first_party_program(&resolved, &supplied_program)
        .expect("overridden invocation");

    let expected_command: Vec<_> = supplied_program
        .iter()
        .cloned()
        .chain(configured.command[2..].iter().cloned())
        .collect();
    assert_eq!(overridden.command, expected_command);
    assert_eq!(overridden.env, configured.env);
    assert_eq!(overridden.tool_config, configured.tool_config);
    assert_eq!(overridden.supervision, configured.supervision);
    assert_eq!(overridden.runtime_limits, configured.runtime_limits);
    assert_eq!(overridden.trace_policy, configured.trace_policy);
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
    assert_eq!(invocation.supervision, AgentSupervisionKind::ThirdParty);
    assert!(invocation.runtime_limits.is_none());

    let overridden =
        agent_invocation_with_first_party_program(&resolved, &["benchmark-agent".to_string()])
            .expect("third-party invocation remains classifiable");
    assert_eq!(
        overridden.command.first().map(String::as_str),
        Some("vendor-agent")
    );
    assert_eq!(overridden.supervision, AgentSupervisionKind::ThirdParty);
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
                    deadlines: AgentDeadlineConfig {
                        tool_timeout_secs: Some(321),
                        model_connect_timeout_secs: None,
                        model_idle_timeout_secs: Some(22),
                        ..AgentDeadlineConfig::default()
                    },
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
