// SPDX-License-Identifier: MPL-2.0

use std::{process::ExitCode, sync::Arc};

use temper_daemon::{
    config::{role_tokens_from_env, ParseOutcome, USAGE},
    spawn_mechanical_backstop, spawn_poll_backstop, Daemon, DaemonRunConfig, ForgeApplier,
    LeaseApplier, MechanicalBackstopConfig, PollBackstopConfig, RepositorySet, RepositoryTarget,
    RoleFeedMode, RoleFeedTarget, RoleRoutingApplier, WebhookConfig,
};
use temper_forge::{Forge, RepositoryId, RepositoryPath, UpsertLabel};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_workflow::{CompiledWorkflow, LeasePolicy, RoleId};

fn main() -> ExitCode {
    let config = match temper_daemon::config::parse(std::env::args().skip(1)) {
        Ok(ParseOutcome::Help) => {
            println!("usage: {USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseOutcome::Run(config)) => config,
        Err(error) => {
            eprintln!("temper-daemon: {error}\nusage: {USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper-daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: DaemonRunConfig) -> Result<(), String> {
    // The daemon runs entirely as engine tasks: the HTTP listener, the pure
    // daemon machine's engine loop, backstop cadence machines, appliers, and
    // wake scans are all completion-driven I/O on the asupersync runtime.
    temper_io_engine::block_on(async move { run_async(config).await })
}

async fn run_async(config: DaemonRunConfig) -> Result<(), String> {
    let forge_config =
        ForgejoConfig::from_env().map_err(|error| format!("Forgejo config: {error}"))?;
    let forge_base_url = forge_config.base_url.clone();
    let forge = Arc::new(ForgejoForge::new(forge_config));
    let workflow = Arc::new(
        temper_reference_delivery::resolve_workflow(config.workflow_file.as_ref())
            .map_err(|error| format!("failed to resolve workflow: {error}"))?,
    );
    let compiled = Arc::new(workflow.compile());
    let repositories = resolve_repositories(forge.as_ref(), &config.repos).await?;
    let repo_ids: Vec<RepositoryId> = repositories
        .repositories()
        .iter()
        .map(|repository| repository.id.clone())
        .collect();
    let normal_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Normal);
    let wake_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Wake);
    let lease_ttl = chrono::Duration::from_std(config.lease_ttl)
        .map_err(|error| format!("invalid --lease-ttl-secs: {error}"))?;

    let daemon = Daemon::with_applier(result_applier(
        forge.clone(),
        forge_base_url,
        workflow.clone(),
        &config,
        lease_ttl,
    ));

    let poll_config = PollBackstopConfig {
        targets: normal_targets,
        cadence: config.poll_cadence,
    };
    spawn_poll_backstop(
        daemon.clone(),
        forge.clone(),
        workflow.clone(),
        compiled.clone(),
        poll_config,
    );

    if let Some(cadence) = config.mechanical_cadence {
        ensure_workflow_labels(forge.as_ref(), &repositories, compiled.as_ref()).await?;
        let mechanical_config = MechanicalBackstopConfig {
            repositories,
            cadence,
            lease_policy: LeasePolicy::new(lease_ttl),
        };
        spawn_mechanical_backstop(forge.clone(), workflow.clone(), mechanical_config);
    }

    let daemon = if let Some(path) = config.webhook_secret_file.as_ref() {
        let secret = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read --webhook-secret-file {}: {error}",
                path.display()
            )
        })?;
        let webhook_config = Arc::new(WebhookConfig {
            secret: secret.trim().to_string(),
            targets: wake_targets,
        });
        daemon.with_webhook(forge, workflow, compiled, webhook_config)
    } else {
        daemon
    };

    let server = temper_daemon::serve(&daemon, config.bind)
        .await
        .map_err(|error| format!("serve failed: {error}"))?;

    if let Err(error) = asupersync::signal::ctrl_c().await {
        eprintln!("temper-daemon: failed to listen for shutdown signal: {error}");
    }
    server.begin_drain(std::time::Duration::from_secs(5));
    Ok(())
}

fn result_applier(
    default_forge: Arc<ForgejoForge>,
    forge_base_url: String,
    workflow: Arc<temper_workflow::ValidatedWorkflow>,
    config: &DaemonRunConfig,
    lease_ttl: chrono::Duration,
) -> Arc<dyn temper_daemon::ResultApplier> {
    let default_chain = applier_chain(
        default_forge,
        workflow.clone(),
        config.daemon_id.clone(),
        lease_ttl,
    );
    let role_tokens = role_tokens_from_env(
        config.roles.iter().map(|role| role.as_str().to_string()),
        std::env::vars(),
    );
    if role_tokens.is_empty() {
        return default_chain;
    }

    let mut routing = RoleRoutingApplier::new(default_chain);
    let mut routed = Vec::new();
    let mut fallback = Vec::new();

    for role in &config.roles {
        let role = role.as_str().to_string();
        if let Some(token) = role_tokens.get(&role) {
            let role_forge = Arc::new(ForgejoForge::new(ForgejoConfig::new(
                forge_base_url.clone(),
                token.clone(),
            )));
            let role_chain = applier_chain(
                role_forge,
                workflow.clone(),
                config.daemon_id.clone(),
                lease_ttl,
            );
            routing = routing.with_route(role.clone(), role_chain);
            routed.push(role);
        } else {
            fallback.push(role);
        }
    }

    eprintln!(
        "temper-daemon: role identities routed={} fallback={}",
        role_list(&routed),
        role_list(&fallback)
    );

    Arc::new(routing)
}

fn applier_chain(
    forge: Arc<ForgejoForge>,
    workflow: Arc<temper_workflow::ValidatedWorkflow>,
    daemon_id: String,
    lease_ttl: chrono::Duration,
) -> Arc<dyn temper_daemon::ResultApplier> {
    Arc::new(LeaseApplier::new(
        forge.clone(),
        LeasePolicy::new(lease_ttl),
        daemon_id,
        Arc::new(ForgeApplier::new(forge, workflow)),
    ))
}

fn role_list(roles: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let roles = roles
        .into_iter()
        .map(|role| role.as_ref().to_string())
        .collect::<Vec<_>>();
    if roles.is_empty() {
        "(none)".to_string()
    } else {
        roles.join(",")
    }
}

async fn resolve_repositories<F: Forge + ?Sized>(
    forge: &F,
    repos: &[RepositoryPath],
) -> Result<RepositorySet, String> {
    let mut resolved = Vec::with_capacity(repos.len());
    for path in repos {
        let repository = forge
            .get_repository_by_path(path)
            .await
            .map_err(|error| format!("repository {} lookup failed: {error}", repo_label(path)))?
            .ok_or_else(|| format!("repository {} not found", repo_label(path)))?;
        resolved.push(RepositoryTarget::new(
            repository.id,
            RepositoryPath::new(repository.owner, repository.name),
        ));
    }
    Ok(RepositorySet::new(resolved))
}

async fn ensure_workflow_labels<F: Forge + ?Sized>(
    forge: &F,
    repositories: &RepositorySet,
    compiled: &CompiledWorkflow,
) -> Result<(), String> {
    for repository in repositories.repositories() {
        for label in compiled.labels().labels() {
            forge
                .upsert_label(
                    &repository.id,
                    UpsertLabel {
                        name: label.id.to_string(),
                        color: Some("#ededed".to_string()),
                        description: None,
                    },
                )
                .await
                .map_err(|error| {
                    format!(
                        "repository {} label {} upsert failed: {error}",
                        repository.display_path(),
                        label.id
                    )
                })?;
        }
    }
    Ok(())
}

fn role_feed_targets(
    repos: &[RepositoryId],
    roles: &[RoleId],
    mode: RoleFeedMode,
) -> Vec<RoleFeedTarget> {
    let mut targets = Vec::with_capacity(repos.len() * roles.len());
    for repo in repos {
        for role in roles {
            targets.push(RoleFeedTarget {
                repo: repo.clone(),
                role: role.clone(),
                mode,
            });
        }
    }
    targets
}

fn repo_label(path: &RepositoryPath) -> String {
    format!("{}/{}", path.owner, path.name)
}
