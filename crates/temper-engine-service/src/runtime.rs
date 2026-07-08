// SPDX-License-Identifier: MPL-2.0

//! Engine service runtime wiring.

use std::sync::Arc;

use temper_config::{ExposeSecret, Resolved};
use temper_engine::{
    Daemon, DaemonRunConfig, EngineConfig, HintedMechanical, MechanicalBackstopConfig,
    PollBackstopConfig, RepositorySet, RoleFeedMode, RoleFeedTarget, WebhookConfig,
    spawn_mechanical_backstop, spawn_poll_backstop,
};
use temper_forge::{Forge, RepositoryId, RepositoryPath};
use temper_workflow::{CompiledWorkflow, LeasePolicy, ValidatedWorkflow};

use crate::{
    engine_config, ensure_workflow_labels, resolve_repositories, result_applier, role_feed_targets,
    worker_pool_auth_config,
};

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
    let EngineConfig {
        daemon: config,
        forge: forge_config,
        role_tokens,
    } = engine_config(resolved)?;
    let forge_config_for_roles = forge_config.clone();
    let forge = temper_forge::factory::new_forgejo(forge_config);

    let (workflow, compiled) = load_workflow(&config)?;
    let (repositories, repo_ids) = resolve_repo_targets(forge.as_ref(), &config.repos).await?;
    let normal_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Normal);
    let wake_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Wake);
    let lease_ttl = lease_ttl(&config)?;

    let daemon = Daemon::with_applier_and_worker_pools(
        Arc::clone(&spawner),
        result_applier(
            forge.clone(),
            forge_config_for_roles,
            workflow.clone(),
            &config,
            &role_tokens,
            lease_ttl,
        ),
        config.worker_pools.clone(),
    )
    .with_worker_pool_auth(worker_pool_auth_config(resolved)?);

    spawn_poll(
        &spawner,
        daemon.clone(),
        forge.clone(),
        workflow.clone(),
        compiled.clone(),
        normal_targets,
        config.poll_cadence,
    );

    let mechanical_trigger = spawn_mechanical(
        &spawner,
        forge.clone(),
        workflow.clone(),
        compiled.as_ref(),
        repositories,
        config.mechanical_cadence,
        lease_ttl,
    )
    .await?;
    let daemon = attach_webhook(
        daemon,
        forge,
        workflow,
        compiled,
        resolved.engine.webhook_secret_value.as_ref(),
        config.webhook_secret_file.as_ref(),
        wake_targets,
        mechanical_trigger,
    )?;

    drain_after_signal(&handle, &daemon, config.bind).await
}

fn load_workflow(
    config: &DaemonRunConfig,
) -> Result<(Arc<ValidatedWorkflow>, Arc<CompiledWorkflow>), String> {
    let workflow = Arc::new(
        temper_reference_delivery::resolve_workflow(config.workflow_file.as_ref())
            .map_err(|error| format!("failed to resolve workflow: {error}"))?,
    );
    let compiled = Arc::new(workflow.compile());
    Ok((workflow, compiled))
}

async fn resolve_repo_targets(
    forge: &dyn Forge,
    repos: &[RepositoryPath],
) -> Result<(RepositorySet, Vec<RepositoryId>), String> {
    let repositories = resolve_repositories(forge, repos).await?;
    let repo_ids = repositories
        .repositories()
        .iter()
        .map(|repository| repository.id.clone())
        .collect();
    Ok((repositories, repo_ids))
}

fn lease_ttl(config: &DaemonRunConfig) -> Result<chrono::Duration, String> {
    chrono::Duration::from_std(config.lease_ttl)
        .map_err(|error| format!("invalid lease ttl: {error}"))
}

fn spawn_poll(
    spawner: &Arc<dyn temper_engine_io::Spawner>,
    daemon: Daemon,
    forge: Arc<dyn Forge>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    targets: Vec<RoleFeedTarget>,
    cadence: std::time::Duration,
) {
    let poll_config = PollBackstopConfig { targets, cadence };
    spawn_poll_backstop(
        spawner,
        daemon,
        forge,
        workflow,
        compiled,
        poll_config,
        temper_engine::system_clock(),
    );
}

async fn spawn_mechanical(
    spawner: &Arc<dyn temper_engine_io::Spawner>,
    forge: Arc<dyn Forge>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: &CompiledWorkflow,
    repositories: RepositorySet,
    cadence: Option<std::time::Duration>,
    lease_ttl: chrono::Duration,
) -> Result<Option<Arc<dyn HintedMechanical>>, String> {
    // A webhook delivery runs an immediate hinted mechanical pass through this,
    // so the cadence itself can stay slow without losing reaction latency.
    let Some(cadence) = cadence else {
        return Ok(None);
    };
    ensure_workflow_labels(forge.as_ref(), &repositories, compiled).await?;
    let mechanical_config = MechanicalBackstopConfig {
        repositories,
        cadence,
        lease_policy: LeasePolicy::new(lease_ttl),
        // TODO(#477 split-worker): distributed engine deployments observe the
        // merge but do not own worker workspaces; add a worker-protocol cleanup
        // request before enabling landed-workstream cleanup outside standalone.
        pull_request_merge_observer: None,
    };
    let trigger = spawn_mechanical_backstop(
        spawner,
        forge,
        workflow,
        mechanical_config,
        temper_engine::system_clock(),
    );
    Ok(Some(Arc::new(trigger)))
}

#[allow(clippy::too_many_arguments)]
fn attach_webhook(
    daemon: Daemon,
    forge: Arc<dyn Forge>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    secret_value: Option<&temper_config::Secret>,
    secret_file: Option<&std::path::PathBuf>,
    wake_targets: Vec<RoleFeedTarget>,
    mechanical_trigger: Option<Arc<dyn HintedMechanical>>,
) -> Result<Daemon, String> {
    let secret = if let Some(secret) = secret_value {
        secret.expose_secret().trim().to_string()
    } else if let Some(path) = secret_file {
        std::fs::read_to_string(path)
            .map_err(|error| {
                format!(
                    "failed to read webhook secret file {}: {error}",
                    path.display()
                )
            })?
            .trim()
            .to_string()
    } else {
        return Ok(daemon);
    };
    let webhook_config = Arc::new(WebhookConfig {
        secret,
        targets: wake_targets,
    });
    Ok(daemon.with_webhook_and_mechanical(
        forge,
        workflow,
        compiled,
        webhook_config,
        temper_engine::system_clock(),
        mechanical_trigger,
    ))
}

async fn drain_after_signal(
    handle: &skein::runtime::RuntimeHandle,
    daemon: &Daemon,
    bind: std::net::SocketAddr,
) -> Result<(), String> {
    let mut sigint = skein::signal::sigint()
        .map_err(|error| format!("failed to register SIGINT handler: {error}"))?;
    let mut sigterm = skein::signal::sigterm()
        .map_err(|error| format!("failed to register SIGTERM handler: {error}"))?;

    let server = temper_engine::serve(handle, daemon, bind)
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
