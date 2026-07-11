// SPDX-License-Identifier: MPL-2.0

//! Engine service runtime wiring.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use temper_config::{ExposeSecret, Resolved};
use temper_engine::{
    Daemon, DaemonRunConfig, EngineConfig, HintedMechanical, MechanicalBackstopConfig,
    MechanicalScope, PollBackstopConfig, RepositorySet, RoleFeedMode, RoleFeedTarget,
    WebhookConfig, spawn_mechanical_backstop, spawn_poll_backstop,
};
use temper_forge::{
    Forge, ForgeError, ForgeResult, IssueQuery, IssueState, PullRequest, PullRequestQuery,
    PullRequestState, RepositoryId, RepositoryPath,
};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, DurableAssignment, LeaseManager, LeasePolicy, METADATA_BEGIN,
    ValidatedWorkflow, parse_metadata_block,
};

use crate::{
    engine_config, ensure_workflow_labels, resolve_repositories, result_applier, role_feed_targets,
    worker_pool_auth_config, workflow_role_limits,
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
    let role_limits = workflow_role_limits(&compiled);
    let (repositories, repo_ids) = resolve_repo_targets(forge.as_ref(), &config.repos).await?;
    let normal_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Normal);
    let wake_targets = role_feed_targets(&repo_ids, &config.roles, RoleFeedMode::Wake);
    let lease_ttl = lease_ttl(&config)?;

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
    let server = temper_engine::serve(&handle, &daemon, config.bind)
        .await
        .map_err(|error| format!("serve failed: {error}"))?;
    if !recovered.is_empty() {
        daemon
            .wait_startup_recovery_grace(std::time::Duration::from_secs(10))
            .await;
    }
    let orphaned = daemon.finish_startup_recovery().await;
    converge_startup_orphans(
        forge.as_ref(),
        LeasePolicy::new(lease_ttl),
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
    // Run the first reconciliation pass to completion before normal role feeds
    // are opened; the cadence loop remains the bounded convergence backstop.
    trigger.run(MechanicalScope::All).await;
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

#[derive(Clone)]
pub struct RecoveredClaim {
    repo: RepositoryId,
    target: ArtifactSource,
    assignment: DurableAssignment,
}

pub async fn stage_startup_assignments(
    daemon: &Daemon,
    forge: &dyn Forge,
    repos: &[RepositoryId],
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    policy: LeasePolicy,
    now: chrono::DateTime<Utc>,
) -> Result<BTreeMap<String, RecoveredClaim>, String> {
    const MAX_RECOVERY_CANDIDATES: usize = 1_000;
    let mut candidates = Vec::new();
    for repo in repos {
        let issues = forge
            .list_issues(
                repo,
                IssueQuery {
                    state: Some(IssueState::Open),
                    body_contains: Some(METADATA_BEGIN.to_string()),
                    ..IssueQuery::default()
                },
            )
            .await
            .map_err(|error| format!("startup issue inventory failed for {repo}: {error}"))?;
        candidates.extend(issues.into_iter().map(|issue| {
            (
                repo.clone(),
                ArtifactSource::Issue {
                    number: issue.number,
                },
                issue.body,
            )
        }));
        let pull_requests = forge
            .list_pull_requests(
                repo,
                PullRequestQuery {
                    state: Some(PullRequestState::Open),
                    body_contains: Some(METADATA_BEGIN.to_string()),
                    ..PullRequestQuery::default()
                },
            )
            .await;
        let pull_requests = startup_pull_inventory(repo, pull_requests)?;
        candidates.extend(pull_requests.into_iter().map(|pull_request| {
            (
                repo.clone(),
                ArtifactSource::PullRequest {
                    number: pull_request.number,
                },
                pull_request.body,
            )
        }));
        if candidates.len() > MAX_RECOVERY_CANDIDATES {
            return Err(format!(
                "startup recovery candidate limit exceeded ({MAX_RECOVERY_CANDIDATES})"
            ));
        }
    }
    candidates.sort_by_key(|(repo, target, _)| {
        let (kind, number) = match target {
            ArtifactSource::Issue { number } => (0_u8, number.get()),
            ArtifactSource::PullRequest { number } => (1_u8, number.get()),
        };
        (repo.clone(), kind, number)
    });

    let mut staged = BTreeMap::new();
    for (repo, target, body) in candidates {
        let Some(metadata) = parse_metadata_block(&body)
            .map_err(|error| format!("invalid workflow metadata on {repo} {target:?}: {error}"))?
        else {
            continue;
        };
        let Some(assignment) = metadata.assignment else {
            continue;
        };
        let Some(job_id) = assignment
            .job_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            quarantine_invalid_assignment(forge, policy, &repo, target, &assignment).await?;
            continue;
        };
        let Some(worker_id) = assignment
            .worker_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            quarantine_invalid_assignment(forge, policy, &repo, target, &assignment).await?;
            continue;
        };
        let Some(prior_boot) = assignment
            .daemon_boot_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            quarantine_invalid_assignment(forge, policy, &repo, target, &assignment).await?;
            continue;
        };
        let Some(expires_at) = assignment
            .expires_at
            .or_else(|| metadata.lease.as_ref().map(|lease| lease.expires_at))
        else {
            quarantine_invalid_assignment(forge, policy, &repo, target, &assignment).await?;
            continue;
        };
        let claim = RecoveredClaim {
            repo: repo.clone(),
            target,
            assignment: assignment.clone(),
        };
        if expires_at <= now {
            LeaseManager::new(forge, policy)
                .rollback_assignment(&repo, target, &assignment)
                .await
                .map_err(|error| format!("could not recover expired claim {job_id}: {error}"))?;
            continue;
        }
        let job = match temper_engine::recovered_job_from_assignment(
            forge,
            &repo,
            target,
            &assignment,
            workflow,
            compiled,
        )
        .await
        {
            Ok(job) => job,
            Err(reason) => {
                tracing::warn!(job_id = %job_id, %reason, "quarantining impossible durable assignment");
                quarantine_invalid_assignment(forge, policy, &repo, target, &assignment).await?;
                continue;
            }
        };
        daemon
            .stage_recovered_job(
                temper_engine::RecoveredJob {
                    job_id: job.job_id,
                    worker_id,
                    role: job.role,
                    repo: job.repo,
                    artifact: job.artifact,
                    job_payload: job.job_payload,
                },
                prior_boot,
            )
            .await
            .map_err(|error| format!("could not stage recovered claim {job_id}: {error:?}"))?;
        staged.insert(job_id, claim);
    }
    Ok(staged)
}

fn startup_pull_inventory(
    repo: &RepositoryId,
    result: ForgeResult<Vec<PullRequest>>,
) -> Result<Vec<PullRequest>, String> {
    match result {
        Ok(pull_requests) => Ok(pull_requests),
        // Forgejo reports its /pulls collection as 404 until a repository has
        // a Git history. The issue inventory immediately before this call
        // succeeded, so the repository itself is known to exist and an absent
        // PR collection is equivalent to an empty recovery inventory.
        Err(ForgeError::NotFound(error)) => {
            tracing::debug!(%repo, %error, "startup PR collection is not available yet");
            Ok(Vec::new())
        }
        Err(error) => Err(format!("startup PR inventory failed for {repo}: {error}")),
    }
}

async fn quarantine_invalid_assignment(
    forge: &dyn Forge,
    policy: LeasePolicy,
    repo: &RepositoryId,
    target: ArtifactSource,
    assignment: &DurableAssignment,
) -> Result<(), String> {
    LeaseManager::new(forge, policy)
        .quarantine_assignment(repo, target, assignment)
        .await
        .map_err(|error| format!("could not quarantine impossible claim on {repo}: {error}"))
}

pub async fn converge_startup_orphans(
    forge: &dyn Forge,
    policy: LeasePolicy,
    recovered: &BTreeMap<String, RecoveredClaim>,
    orphaned: &[temper_engine::RecoveredJob],
) -> Result<(), String> {
    for orphan in orphaned {
        let claim = recovered.get(&orphan.job_id).ok_or_else(|| {
            format!(
                "startup recovery lost durable context for {}",
                orphan.job_id
            )
        })?;
        LeaseManager::new(forge, policy)
            .rollback_assignment(&claim.repo, claim.target, &claim.assignment)
            .await
            .map_err(|error| {
                format!(
                    "could not converge orphaned claim {}: {error}",
                    orphan.job_id
                )
            })?;
    }
    Ok(())
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
    fn missing_pull_collection_is_empty_during_startup_inventory() {
        let repo = RepositoryId::new("forgejo:acme/empty");
        let result = startup_pull_inventory(
            &repo,
            Err(ForgeError::NotFound(
                "pull collection unavailable".to_string(),
            )),
        )
        .expect("an absent PR collection is empty for an existing repository");

        assert!(result.is_empty());
    }

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
