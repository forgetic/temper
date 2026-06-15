// SPDX-License-Identifier: MPL-2.0

//! Standalone (all-in-one) `temper daemon`: engine + worker + agent on one
//! single-threaded event loop.
//!
//! Assembles all three planes in one process: a [`Daemon`] (the orchestrator),
//! an in-process worker driving an [`InProcessAgentRunner`], wired by an
//! [`InProcessTransport`] (no HTTP byte round-trip), and the coding agent running
//! as per-job futures on the same loop. The engine half reuses the exact wiring
//! the slim `temper-engine` binary runs (via [`temper_engine_service`]); only the
//! worker→daemon carrier and the worker→agent runner differ (in-memory and
//! in-process, vs. HTTP + subprocess).

mod agent_runner;
mod transport;

pub use agent_runner::InProcessAgentRunner;
pub use transport::InProcessTransport;

use std::sync::Arc;
use std::time::Duration;

use skein::runtime::RuntimeHandle;
use temper_config::Resolved;
use temper_engine::{
    Daemon, HintedMechanical, MechanicalBackstopConfig, PollBackstopConfig, RoleFeedMode,
    WebhookConfig, spawn_mechanical_backstop, spawn_poll_backstop,
};
use temper_engine_service::{
    daemon_run_config, ensure_workflow_labels, forgejo_config, resolve_repositories,
    result_applier, role_feed_targets,
};
use temper_forge::RepositoryId;
use temper_worker::{
    CapabilitySpec, CodingExecutor, CodingExecutorConfig, ExecutorSelection, WorkerConfig,
    run_worker_with_transport,
};
use temper_workflow::LeasePolicy;

/// Runs the standalone daemon on the skein runtime until SIGINT/SIGTERM.
pub fn run(resolved: &Resolved) -> Result<(), String> {
    let resolved = resolved.clone();
    temper_engine_io::block_on_with(
        move |_cx, handle| async move { run_async(handle, &resolved).await },
    )
}

async fn run_async(handle: RuntimeHandle, resolved: &Resolved) -> Result<(), String> {
    let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle.clone());

    // --- Forge + workflow + repositories (the engine half, reusing the engine
    //     service's adapters + wiring) ---
    let forge_config = forgejo_config(resolved)?;
    let forge_base_url = forge_config.base_url.clone();
    let forge = temper_forge::factory::new_forgejo(forge_config);
    let daemon_config = daemon_run_config(resolved)?;

    let workflow = Arc::new(
        temper_reference_delivery::resolve_workflow(daemon_config.workflow_file.as_ref())
            .map_err(|error| format!("failed to resolve workflow: {error}"))?,
    );
    let compiled = Arc::new(workflow.compile());
    let repositories = resolve_repositories(forge.as_ref(), &daemon_config.repos).await?;
    let repo_ids: Vec<RepositoryId> = repositories
        .repositories()
        .iter()
        .map(|repository| repository.id.clone())
        .collect();
    // The worker registers capabilities keyed by the artifact repo's owner/name
    // (the same key the daemon assigns jobs on). Capture it before `repositories`
    // is moved into the mechanical backstop config.
    let repo_paths: Vec<String> = repositories
        .repositories()
        .iter()
        .map(|repository| repository.display_path())
        .collect();
    let lease_ttl = chrono::Duration::from_std(daemon_config.lease_ttl)
        .map_err(|error| format!("invalid lease ttl: {error}"))?;

    // --- Daemon (orchestrator) on this loop, with per-role token routing ---
    let applier = result_applier(
        forge.clone(),
        forge_base_url,
        workflow.clone(),
        &daemon_config,
        &resolved.forge.role_tokens,
        lease_ttl,
    );
    let daemon = Daemon::with_applier(Arc::clone(&spawner), applier);

    spawn_poll_backstop(
        &spawner,
        daemon.clone(),
        forge.clone(),
        workflow.clone(),
        compiled.clone(),
        PollBackstopConfig {
            targets: role_feed_targets(&repo_ids, &daemon_config.roles, RoleFeedMode::Normal),
            cadence: daemon_config.poll_cadence,
        },
        temper_engine::system_clock(),
    );

    let mut mechanical_trigger: Option<Arc<dyn HintedMechanical>> = None;
    if let Some(cadence) = daemon_config.mechanical_cadence {
        ensure_workflow_labels(forge.as_ref(), &repositories, compiled.as_ref()).await?;
        let trigger = spawn_mechanical_backstop(
            &spawner,
            forge.clone(),
            workflow.clone(),
            MechanicalBackstopConfig {
                repositories,
                cadence,
                lease_policy: LeasePolicy::new(lease_ttl),
            },
            temper_engine::system_clock(),
        );
        mechanical_trigger = Some(Arc::new(trigger));
    }

    // --- In-process worker + agent on the same loop ---
    let provider = crate::provider::build_provider(
        resolved,
        &resolved.worker.workspace_root.join(".temper-auth"),
    )?;
    let role_identities = temper_worker_service::role_identities(resolved);
    let git_base_url = temper_worker_service::git_base_url(resolved)?;
    let capabilities: Vec<CapabilitySpec> = repo_paths
        .iter()
        .flat_map(|repo| {
            daemon_config.roles.iter().map(move |role| CapabilitySpec {
                role: role.as_str().to_string(),
                repo: repo.clone(),
            })
        })
        .collect();

    let runner = Arc::new(InProcessAgentRunner::new(
        handle.clone(),
        provider,
        resolved.agent.max_iterations,
        resolved.agent.config_dir.clone(),
        resolved.agent.enable_subagents,
    ));
    let executor = Arc::new(
        CodingExecutor::new(
            CodingExecutorConfig {
                workspace_root: resolved.worker.workspace_root.clone(),
                git_base_url,
                role_identities,
            },
            runner,
        )
        .with_progress_sink(Arc::new(InProcessProgressSink::new(
            handle.clone(),
            daemon.clone(),
            resolved.worker.worker_id.clone(),
        ))),
    );

    let worker_config = WorkerConfig {
        // Unused on the in-process transport, but the struct carries it.
        daemon_url: String::new(),
        worker_id: resolved.worker.worker_id.clone(),
        capabilities,
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_secs(20),
        heartbeat_interval: Duration::from_secs(10),
        executor: ExecutorSelection::Stub, // not consulted: the executor is built directly
    };
    let transport = Arc::new(InProcessTransport::new(daemon.clone()));

    let worker_handle = handle.clone();
    handle.spawn_with_cx(move |_cx| async move {
        let _ = run_worker_with_transport(worker_handle, worker_config, executor, transport).await;
    });

    // --- Webhook route (optional) ---
    let daemon = if let Some(path) = daemon_config.webhook_secret_file.as_ref() {
        let secret = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read webhook secret file {}: {error}",
                path.display()
            )
        })?;
        let webhook_config = Arc::new(WebhookConfig {
            secret: secret.trim().to_string(),
            targets: role_feed_targets(&repo_ids, &daemon_config.roles, RoleFeedMode::Wake),
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

    // --- HTTP listener (the readiness signal operators wait for) ---
    let server = temper_engine::serve(&handle, &daemon, daemon_config.bind)
        .await
        .map_err(|error| format!("serve failed: {error}"))?;
    // The component log prefix is `temper-daemon:` (matching the engine's webhook
    // / mechanical lines and `temper-worker:`), independent of the `temper daemon`
    // command name.
    eprintln!("temper-daemon: serving on {}", server.local_addr());

    let mut sigint = skein::signal::sigint()
        .map_err(|error| format!("failed to register SIGINT handler: {error}"))?;
    let mut sigterm = skein::signal::sigterm()
        .map_err(|error| format!("failed to register SIGTERM handler: {error}"))?;
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

/// Relay agent step-progress to the co-resident daemon in-process (no HTTP).
/// Fire-and-forget per the sink contract: a slow/failed apply never stalls or
/// fails the agent turn.
struct InProcessProgressSink {
    handle: RuntimeHandle,
    daemon: Daemon,
    worker_id: String,
}

impl InProcessProgressSink {
    fn new(handle: RuntimeHandle, daemon: Daemon, worker_id: String) -> Self {
        Self {
            handle,
            daemon,
            worker_id,
        }
    }
}

impl temper_worker::ProgressSink for InProcessProgressSink {
    fn report(&self, progress: temper_agent_protocol::StepProgress) {
        let message = temper_worker::progress_message(&self.worker_id, &progress);
        let daemon = self.daemon.clone();
        self.handle.spawn_with_cx(move |_cx| async move {
            let _ = daemon.deliver_protocol_message(message).await;
        });
    }
}
