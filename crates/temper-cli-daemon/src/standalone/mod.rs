// SPDX-License-Identifier: MPL-2.0

//! Standalone (all-in-one) `temper serve standalone`: engine + worker + agent on
//! one single-threaded event loop.
//!
//! Assembles all three planes in one process: a [`Daemon`] (the orchestrator),
//! an in-process worker driving an [`InProcessAgentRunner`], wired by the
//! reusable [`InProcessTransport`] (no HTTP byte round-trip), and the coding
//! agent running as per-job futures on the same loop. The engine half reuses the
//! exact wiring the slim `temper-engine` binary runs (via
//! [`temper_engine_service`]); only the worker→daemon carrier and the
//! worker→agent runner differ (in-memory and in-process, vs. HTTP + subprocess).

mod agent_runner;
mod banner;
mod trace_config;
mod transport;
mod workstream_cleanup;

pub use agent_runner::InProcessAgentRunner;
pub use transport::InProcessTransport;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use skein::runtime::RuntimeHandle;
use temper_config::{ExposeSecret, Resolved, WorkerSettings};
use temper_engine::{
    Daemon, EngineConfig, HintedMechanical, MechanicalBackstopConfig, MechanicalScope,
    PollBackstopConfig, RoleFeedMode, WebhookConfig, run_mechanical_backstop_tick,
    spawn_mechanical_backstop, spawn_poll_backstop,
};
use temper_engine_service::{
    converge_startup_orphans, engine_config, ensure_workflow_labels, resolve_repositories,
    result_applier, role_feed_targets, stage_startup_assignments, start_trace_journal,
    worker_pool_auth_config, workflow_role_limits,
};
use temper_forge::RepositoryId;
use temper_log::emit::{emit_engine_status, emit_trigger_status, emit_worker_status};
use temper_worker::{
    CapabilitySpec, CodingExecutor, CodingExecutorConfig, ExecutorSelection, RoleGitIdentity,
    WorkerAgentTraceConfig, WorkerConfig, run_worker_with_transport,
};
use temper_worker_service::selected_worker_auth;
use temper_workflow::{InMemoryJournal, LeasePolicy};
use workstream_cleanup::StandaloneWorkstreamCleaner;

/// Runs the standalone daemon on the skein runtime until SIGINT/SIGTERM.
///
/// `config_path` is the on-disk config the deployment loaded from (for the §7
/// `config loaded from <path>` startup line), or `None` when the deployment was
/// assembled from env/defaults only.
pub fn run(resolved: &Resolved, config_path: Option<&Path>) -> Result<(), String> {
    let resolved = resolved.clone();
    let config_path = config_path.map(Path::to_path_buf);
    temper_engine_io::block_on_with(move |cx, handle| async move {
        run_async(cx, handle, &resolved, config_path.as_deref()).await
    })
}

async fn run_async(
    cx: skein::cx::Cx,
    handle: RuntimeHandle,
    resolved: &Resolved,
    config_path: Option<&Path>,
) -> Result<(), String> {
    let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle.clone());

    // §7 startup/health banner — first two lines: who we are and where the
    // config came from. Pure ASCII bodies via the temper-log status helpers; the
    // helper sets the `service=engine` machine field.
    emit_engine_status(banner::starting(
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
    ));
    emit_engine_status(banner::config_loaded(config_path));

    // --- Forge + workflow + repositories (the engine half, reusing the engine
    //     service's adapters + wiring) ---
    let EngineConfig {
        daemon: daemon_config,
        forge: forge_config,
        role_tokens,
        agent_traces: engine_agent_traces,
    } = engine_config(resolved)?;
    trace_config::warn_if_engine_storage_unavailable(resolved, &engine_agent_traces);
    let forge_base_url = forge_config.base_url.clone();
    let forge_config_for_roles = forge_config.clone();
    let forge = temper_forge::factory::new_forgejo(forge_config);

    // §7 forge line, emitted after a connectivity/auth probe (current_user is the
    // forge's whoami). A failed probe is reported but not fatal — the daemon's
    // existing per-call error handling already surfaces real outages, and a slow
    // boot probe should not block the standalone loop from coming up.
    emit_engine_status(forge_banner_line(forge.as_ref(), &forge_base_url).await);

    let workflow = Arc::new(
        temper_reference_delivery::resolve_workflow(daemon_config.workflow_file.as_ref())
            .map_err(|error| format!("failed to resolve workflow: {error}"))?,
    );
    let compiled = Arc::new(workflow.compile());
    let role_limits = workflow_role_limits(&compiled);

    // §7 workflow line: name, the configured roles, and the queue count.
    let role_names: Vec<String> = daemon_config
        .roles
        .iter()
        .map(|role| role.as_str().to_string())
        .collect();
    emit_engine_status(banner::workflow(
        workflow.name(),
        &role_names,
        workflow.queues().len(),
    ));

    let repositories = resolve_repositories(forge.as_ref(), &daemon_config.repos).await?;
    let artifact_catalog = temper_engine::ConfiguredRepositoryCatalog::from_repository_set(
        &repositories,
        forge_base_url.clone(),
    )?;
    let artifact_context = Arc::new(temper_engine::ArtifactContextBundleService::new(
        forge.clone(),
        workflow.clone(),
        artifact_catalog,
        temper_engine::ArtifactContextPolicy::default(),
    ));
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

    // §7 watching line: the repos this daemon orchestrates.
    emit_engine_status(banner::watching(&repo_paths));

    let lease_ttl = chrono::Duration::from_std(daemon_config.lease_ttl)
        .map_err(|error| format!("invalid lease ttl: {error}"))?;

    // Startup recovery is a hard barrier: finish every durable create intent
    // before the daemon, worker, webhook, or polling scans can dispatch work.
    let recovery_executor = workflow.executor(forge.as_ref());
    for repo_id in &repo_ids {
        recovery_executor
            .recover_create_issue_intents(repo_id)
            .await
            .map_err(|error| {
                format!("failed to recover durable child-create intents in `{repo_id}`: {error}")
            })?;
    }

    // --- Daemon (orchestrator) on this loop, with per-role token routing ---
    let applier = result_applier(
        forge.clone(),
        forge_config_for_roles,
        workflow.clone(),
        &daemon_config,
        &role_tokens,
        lease_ttl,
    );
    let daemon = standalone_daemon(
        Arc::clone(&spawner),
        applier,
        daemon_config.worker_pools.clone(),
        role_limits,
    )
    .with_worker_pool_auth(worker_pool_auth_config(resolved)?)
    .with_artifact_context_service(artifact_context)
    .with_forge_context_reader(forge.clone(), workflow.clone())
    .begin_startup_recovery();

    // The prior in-process worker died with this standalone process, so there
    // is no live prior worker to reattach. Inventory still runs through the
    // shared deterministic reconstruction path, then converges every staged
    // claim before any new in-process worker or feed starts.
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
    let _trace_journal = start_trace_journal(&engine_agent_traces, recovered.keys().cloned());
    let orphaned = daemon.collect_startup_orphans().await;
    converge_startup_orphans(
        forge.as_ref(),
        LeasePolicy::new(lease_ttl),
        workflow.as_ref(),
        &recovered,
        &orphaned,
    )
    .await?;

    ensure_workflow_labels(forge.as_ref(), &repositories, compiled.as_ref()).await?;

    // §7 per-repo label-verification lines, hooked to the existing
    // `ensure_workflow_labels` bootstrap. The labels come from the compiled
    // workflow (the exact set just upserted on every repo).
    let label_names: Vec<String> = compiled
        .labels()
        .labels()
        .iter()
        .map(|label| label.id.to_string())
        .collect();
    for repo in &repo_paths {
        emit_engine_status(banner::repo_labels(repo, &label_names));
    }

    let startup_mechanical_config = MechanicalBackstopConfig {
        repositories: repositories.clone(),
        cadence: daemon_config
            .mechanical_cadence
            .unwrap_or(Duration::from_secs(1)),
        lease_policy: LeasePolicy::new(lease_ttl),
        pull_request_merge_observer: Some(Arc::new(StandaloneWorkstreamCleaner::new(
            daemon.clone(),
            resolved.worker.workspace_root.clone(),
        ))),
    };
    let startup_journals = (0..repositories.repositories().len())
        .map(|_| InMemoryJournal::new())
        .collect::<Vec<_>>();
    run_mechanical_backstop_tick(
        forge.as_ref(),
        workflow.as_ref(),
        (temper_engine::system_clock())(),
        &startup_mechanical_config,
        &startup_journals,
        &MechanicalScope::All,
    )
    .await
    .map_err(|error| format!("startup mechanical reconciliation failed: {error}"))?;

    daemon.complete_startup_recovery().await;
    let mut mechanical_trigger: Option<Arc<dyn HintedMechanical>> = None;
    if let Some(cadence) = daemon_config.mechanical_cadence {
        let trigger = spawn_mechanical_backstop(
            &spawner,
            forge.clone(),
            workflow.clone(),
            MechanicalBackstopConfig {
                cadence,
                ..startup_mechanical_config
            },
            temper_engine::system_clock(),
        );
        mechanical_trigger = Some(Arc::new(trigger));

        // §7 mechanical-backstop line: cadence and the repo span it covers.
        emit_engine_status(banner::mechanical_backstop(cadence, repo_paths.len()));
    }

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

    // §7 poll-backstop line is emitted only after assignment convergence and
    // the first bounded mechanical pass have both completed.
    emit_engine_status(banner::poll_backstop(
        daemon_config.poll_cadence,
        &role_names,
        repo_ids.len(),
    ));

    // --- In-process worker + agent on the same loop ---
    let provider = crate::provider::build_provider(
        resolved,
        &resolved.worker.workspace_root.join(".temper-auth"),
    )?;
    let git_base_url = temper_worker_service::git_base_url(resolved)?;
    // The worker's runtime capabilities come from the resolved worker shape. In
    // legacy configs that is `[worker] capabilities` (or the engine repo/role
    // default); when target-era pools are present, `apply_runtime_overrides`
    // narrows it to the selected standalone pool before this point.
    let capabilities: Vec<CapabilitySpec> = resolved
        .worker
        .capabilities
        .iter()
        .map(|capability| CapabilitySpec {
            role: capability.role.clone(),
            repo: capability.repo.clone(),
        })
        .collect();

    let worker_config = standalone_worker_config(
        &resolved.worker,
        capabilities,
        temper_worker_service::role_identities(resolved),
        temper_worker_service::worker_agent_trace_config(resolved),
    )?;
    trace_config::warn_if_worker_storage_unavailable(resolved, &worker_config.agent_traces);

    // The startup capacity line reports workflow-global role concurrency,
    // not this worker's advertised local capacity. Preserve compiled role
    // declaration order and retain `None` for an explicit `unlimited` token.
    let workflow_role_capacity: Vec<(String, Option<u32>)> = compiled
        .roles()
        .iter()
        .map(|role| (role.id.as_str().to_string(), role.concurrency))
        .collect();

    let pr_freshness_guard = Arc::new(InProcessPrFreshnessGuard::new(daemon.clone()));
    let transport = Arc::new(InProcessTransport::new(daemon.clone()));
    let forge_context = temper_worker::forge_context_host(
        Arc::clone(&transport),
        cx,
        worker_config.worker_id.clone(),
        worker_config.worker_auth.clone(),
    );
    let runner = Arc::new(
        InProcessAgentRunner::new(
            handle.clone(),
            provider,
            resolved.agent.max_iterations,
            resolved.agent.config_dir.clone(),
            resolved.agent.enable_subagents,
        )
        .with_tool_config(temper_worker_service::agent_tool_config(resolved))
        .with_trace_policy(worker_config.agent_traces.policy.clone())
        .with_trace_collector(worker_config.agent_traces.clone())
        .with_forge_context_host(forge_context),
    );
    let executor = Arc::new(
        CodingExecutor::new(
            CodingExecutorConfig {
                workspace_root: resolved.worker.workspace_root.clone(),
                git_base_url,
                // The worker config is the single source of truth for role
                // identities (issue #199); the executor sources them from it.
                role_identities: worker_config.role_identities.clone(),
            },
            runner,
        )
        .with_pr_freshness_guard(pr_freshness_guard),
    );

    let worker_handle = handle.clone();
    handle.spawn_with_cx(move |_cx| async move {
        let _ = run_worker_with_transport(worker_handle, worker_config, executor, transport).await;
    });

    // §7 planes-up line (engine + worker + agent all on this loop) and the
    // workflow's global per-role concurrency limits.
    emit_engine_status(banner::planes_up());
    emit_worker_status(banner::capacity(&workflow_role_capacity));

    // --- Webhook route (optional) ---
    let webhook_enabled = resolved.engine.webhook_secret_value.is_some()
        || daemon_config.webhook_secret_file.is_some();
    let daemon = if let Some(secret) = resolved.engine.webhook_secret_value.as_ref() {
        let webhook_config = Arc::new(WebhookConfig {
            secret: secret.expose_secret().trim().to_string(),
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
    } else if let Some(path) = daemon_config.webhook_secret_file.as_ref() {
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

    let local_addr = server.local_addr();
    if webhook_enabled {
        // §7 trigger line: emit only after the socket is actually bound. The
        // standalone assembly runs engine+worker+agent in one process, so this
        // listener is co-resident and webhook events fire on the daemon path.
        emit_trigger_status(banner::webhook_listener(&local_addr.to_string()));
    }
    // Bound-address detail for operators who need the listener socket; the
    // operator-facing readiness banner is the §7 `trigger: webhook listener up
    // …` line (WI-3), so this stays at debug to keep `RUST_LOG=info` to the §7
    // catalog. (The analogous line in the engine daemon handle is already debug.)
    let message = serving_debug_message(local_addr);
    tracing::debug!(
        target: "temper::engine",
        service = "engine",
        addr = %local_addr,
        "{message}"
    );

    // §7 readiness line: everything is up and the daemon is idle, watching its
    // repos. This is the operator-facing "ready" the boot block closes on.
    emit_engine_status(banner::ready(&repo_paths));

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
    daemon.release_assignments_for_shutdown().await;
    server.begin_drain(std::time::Duration::from_secs(5));
    Ok(())
}

fn standalone_daemon(
    spawner: Arc<dyn temper_engine_io::Spawner>,
    applier: Arc<dyn temper_engine::ResultApplier>,
    worker_pools: Vec<temper_engine::WorkerPoolPolicy>,
    role_limits: BTreeMap<String, u32>,
) -> Daemon {
    Daemon::with_applier_worker_pools_and_role_limits(spawner, applier, worker_pools, role_limits)
}

pub(super) fn standalone_worker_config(
    worker: &WorkerSettings,
    capabilities: Vec<CapabilitySpec>,
    role_identities: BTreeMap<String, RoleGitIdentity>,
    agent_traces: WorkerAgentTraceConfig,
) -> Result<WorkerConfig, String> {
    Ok(WorkerConfig {
        // Unused on the in-process transport, but the struct carries it.
        daemon_url: String::new(),
        worker_id: worker.worker_id.clone(),
        worker_pool: worker.selected_pool.clone(),
        worker_auth: selected_worker_auth(worker)?,
        capabilities,
        role_identities,
        max_concurrent_jobs: worker.max_concurrent_jobs,
        poll_wait: Duration::from_secs(20),
        heartbeat_interval: Duration::from_secs(10),
        agent_traces,
        executor: ExecutorSelection::Stub, // not consulted: the executor is built directly
    })
}

/// Builds the §7 forge banner line after a `current_user` connectivity/auth
/// probe.
///
/// `current_user` is the forge's whoami: a successful call proves both
/// reachability and that the configured token authenticates, and yields the bot
/// login for the line. A failed probe is non-fatal — the line degrades to an
/// `unreachable/auth failed` note rather than aborting boot, because the daemon's
/// per-request error handling already surfaces real outages, and a slow boot
/// probe should not gate the standalone loop from coming up.
async fn forge_banner_line(forge: &dyn temper_forge::Forge, url: &str) -> String {
    match forge.current_user().await {
        Ok(user) => banner::forge_reachable(url, &user.handle),
        Err(error) => format!("forge: forgejo @ {url} (unreachable or auth failed: {error})"),
    }
}

struct InProcessPrFreshnessGuard {
    daemon: Daemon,
}

impl InProcessPrFreshnessGuard {
    fn new(daemon: Daemon) -> Self {
        Self { daemon }
    }
}

impl temper_worker::PrFreshnessGuard for InProcessPrFreshnessGuard {
    fn check<'a>(
        &'a self,
        check: &'a temper_protocol_agent::PullRequestFreshness,
    ) -> Pin<Box<dyn Future<Output = Result<(), temper_worker::PrFreshnessFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            let response = self
                .daemon
                .check_pull_request_freshness(protocol_worker_freshness(check))
                .await;
            temper_worker::map_pr_freshness_response(response)
        })
    }
}

fn protocol_worker_freshness(
    check: &temper_protocol_agent::PullRequestFreshness,
) -> temper_protocol_worker::PullRequestFreshness {
    temper_protocol_worker::PullRequestFreshness {
        repository_id: check.repository_id.clone(),
        repo: check.repo.clone(),
        role: check.role.clone(),
        queue: check.queue.clone(),
        action: check.action.clone(),
        number: check.number,
        pull_request_id: check.pull_request_id.clone(),
        head_sha: check.head_sha.clone(),
        queue_condition: check.queue_condition.clone(),
        queue_labels: check.queue_labels.clone(),
    }
}

fn serving_debug_message(addr: impl std::fmt::Display) -> String {
    format!(
        "{}serving on {addr}",
        temper_log::Service::Engine.human_prefix()
    )
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
    fn serving_debug_message_uses_padded_engine_prefix() {
        let message = serving_debug_message("127.0.0.1:8314");

        assert_eq!(message, "engine:  serving on 127.0.0.1:8314");
        assert_eq!(&message[.."engine:  ".len()], "engine:  ");
    }

    #[test]
    fn standalone_daemon_preserves_distinct_workflow_role_limits() {
        temper_engine_io::block_on_with(move |_cx, handle| async move {
            let daemon = standalone_daemon(
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
