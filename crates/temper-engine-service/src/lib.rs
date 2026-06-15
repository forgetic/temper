// SPDX-License-Identifier: MPL-2.0

//! The **engine** service: the orchestrator that routes work, applies results,
//! and lands PRs.
//!
//! [`run`] is the entry point used by both the slim `temper-engine` binary and
//! the unified binary's `temper daemon --service engine`. The reusable wiring
//! ([`resolve_repositories`], [`role_feed_targets`], [`ensure_workflow_labels`],
//! [`result_applier`]) is `pub` so the unified binary's standalone (all-in-one)
//! mode composes the same engine without duplicating it.

mod adapt;

use std::collections::BTreeMap;
use std::sync::Arc;

use temper_config::Resolved;
use temper_engine::{
    Daemon, DaemonRunConfig, ForgeApplier, HintedMechanical, LeaseApplier,
    MechanicalBackstopConfig, PollBackstopConfig, RepositorySet, RepositoryTarget, ResultApplier,
    RoleFeedMode, RoleFeedTarget, RoleRoutingApplier, WebhookConfig, spawn_mechanical_backstop,
    spawn_poll_backstop,
};
use temper_forge::{Forge, RepositoryId, RepositoryPath, UpsertLabel};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_workflow::{CompiledWorkflow, LeasePolicy, RoleId};

pub use adapt::{daemon_run_config, forgejo_config};

/// Runs the engine on the skein runtime until SIGINT/SIGTERM, then drains.
pub fn run(resolved: &Resolved) -> Result<(), String> {
    // The engine runs entirely as engine tasks: the HTTP listener, the pure
    // daemon machine's loop, backstop cadence machines, appliers, and wake scans
    // are all completion-driven I/O on the skein runtime.
    let resolved = resolved.clone();
    temper_engine_io::block_on_with(
        move |_cx, handle| async move { run_async(handle, &resolved).await },
    )
}

/// The async engine wiring: builds the daemon, backstops, and webhook route, then
/// serves until a shutdown signal.
pub async fn run_async(
    handle: skein::runtime::RuntimeHandle,
    resolved: &Resolved,
) -> Result<(), String> {
    let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle.clone());
    let forge_config = forgejo_config(resolved)?;
    let forge_base_url = forge_config.base_url.clone();
    let forge = Arc::new(ForgejoForge::new(forge_config));
    let config = daemon_run_config(resolved)?;

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
        .map_err(|error| format!("invalid lease ttl: {error}"))?;

    let daemon = Daemon::with_applier(
        Arc::clone(&spawner),
        result_applier(
            forge.clone(),
            forge_base_url,
            workflow.clone(),
            &config,
            &resolved.forge.role_tokens,
            lease_ttl,
        ),
    );

    let poll_config = PollBackstopConfig {
        targets: normal_targets,
        cadence: config.poll_cadence,
    };
    spawn_poll_backstop(
        &spawner,
        daemon.clone(),
        forge.clone(),
        workflow.clone(),
        compiled.clone(),
        poll_config,
        temper_engine::system_clock(),
    );

    // The mechanical accelerator shared with the webhook path, if the mechanical
    // backstop is enabled. A webhook delivery runs an immediate hinted mechanical
    // pass through this, so the backstop cadence itself can stay slow (idle
    // quiet) without losing reaction latency (ADR 0009).
    let mut mechanical_trigger: Option<Arc<dyn HintedMechanical>> = None;
    if let Some(cadence) = config.mechanical_cadence {
        ensure_workflow_labels(forge.as_ref(), &repositories, compiled.as_ref()).await?;
        let mechanical_config = MechanicalBackstopConfig {
            repositories,
            cadence,
            lease_policy: LeasePolicy::new(lease_ttl),
        };
        let trigger = spawn_mechanical_backstop(
            &spawner,
            forge.clone(),
            workflow.clone(),
            mechanical_config,
            temper_engine::system_clock(),
        );
        mechanical_trigger = Some(Arc::new(trigger));
    }

    let daemon = if let Some(path) = config.webhook_secret_file.as_ref() {
        let secret = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read webhook secret file {}: {error}",
                path.display()
            )
        })?;
        let webhook_config = Arc::new(WebhookConfig {
            secret: secret.trim().to_string(),
            targets: wake_targets,
        });
        daemon.with_webhook_and_mechanical(
            forge,
            workflow,
            compiled,
            webhook_config,
            temper_engine::system_clock(),
            mechanical_trigger,
        )
    } else {
        daemon
    };

    let mut sigint = skein::signal::sigint()
        .map_err(|error| format!("failed to register SIGINT handler: {error}"))?;
    let mut sigterm = skein::signal::sigterm()
        .map_err(|error| format!("failed to register SIGTERM handler: {error}"))?;

    let server = temper_engine::serve(&handle, &daemon, config.bind)
        .await
        .map_err(|error| format!("serve failed: {error}"))?;

    std::future::poll_fn(|task_cx| {
        if sigint.poll_recv(task_cx).is_ready() || sigterm.poll_recv(task_cx).is_ready() {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    server.begin_drain(std::time::Duration::from_secs(5));
    Ok(())
}

/// Builds the result applier chain, routing each role's writes through that
/// role's forge identity when a per-role token is available (otherwise the
/// default identity).
pub fn result_applier(
    default_forge: Arc<ForgejoForge>,
    forge_base_url: String,
    workflow: Arc<temper_workflow::ValidatedWorkflow>,
    config: &DaemonRunConfig,
    role_tokens: &BTreeMap<String, String>,
    lease_ttl: chrono::Duration,
) -> Arc<dyn ResultApplier> {
    let default_chain = applier_chain(
        default_forge,
        workflow.clone(),
        config.daemon_id.clone(),
        lease_ttl,
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
        "temper-engine: role identities routed={} fallback={}",
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
) -> Arc<dyn ResultApplier> {
    Arc::new(LeaseApplier::new(
        forge.clone(),
        LeasePolicy::new(lease_ttl),
        daemon_id,
        Arc::new(ForgeApplier::new(forge, workflow)),
        temper_engine::system_clock(),
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

/// Resolves each configured `owner/name` to a live repository (id + path),
/// failing if any is missing on the forge.
pub async fn resolve_repositories<F: Forge + ?Sized>(
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

/// Upserts every workflow label on every repository (idempotent), so the
/// mechanical backstop's label transitions have somewhere to land.
pub async fn ensure_workflow_labels<F: Forge + ?Sized>(
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

/// The cross-product of repositories and roles in the given feed mode.
pub fn role_feed_targets(
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
