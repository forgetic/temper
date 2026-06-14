// SPDX-License-Identifier: MPL-2.0

//! `temper run` — daemon + worker + agent on one single-threaded event loop.
//!
//! Assembles all three planes in one process: a [`Daemon`] (the orchestrator,
//! routing + applying results), an in-process worker driving an
//! [`InProcessAgentRunner`], wired by an [`InProcessTransport`] (no TCP, no HTTP
//! byte round-trip), and the coding agent running as per-job futures on the same
//! loop. The agent's git/tool work offloads to the blocking pool, so the loop
//! thread is never blocked while a job runs.
//!
//! This is the solo-dev UX: one binary, one process, point it at a forge and go.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use skein::runtime::RuntimeHandle;
use temper_agent_runtime::ProviderConfig;
use temper_daemon::{
    Daemon, DaemonRunConfig, ForgeApplier, LeaseApplier, MechanicalBackstopConfig,
    PollBackstopConfig, RepositorySet, RepositoryTarget, ResultApplier, RoleFeedMode,
    RoleFeedTarget, RoleRoutingApplier, config::role_tokens_from_env, spawn_mechanical_backstop,
    spawn_poll_backstop,
};
use temper_forge::{Forge, RepositoryId, RepositoryPath, UpsertLabel};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_worker_orchestrator::{
    CapabilitySpec, CodingExecutor, CodingExecutorConfig, ExecutorSelection, WorkerConfig,
    role_identities_from_env, run_worker_with_transport,
};
use temper_workflow::{CompiledWorkflow, LeasePolicy, RoleId};

use crate::run::{InProcessAgentRunner, InProcessTransport};

/// Configuration for the all-in-one mode: the daemon's run config (forge, repos,
/// roles, cadences) plus the worker/agent surface.
pub struct AllInOneConfig {
    /// Daemon orchestration config (reused from `temper daemon`).
    pub daemon: DaemonRunConfig,
    /// LLM provider/auth for the in-process agent.
    pub provider: ProviderConfig,
    /// Root for per-(repo, role) agent workspaces.
    pub workspace_root: PathBuf,
    /// Git base URL the agent pushes branches to (usually the same forge).
    pub git_base_url: String,
    /// Max agent iterations per job.
    pub max_iterations: usize,
    /// Optional agent config-dir (prompt overlays).
    pub config_dir: Option<PathBuf>,
    /// Enable the agent's investigate/delegate sub-agents.
    pub enable_subagents: bool,
    /// Worker identity.
    pub worker_id: String,
}

/// Run daemon + worker + agent to (effective) completion on one loop. Returns
/// when a shutdown signal fires.
pub async fn run_all_in_one(handle: RuntimeHandle, config: AllInOneConfig) -> Result<(), String> {
    let AllInOneConfig {
        daemon: daemon_config,
        provider,
        workspace_root,
        git_base_url,
        max_iterations,
        config_dir,
        enable_subagents,
        worker_id,
    } = config;

    let spawner: Arc<dyn temper_io_engine::Spawner> = Arc::new(handle.clone());

    // --- Forge + workflow + repositories (shared daemon setup) ---
    let forge_config =
        ForgejoConfig::from_env().map_err(|error| format!("Forgejo config: {error}"))?;
    let forge_base_url = forge_config.base_url.clone();
    let forge = Arc::new(ForgejoForge::new(forge_config));
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
    // The worker registers capabilities keyed by the artifact repo's `owner/name`
    // path — the same key the daemon assigns jobs on (NOT the RepositoryId). Capture
    // it before `repositories` is moved into the mechanical backstop config.
    let repo_paths: Vec<String> = repositories
        .repositories()
        .iter()
        .map(|repository| repository.display_path())
        .collect();
    let lease_ttl = chrono::Duration::from_std(daemon_config.lease_ttl)
        .map_err(|error| format!("invalid --lease-ttl-secs: {error}"))?;

    // --- Daemon (orchestrator) on this loop ---
    // Per-role Forge token routing: PRs/comments a role's job produces are
    // applied with that role's identity (TEMPER_FORGEJO_TOKEN_<ROLE>), not the
    // admin default — exactly as the split daemon does.
    let applier = result_applier(
        forge.clone(),
        &forge_base_url,
        workflow.clone(),
        &daemon_config,
        lease_ttl,
    );
    let daemon = Daemon::with_applier(Arc::clone(&spawner), applier);

    // The poll backstop discovers role work and enqueues it into the daemon —
    // the same cadence machine the split daemon runs.
    spawn_poll_backstop(
        &spawner,
        daemon.clone(),
        forge.clone(),
        workflow.clone(),
        compiled.clone(),
        PollBackstopConfig {
            targets: role_feed_targets(&repo_ids, &daemon_config.roles),
            cadence: daemon_config.poll_cadence,
        },
        temper_daemon::system_clock(),
    );

    // The mechanical backstop runs the workflow's automation transitions (stamp
    // raw intake ready, land CI-green PRs) — without it, seeded intake never
    // advances to the role queues, so role workers get no work. Enabled when a
    // mechanical cadence is configured (the split daemon does the same).
    if let Some(cadence) = daemon_config.mechanical_cadence {
        ensure_workflow_labels(forge.as_ref(), &repositories, compiled.as_ref()).await?;
        spawn_mechanical_backstop(
            &spawner,
            forge.clone(),
            workflow.clone(),
            MechanicalBackstopConfig {
                repositories,
                cadence,
                lease_policy: LeasePolicy::new(lease_ttl),
            },
            temper_daemon::system_clock(),
        );
    }

    // --- In-process worker + agent on the same loop ---
    let role_identities = role_identities_from_env(
        daemon_config
            .roles
            .iter()
            .map(|role| role.as_str().to_string()),
        std::env::vars(),
    )?;
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
        max_iterations,
        config_dir,
        enable_subagents,
    ));
    let executor = Arc::new(
        CodingExecutor::new(
            CodingExecutorConfig {
                workspace_root,
                git_base_url,
                role_identities,
            },
            runner,
        )
        .with_progress_sink(Arc::new(InProcessProgressSink::new(
            handle.clone(),
            daemon.clone(),
            worker_id.clone(),
        ))),
    );

    let worker_config = WorkerConfig {
        // Unused on the in-process transport, but the struct carries it.
        daemon_url: String::new(),
        worker_id: worker_id.clone(),
        capabilities,
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_secs(20),
        heartbeat_interval: Duration::from_secs(10),
        executor: ExecutorSelection::Stub, // not consulted: we build the executor directly
    };
    let transport = Arc::new(InProcessTransport::new(daemon.clone()));

    // Spawn the worker loop as a task; it registers + polls the co-resident
    // daemon over the in-process transport and runs jobs through the in-process
    // agent. The daemon loop, the poll backstop, the worker loop, and per-job
    // agent/git futures all interleave on this one thread.
    let worker_handle = handle.clone();
    handle.spawn_with_cx(move |_cx| async move {
        let _ = run_worker_with_transport(worker_handle, worker_config, executor, transport).await;
    });

    // --- Wait for shutdown ---
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
    Ok(())
}

/// Relay agent step-progress to the co-resident daemon in-process (no HTTP),
/// mirroring the split worker's `DaemonRelayProgressSink` but over the in-memory
/// carrier. Fire-and-forget per the sink contract: the delivery is spawned as a
/// detached task so a slow/failed apply never stalls or fails the agent turn.
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

impl temper_worker_orchestrator::ProgressSink for InProcessProgressSink {
    fn report(&self, progress: temper_agent_protocol::StepProgress) {
        let message = temper_worker_orchestrator::progress_message(&self.worker_id, &progress);
        let daemon = self.daemon.clone();
        // Fire-and-forget on a detached task: the daemon applies progress
        // idempotently; a failure here is observability loss, not a turn failure.
        self.handle.spawn_with_cx(move |_cx| async move {
            let _ = daemon.deliver_protocol_message(message).await;
        });
    }
}

/// Build the result applier with per-role Forge token routing, mirroring the
/// split daemon: a role's applied results (PRs, comments) use that role's
/// identity when `TEMPER_FORGEJO_TOKEN_<ROLE>` is set, falling back to the
/// admin/default forge otherwise.
fn result_applier(
    default_forge: Arc<ForgejoForge>,
    forge_base_url: &str,
    workflow: Arc<temper_workflow::ValidatedWorkflow>,
    config: &DaemonRunConfig,
    lease_ttl: chrono::Duration,
) -> Arc<dyn ResultApplier> {
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
    for role in &config.roles {
        let role = role.as_str().to_string();
        if let Some(token) = role_tokens.get(&role) {
            let role_forge = Arc::new(ForgejoForge::new(ForgejoConfig::new(
                forge_base_url.to_string(),
                token.clone(),
            )));
            let role_chain = applier_chain(
                role_forge,
                workflow.clone(),
                config.daemon_id.clone(),
                lease_ttl,
            );
            routing = routing.with_route(role.clone(), role_chain);
        }
    }
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
        temper_daemon::system_clock(),
    ))
}

/// Upsert every workflow label into each repository so the mechanical backstop's
/// label-driven automation has the labels to apply. (Same as the split daemon.)
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

async fn resolve_repositories<F: Forge + ?Sized>(
    forge: &F,
    repos: &[RepositoryPath],
) -> Result<RepositorySet, String> {
    let mut resolved = Vec::with_capacity(repos.len());
    for path in repos {
        let repository = forge
            .get_repository_by_path(path)
            .await
            .map_err(|error| {
                format!(
                    "repository {}/{} lookup failed: {error}",
                    path.owner, path.name
                )
            })?
            .ok_or_else(|| format!("repository {}/{} not found", path.owner, path.name))?;
        resolved.push(RepositoryTarget::new(
            repository.id,
            RepositoryPath::new(repository.owner, repository.name),
        ));
    }
    Ok(RepositorySet::new(resolved))
}

fn role_feed_targets(repos: &[RepositoryId], roles: &[RoleId]) -> Vec<RoleFeedTarget> {
    let mut targets = Vec::with_capacity(repos.len() * roles.len());
    for repo in repos {
        for role in roles {
            targets.push(RoleFeedTarget {
                repo: repo.clone(),
                role: role.clone(),
                mode: RoleFeedMode::Normal,
            });
        }
    }
    targets
}
