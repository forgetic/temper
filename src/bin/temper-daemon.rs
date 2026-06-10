// SPDX-License-Identifier: MPL-2.0

use std::{process::ExitCode, sync::Arc};

use temper_daemon::{
    config::{ParseOutcome, USAGE},
    router_with_webhook, run_mechanical_backstop, run_poll_backstop, serve_router, Daemon,
    DaemonRunConfig, ForgeApplier, LeaseApplier, MechanicalBackstopConfig, PollBackstopConfig,
    RepositorySet, RepositoryTarget, RoleFeedMode, RoleFeedTarget, WebhookConfig,
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to build tokio runtime: {error}"))?;

    runtime.block_on(run_async(config))
}

async fn run_async(config: DaemonRunConfig) -> Result<(), String> {
    let forge = Arc::new(ForgejoForge::new(
        ForgejoConfig::from_env().map_err(|error| format!("Forgejo config: {error}"))?,
    ));
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

    let daemon = Daemon::with_applier(Arc::new(LeaseApplier::new(
        forge.clone(),
        LeasePolicy::new(lease_ttl),
        config.daemon_id.clone(),
        Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
    )));

    let poll_config = PollBackstopConfig {
        targets: normal_targets,
        cadence: config.poll_cadence,
    };
    let poll_daemon = daemon.clone();
    let poll_forge = forge.clone();
    let poll_workflow = workflow.clone();
    let poll_compiled = compiled.clone();
    tokio::spawn(async move {
        run_poll_backstop(
            &poll_daemon,
            poll_forge.as_ref(),
            poll_workflow.as_ref(),
            poll_compiled.as_ref(),
            &poll_config,
        )
        .await;
    });

    if let Some(cadence) = config.mechanical_cadence {
        ensure_workflow_labels(forge.as_ref(), &repositories, compiled.as_ref()).await?;
        let mechanical_config = MechanicalBackstopConfig {
            repositories,
            cadence,
            lease_policy: LeasePolicy::new(lease_ttl),
        };
        let mechanical_forge = forge.clone();
        let mechanical_workflow = workflow.clone();
        tokio::spawn(async move {
            run_mechanical_backstop(
                mechanical_forge.as_ref(),
                mechanical_workflow.as_ref(),
                &mechanical_config,
            )
            .await;
        });
    }

    let router = if let Some(path) = config.webhook_secret_file.as_ref() {
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
        router_with_webhook(&daemon, forge, workflow, compiled, webhook_config)
    } else {
        daemon.router()
    };

    serve_router(router, config.bind)
        .await
        .map_err(|error| format!("serve failed: {error}"))
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
