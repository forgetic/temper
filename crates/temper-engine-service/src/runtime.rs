// SPDX-License-Identifier: MPL-2.0

//! Engine service runtime wiring.

use std::sync::Arc;

use temper_config::Resolved;
use temper_engine::{
    Daemon, DaemonRunConfig, HintedMechanical, MechanicalBackstopConfig, PollBackstopConfig,
    RepositorySet, RoleFeedMode, RoleFeedTarget, WebhookConfig, spawn_mechanical_backstop,
    spawn_poll_backstop,
};
use temper_forge_model::{RepositoryId, RepositoryPath};
use temper_forge_forgejo::ForgejoForge;
use temper_workflow::{CompiledWorkflow, LeasePolicy, ValidatedWorkflow};

use crate::{
    daemon_run_config, ensure_workflow_labels, forgejo_config, resolve_repositories,
    result_applier, role_feed_targets,
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
    let forge_config = forgejo_config(resolved)?;
    let forge_base_url = forge_config.base_url.clone();
    let forge = Arc::new(ForgejoForge::new(forge_config));
    let config = daemon_run_config(resolved)?;

    let (workflow, compiled) = load_workflow(&config)?;
    let (repositories, repo_ids) = resolve_repo_targets(forge.as_ref(), &config.repos).await?;
    let normal_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Normal);
    let wake_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Wake);
    let lease_ttl = lease_ttl(&config)?;

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
    forge: &ForgejoForge,
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
    forge: Arc<ForgejoForge>,
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
    forge: Arc<ForgejoForge>,
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

fn attach_webhook(
    daemon: Daemon,
    forge: Arc<ForgejoForge>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    secret_file: Option<&std::path::PathBuf>,
    wake_targets: Vec<RoleFeedTarget>,
    mechanical_trigger: Option<Arc<dyn HintedMechanical>>,
) -> Result<Daemon, String> {
    let Some(path) = secret_file else {
        return Ok(daemon);
    };
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
