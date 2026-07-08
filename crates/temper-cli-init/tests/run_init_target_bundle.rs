// SPDX-License-Identifier: MPL-2.0

use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_cli_init::{InitOptions, InitOverrides, InitTopology, RepoSelection, run_init};
use temper_config::DeploymentTopology;

mod support;
use support::{StubProvisioner, resolve_generated_bundle_non_strict};

#[test]
fn non_interactive_distributed_provider_none_writes_target_bundle_without_provider_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path.clone()),
            credentials: Some(credentials_path.clone()),
        },
        topology: InitTopology::Distributed,
        non_interactive: true,
        overrides: InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            admin_user: Some("root".to_string()),
            admin_password: Some("admin-pass".to_string()),
            provider: Some("none".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut provisioner = StubProvisioner { seen: None };

    run_init(&mut prompter, &mut provisioner, &opts).expect("provider-none init succeeds");

    assert!(provisioner.seen.is_none());
    let config = std::fs::read_to_string(&config_path).expect("config");
    assert!(config.contains("topology = \"distributed\""), "{config}");
    assert!(config.contains("name = \"default\""), "{config}");
    assert!(!config.contains("[agent.providers."), "{config}");
    assert!(!config.contains("[agent.profiles."), "{config}");
    let creds = std::fs::read_to_string(&credentials_path).expect("credentials");
    assert!(!creds.contains("[agent.providers."), "{creds}");
    assert!(!creds.contains("agent-provider"), "{creds}");
    assert!(creds.contains("[secrets.webhook-secret]"), "{creds}");
    assert!(creds.contains("[secrets.worker-default-token]"), "{creds}");

    let resolved = resolve_generated_bundle_non_strict(&config_path, &credentials_path);
    assert_eq!(
        resolved.deployment.topology,
        Some(DeploymentTopology::Distributed)
    );
    let pool = resolved.worker.pools.first().expect("pool resolves");
    assert_eq!(pool.name, "default");
    assert_eq!(pool.agent_profile, None);
}

#[test]
fn non_interactive_repeated_repos_are_written_to_engine_and_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path.clone()),
            credentials: Some(credentials_path.clone()),
        },
        non_interactive: true,
        overrides: InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            repos: vec![
                RepoSelection {
                    owner: "acme".to_string(),
                    name: "service".to_string(),
                },
                RepoSelection {
                    owner: "acme".to_string(),
                    name: "docs".to_string(),
                },
            ],
            admin_user: Some("root".to_string()),
            admin_password: Some("admin-pass".to_string()),
            provider: Some("none".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut provisioner = StubProvisioner { seen: None };

    run_init(&mut prompter, &mut provisioner, &opts).expect("multi-repo local init succeeds");

    let resolved = resolve_generated_bundle_non_strict(&config_path, &credentials_path);
    let repos: Vec<_> = resolved
        .engine
        .repos
        .iter()
        .map(|repo| repo.display())
        .collect();
    assert_eq!(repos, vec!["acme/service", "acme/docs"]);
    let pool_repos: Vec<_> = resolved.worker.pools[0]
        .repos
        .iter()
        .map(|repo| repo.display())
        .collect();
    assert_eq!(pool_repos, vec!["acme/service", "acme/docs"]);
}
