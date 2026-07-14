// SPDX-License-Identifier: MPL-2.0

//! Engine service runtime wiring.

use std::collections::BTreeMap;
use std::sync::Arc;

use temper_config::{ExposeSecret, Resolved};
use temper_engine::{
    AgentTraceJournal, Daemon, DaemonRunConfig, EngineAgentTraceConfig, EngineConfig,
    HintedMechanical, MechanicalBackstopConfig, MechanicalScope, PollBackstopConfig, RepositorySet,
    RetentionProtection, RoleFeedMode, RoleFeedTarget, WebhookConfig, run_mechanical_backstop_tick,
    spawn_mechanical_backstop, spawn_poll_backstop,
};
use temper_forge::{Forge, RepositoryId, RepositoryPath};
use temper_workflow::{CompiledWorkflow, InMemoryJournal, LeasePolicy, ValidatedWorkflow};

use crate::{
    converge_startup_orphans, engine_config, ensure_workflow_labels, resolve_repositories,
    result_applier, role_feed_targets, stage_startup_assignments, worker_pool_auth_config,
    workflow_role_limits,
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
        agent_traces,
    } = engine_config(resolved)?;
    if resolved.observability.agent_traces.capture_requested()
        && agent_traces.journal_root.is_none()
    {
        tracing::warn!(
            target: "temper::engine",
            service = "engine",
            "agent tracing disabled: no durable paths.state_dir is available for the engine journal"
        );
    }
    let forge_config_for_roles = forge_config.clone();
    let forge_url = forge_config.base_url.clone();
    let forge = temper_forge::factory::new_forgejo(forge_config);

    let (workflow, compiled) = load_workflow(&config)?;
    let role_limits = workflow_role_limits(&compiled);
    let (repositories, repo_ids) = resolve_repo_targets(forge.as_ref(), &config.repos).await?;
    let artifact_catalog =
        temper_engine::ConfiguredRepositoryCatalog::from_repository_set(&repositories, forge_url)?;
    let artifact_context = Arc::new(temper_engine::ArtifactContextBundleService::new(
        forge.clone(),
        workflow.clone(),
        artifact_catalog,
        temper_engine::ArtifactContextPolicy::default(),
    ));
    let normal_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Normal);
    let wake_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Wake);
    let lease_ttl = lease_ttl(&config)?;

    // Complete durable child-create intents before constructing the daemon or
    // spawning either dispatch backstop. This is the startup recovery barrier:
    // no role scan can observe a partially-wired child while recovery runs.
    recover_child_create_intents(forge.as_ref(), workflow.as_ref(), &repo_ids).await?;

    let daemon = split_daemon(
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
        role_limits,
    )
    .with_worker_pool_auth(worker_pool_auth_config(resolved)?)
    .with_artifact_context_service(artifact_context)
    .with_forge_context_reader(forge.clone(), workflow.clone())
    .begin_startup_recovery();

    // Inventory durable claims before opening any feed. The worker protocol
    // listener is intentionally available during the bounded grace so external
    // workers can register and prove prior ownership with `Heartbeat.jobs`.
    let recovered = stage_startup_assignments(
        &daemon,
        forge.as_ref(),
        &repo_ids,
        workflow.as_ref(),
        compiled.as_ref(),
        LeasePolicy::new(lease_ttl),
        (temper_engine::system_clock())(),
    )
    .await?;
    // Trace storage is intentionally best-effort: startup recovery and
    // retention run before transport opens, but a journal failure can never
    // prevent assignment execution.
    let trace_journal = start_trace_journal(&agent_traces, recovered.keys().cloned());
    let daemon = attach_trace_query(daemon, &agent_traces, trace_journal.as_ref());
    let server = temper_engine::serve(&handle, &daemon, config.bind)
        .await
        .map_err(|error| format!("serve failed: {error}"))?;
    if !recovered.is_empty() {
        daemon
            .wait_startup_recovery_grace(std::time::Duration::from_secs(10))
            .await;
    }
    let orphaned = daemon.collect_startup_orphans().await;
    converge_startup_orphans(
        forge.as_ref(),
        LeasePolicy::new(lease_ttl),
        workflow.as_ref(),
        &recovered,
        &orphaned,
    )
    .await?;

    let mechanical_trigger = spawn_mechanical(
        &spawner,
        forge.clone(),
        workflow.clone(),
        compiled.as_ref(),
        repositories,
        &daemon,
        config.mechanical_cadence,
        lease_ttl,
    )
    .await?;

    spawn_poll(
        &spawner,
        daemon.clone(),
        forge.clone(),
        workflow.clone(),
        compiled.clone(),
        normal_targets,
        config.poll_cadence,
    );
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

    drain_after_signal(&daemon, server).await
}

pub fn start_trace_journal(
    config: &EngineAgentTraceConfig,
    recovered_job_ids: impl IntoIterator<Item = String>,
) -> Option<AgentTraceJournal> {
    let protection = RetentionProtection {
        job_ids: recovered_job_ids.into_iter().collect(),
        ..RetentionProtection::default()
    };
    match AgentTraceJournal::from_engine_config_with_clock_and_protection(
        config,
        temper_engine::system_clock(),
        &protection,
    ) {
        Ok(journal) => journal,
        Err(error) => {
            tracing::error!(
                target: "temper::engine",
                error = %error,
                "agent trace journal unavailable; assignment execution will continue"
            );
            None
        }
    }
}

/// Attaches the journal-backed query executor only when the named read token
/// resolved. Keeping this composition helper shared gives split and standalone
/// deployments identical disabled-route behavior.
pub fn attach_trace_query(
    daemon: Daemon,
    config: &EngineAgentTraceConfig,
    journal: Option<&AgentTraceJournal>,
) -> Daemon {
    match (journal, config.read_token.as_ref()) {
        (Some(journal), Some(read_token)) => {
            daemon.with_agent_trace_query(journal.clone(), read_token.clone())
        }
        _ => daemon,
    }
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

fn split_daemon(
    spawner: Arc<dyn temper_engine_io::Spawner>,
    applier: Arc<dyn temper_engine::ResultApplier>,
    worker_pools: Vec<temper_engine::WorkerPoolPolicy>,
    role_limits: BTreeMap<String, u32>,
) -> Daemon {
    Daemon::with_applier_worker_pools_and_role_limits(spawner, applier, worker_pools, role_limits)
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

async fn recover_child_create_intents(
    forge: &dyn Forge,
    workflow: &ValidatedWorkflow,
    repo_ids: &[RepositoryId],
) -> Result<(), String> {
    let executor = workflow.executor(forge);
    for repo_id in repo_ids {
        executor
            .recover_create_issue_intents(repo_id)
            .await
            .map_err(|error| {
                format!("failed to recover durable child-create intents in `{repo_id}`: {error}")
            })?;
    }
    Ok(())
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

#[allow(clippy::too_many_arguments)]
async fn spawn_mechanical(
    spawner: &Arc<dyn temper_engine_io::Spawner>,
    forge: Arc<dyn Forge>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: &CompiledWorkflow,
    repositories: RepositorySet,
    daemon: &Daemon,
    cadence: Option<std::time::Duration>,
    lease_ttl: chrono::Duration,
) -> Result<Option<Arc<dyn HintedMechanical>>, String> {
    ensure_workflow_labels(forge.as_ref(), &repositories, compiled).await?;
    let initial_config = MechanicalBackstopConfig {
        repositories: repositories.clone(),
        cadence: cadence.unwrap_or(std::time::Duration::from_secs(1)),
        lease_policy: LeasePolicy::new(lease_ttl),
        // TODO(#477 split-worker): distributed engine deployments observe the
        // merge but do not own worker workspaces; add a worker-protocol cleanup
        // request before enabling landed-workstream cleanup outside standalone.
        pull_request_merge_observer: None,
    };
    let journals = (0..repositories.repositories().len())
        .map(|_| InMemoryJournal::new())
        .collect::<Vec<_>>();
    run_mechanical_backstop_tick(
        forge.as_ref(),
        workflow.as_ref(),
        (temper_engine::system_clock())(),
        &initial_config,
        &journals,
        &MechanicalScope::All,
    )
    .await
    .map_err(|error| format!("startup mechanical reconciliation failed: {error}"))?;
    daemon.complete_startup_recovery().await;

    let Some(cadence) = cadence else {
        return Ok(None);
    };
    let mechanical_config = MechanicalBackstopConfig {
        cadence,
        ..initial_config
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
    daemon_guard: &Daemon,
    server: temper_engine_io::http::EngineHttpServer,
) -> Result<(), String> {
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
    daemon_guard.release_assignments_for_shutdown().await;
    server.begin_drain(std::time::Duration::from_secs(5));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_engine::NoopApplier;
    use temper_protocol_worker::{
        Artifact, Capability, Capacity, Poll, Register, WORKER_PROTOCOL_VERSION,
        WorkerProtocolMessage,
    };

    #[test]
    fn split_daemon_preserves_distinct_workflow_role_limits() {
        temper_engine_io::block_on_with(move |_cx, handle| async move {
            let daemon = split_daemon(
                Arc::new(handle),
                Arc::new(NoopApplier),
                Vec::new(),
                BTreeMap::from([("alpha".to_string(), 1), ("beta".to_string(), 2)]),
            );

            assert_configured_dispatch(&daemon).await;
        });
    }

    async fn assert_configured_dispatch(daemon: &Daemon) {
        let roles = ["alpha", "beta", "gamma"];
        let register = WorkerProtocolMessage::Register(Register {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker".to_string(),
            worker_pool: None,
            capabilities: roles
                .iter()
                .map(|role| Capability {
                    role: (*role).to_string(),
                    repo: "acme/widgets".to_string(),
                })
                .collect(),
            capacity: Capacity {
                max_concurrent_jobs: 10,
            },
            labels: None,
        });
        assert_eq!(daemon.deliver_protocol_message(register).await, Ok(None),);

        for (job_id, role) in [
            ("alpha-1", "alpha"),
            ("alpha-2", "alpha"),
            ("beta-1", "beta"),
            ("beta-2", "beta"),
            ("beta-3", "beta"),
            ("gamma-1", "gamma"),
            ("gamma-2", "gamma"),
        ] {
            daemon
                .enqueue_job(
                    job_id,
                    role,
                    "acme/widgets",
                    Artifact {
                        item: Default::default(),
                        kind: "issue".to_string(),
                    },
                    Default::default(),
                )
                .await;
        }

        let mut assigned_roles = Vec::new();
        for _ in 0..5 {
            let response = daemon
                .deliver_protocol_message(WorkerProtocolMessage::Poll(Poll {
                    protocol_version: WORKER_PROTOCOL_VERSION,
                    worker_id: "worker".to_string(),
                    free_capacity: 10,
                    max_wait_ms: Some(0),
                }))
                .await
                .expect("poll succeeds")
                .expect("eligible work remains");
            let WorkerProtocolMessage::Assign(assign) = response else {
                panic!("expected assignment, got {response:?}");
            };
            assigned_roles.push(assign.role);
        }

        assert_eq!(assigned_roles, ["alpha", "beta", "beta", "gamma", "gamma"]);
    }
}
