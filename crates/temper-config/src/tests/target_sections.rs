// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn target_sections_resolve_relative_to_config_base() {
    let config = parse_config(
        r#"
schema_version = 1
[deployment]
name = "local-dev"
topology = "standalone"
[workflow]
file = "flows/workflow.json"
[paths]
state_dir = "state"
workspace_dir = "workspace"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("target sections resolve");

    assert_eq!(resolved.deployment.name.as_deref(), Some("local-dev"));
    assert_eq!(
        resolved.deployment.topology,
        Some(DeploymentTopology::Standalone)
    );
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.paths.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.paths.state_dir.as_deref(),
        Some(std::path::Path::new("/bundle/state"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/workspace")
    );
    assert_eq!(
        resolved.paths.workspace_dir,
        std::path::Path::new("/bundle/workspace")
    );
}

#[test]
fn target_worker_pools_and_agent_profiles_parse_and_resolve_without_runtime_switch() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["ai/temper"]
roles = ["engineer"]
[worker]
max_concurrent_jobs = 3
[[worker.pools]]
name = "engineers"
roles = ["engineer"]
repos = ["ai/temper"]
max_concurrent_jobs = 2
agent_profile = "coding"
worker_token = "worker-engineers-token"
[agent]
provider = "deepseek"
[agent.profiles.coding]
command = ["temper", "agent"]
provider = "anthropic"
model = "claude-opus-4-8"
investigate_model = "claude-haiku-4-5"
provider_url = "http://fake-llm"
max_iterations = 250
subagents = true
credential = "agent-provider"
"#,
    );

    assert_eq!(config.worker.pools.len(), 1);
    assert_eq!(config.worker.pools[0].name.as_deref(), Some("engineers"));
    assert!(config.agent.profiles.contains_key("coding"));

    let credentials = parse_credentials(
        r#"
schema_version = 1
[secrets]
worker-engineers-token = "worker-secret-value"
agent-provider = "provider-secret-value"
"#,
    );
    let resolved = resolve(&config, &credentials, &NoEnv).expect("resolves");

    // Target metadata is present for inspection/future work.
    let pool = resolved.worker.pools.first().expect("pool resolves");
    assert_eq!(pool.name, "engineers");
    assert_eq!(pool.roles, vec!["engineer"]);
    assert_eq!(pool.repos[0].display(), "ai/temper");
    assert_eq!(pool.max_concurrent_jobs, Some(2));
    assert_eq!(pool.agent_profile.as_deref(), Some("coding"));
    assert_eq!(
        pool.worker_token.as_ref().map(|reference| reference.name.as_str()),
        Some("worker-engineers-token")
    );
    assert_eq!(
        pool.worker_token
            .as_ref()
            .map(|reference| reference.available),
        Some(true)
    );

    let profile = resolved
        .agent
        .profiles
        .get("coding")
        .expect("profile resolves");
    assert_eq!(profile.command, vec!["temper", "agent"]);
    assert_eq!(profile.provider, Some(ProviderKind::Anthropic));
    assert_eq!(profile.model.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(
        profile.investigate_model.as_deref(),
        Some("claude-haiku-4-5")
    );
    assert_eq!(profile.provider_url.as_deref(), Some("http://fake-llm"));
    assert_eq!(profile.max_iterations, Some(250));
    assert_eq!(profile.subagents, Some(true));
    assert_eq!(
        profile
            .credential
            .as_ref()
            .map(|reference| reference.name.as_str()),
        Some("agent-provider")
    );

    // Active runtime fields still come from legacy/default settings, not pools/profiles.
    assert_eq!(resolved.worker.max_concurrent_jobs, 3);
    assert_eq!(resolved.worker.capabilities.len(), 1);
    assert_eq!(resolved.worker.capabilities[0].repo, "ai/temper");
    assert_eq!(resolved.worker.capabilities[0].role, "engineer");
    assert_eq!(resolved.agent.provider.kind, ProviderKind::DeepSeek);
}

#[test]
fn legacy_only_config_has_no_target_metadata_and_preserves_runtime_fields() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["ai/temper"]
roles = ["engineer", "architect"]
[worker]
max_concurrent_jobs = 4
[agent]
provider = "chatgpt"
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");

    assert!(resolved.worker.pools.is_empty());
    assert!(resolved.agent.profiles.is_empty());
    assert_eq!(resolved.worker.max_concurrent_jobs, 4);
    assert_eq!(resolved.worker.capabilities.len(), 2);
    assert!(
        resolved
            .worker
            .capabilities
            .iter()
            .any(|capability| capability.repo == "ai/temper" && capability.role == "engineer")
    );
    assert_eq!(resolved.agent.provider.kind, ProviderKind::ChatGpt);
}

#[test]
fn target_pool_and_profile_validation_errors_name_clear_fields() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "empty pool name",
            r#"
schema_version = 1
[[worker.pools]]
name = " "
roles = ["engineer"]
"#,
            &["worker.pools[0].name"],
        ),
        (
            "duplicate pool name",
            r#"
schema_version = 1
[[worker.pools]]
name = "engineers"
roles = ["engineer"]
[[worker.pools]]
name = " engineers "
roles = ["architect"]
"#,
            &["worker.pools.name", "duplicate", "engineers"],
        ),
        (
            "empty pool roles",
            r#"
schema_version = 1
[[worker.pools]]
name = "engineers"
roles = []
"#,
            &["worker.pools[0].roles"],
        ),
        (
            "invalid pool repo",
            r#"
schema_version = 1
[[worker.pools]]
name = "engineers"
roles = ["engineer"]
repos = ["ai"]
"#,
            &["worker.pools[0].repos[0]", "owner/name"],
        ),
        (
            "zero pool capacity",
            r#"
schema_version = 1
[[worker.pools]]
name = "engineers"
roles = ["engineer"]
max_concurrent_jobs = 0
"#,
            &["worker.pools[0].max_concurrent_jobs"],
        ),
        (
            "missing referenced profile",
            r#"
schema_version = 1
[[worker.pools]]
name = "engineers"
roles = ["engineer"]
agent_profile = "missing"
[agent.profiles.coding]
provider = "anthropic"
"#,
            &["worker.pools[0].agent_profile", "missing"],
        ),
        (
            "empty profile name",
            r#"
schema_version = 1
[agent.profiles.""]
provider = "anthropic"
"#,
            &["agent.profiles", "name"],
        ),
        (
            "duplicate trimmed profile name",
            r#"
schema_version = 1
[agent.profiles.coding]
provider = "anthropic"
[agent.profiles." coding "]
provider = "deepseek"
"#,
            &["agent.profiles", "duplicate", "coding"],
        ),
        (
            "empty profile provider",
            r#"
schema_version = 1
[agent.profiles.coding]
provider = " "
"#,
            &["agent.profiles.coding.provider"],
        ),
        (
            "invalid profile provider",
            r#"
schema_version = 1
[agent.profiles.coding]
provider = "bogus"
"#,
            &["agent.profiles.coding.provider", "bogus"],
        ),
        (
            "zero profile max iterations",
            r#"
schema_version = 1
[agent.profiles.coding]
max_iterations = 0
"#,
            &["agent.profiles.coding.max_iterations"],
        ),
    ];

    for (name, toml, expected) in cases {
        let config = parse_config(toml);
        let err = resolve(&config, &Credentials::default(), &NoEnv)
            .expect_err(&format!("{name}: expected invalid config"));
        let message = err.to_string();
        for needle in *expected {
            assert!(
                message.contains(needle),
                "{name}: expected `{needle}` in `{message}`"
            );
        }
    }
}

#[test]
fn target_state_dir_controls_default_workspace_root() {
    let config = parse_config(
        r#"
schema_version = 1
[paths]
state_dir = "state"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("state dir resolves");

    assert_eq!(
        resolved.paths.state_dir.as_deref(),
        Some(std::path::Path::new("/bundle/state"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/state/workspace")
    );
    assert_eq!(
        resolved.paths.workspace_dir,
        std::path::Path::new("/bundle/state/workspace")
    );
}

#[test]
fn legacy_workflow_and_workspace_remain_supported() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
workflow = "flows/workflow.json"
[worker]
workspace = "workspace"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("legacy path fields resolve");

    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/workspace")
    );
}

#[test]
fn matching_target_and_legacy_paths_are_accepted() {
    let config = parse_config(
        r#"
schema_version = 1
[workflow]
file = "flows/workflow.json"
[paths]
workspace_dir = "workspace"
[engine]
workflow = "flows/workflow.json"
[worker]
workspace = "workspace"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("matching target and legacy values resolve");

    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/workspace")
    );
}

#[test]
fn conflicting_target_and_legacy_workflow_is_invalid() {
    let config = parse_config(
        r#"
schema_version = 1
[workflow]
file = "target-workflow.json"
[engine]
workflow = "legacy-workflow.json"
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("conflicting workflow fields are rejected");
    let message = err.to_string();
    assert!(message.contains("workflow.file"), "{message}");
    assert!(message.contains("engine.workflow"), "{message}");
    assert!(message.contains("conflicting"), "{message}");
}

#[test]
fn conflicting_target_and_legacy_workspace_is_invalid() {
    let config = parse_config(
        r#"
schema_version = 1
[paths]
workspace_dir = "target-workspace"
[worker]
workspace = "legacy-workspace"
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("conflicting workspace fields are rejected");
    let message = err.to_string();
    assert!(message.contains("paths.workspace_dir"), "{message}");
    assert!(message.contains("worker.workspace"), "{message}");
    assert!(message.contains("conflicting"), "{message}");
}

#[test]
fn invalid_deployment_topology_is_rejected() {
    let config = parse_config(
        r#"
schema_version = 1
[deployment]
topology = "clustered"
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("invalid topology is rejected");
    assert!(
        err.to_string().contains("deployment.topology"),
        "error should name the invalid field: {err}"
    );
}
