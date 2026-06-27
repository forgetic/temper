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
mod banner;
mod hooks;
mod transport;

pub use agent_runner::InProcessAgentRunner;
pub use transport::InProcessTransport;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use skein::runtime::RuntimeHandle;
use temper_config::{ExposeSecret, Resolved, WorkerSettings};
use temper_engine::{
    Daemon, EngineConfig, HintedMechanical, MechanicalBackstopConfig, PollBackstopConfig,
    PullRequestMergeObserver, RoleFeedMode, WebhookConfig, spawn_mechanical_backstop,
    spawn_poll_backstop,
};
use temper_engine_service::{
    engine_config, ensure_workflow_labels, resolve_repositories, result_applier, role_feed_targets,
};
use temper_forge::{PullRequest, PullRequestState, RepositoryId};
use temper_log::emit::{emit_engine_status, emit_trigger_status, emit_worker_status};
use temper_worker::{
    CapabilitySpec, CodingExecutor, CodingExecutorConfig, ExecutorSelection, RoleGitIdentity,
    WorkerConfig, run_worker_with_transport,
};
use temper_workflow::LeasePolicy;

/// Runs the standalone daemon on the skein runtime until SIGINT/SIGTERM.
///
/// `config_path` is the on-disk config the deployment loaded from (for the §7
/// `config loaded from <path>` startup line), or `None` when the deployment was
/// assembled from env/defaults only.
pub fn run(resolved: &Resolved, config_path: Option<&Path>) -> Result<(), String> {
    let resolved = resolved.clone();
    let config_path = config_path.map(Path::to_path_buf);
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        run_async(handle, &resolved, config_path.as_deref()).await
    })
}

async fn run_async(
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
    } = engine_config(resolved)?;
    let forge_base_url = forge_config.base_url.clone();
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

    // --- Daemon (orchestrator) on this loop, with per-role token routing ---
    let applier = result_applier(
        forge.clone(),
        forge_base_url,
        workflow.clone(),
        &daemon_config,
        &role_tokens,
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

    // §7 poll-backstop line: cadence, the roles whose feeds it scans (the
    // configured roles drive the normal-mode poll backstop), and the repo span.
    emit_engine_status(banner::poll_backstop(
        daemon_config.poll_cadence,
        &role_names,
        repo_ids.len(),
    ));

    let mut mechanical_trigger: Option<Arc<dyn HintedMechanical>> = None;
    if let Some(cadence) = daemon_config.mechanical_cadence {
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

        let trigger = spawn_mechanical_backstop(
            &spawner,
            forge.clone(),
            workflow.clone(),
            MechanicalBackstopConfig {
                repositories,
                cadence,
                lease_policy: LeasePolicy::new(lease_ttl),
                pull_request_merge_observer: Some(Arc::new(StandaloneWorkstreamCleaner::new(
                    daemon.clone(),
                    resolved.worker.workspace_root.clone(),
                ))),
            },
            temper_engine::system_clock(),
        );
        mechanical_trigger = Some(Arc::new(trigger));

        // §7 mechanical-backstop line: cadence and the repo span it covers.
        emit_engine_status(banner::mechanical_backstop(cadence, repo_paths.len()));
    }

    // --- In-process worker + agent on the same loop ---
    let provider = crate::provider::build_provider(
        resolved,
        &resolved.worker.workspace_root.join(".temper-auth"),
    )?;
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

    let worker_config = standalone_worker_config(
        &resolved.worker,
        capabilities,
        temper_worker_service::role_identities(resolved),
    );

    // Per-role concurrency for the §7 `capacity:` line — the standalone worker
    // runs `max_concurrent_jobs` per role, shared across all repos. Captured
    // before `worker_config` is moved into the worker task.
    let per_role_capacity = worker_config.max_concurrent_jobs as u64;

    let pr_freshness_guard = Arc::new(InProcessPrFreshnessGuard::new(daemon.clone()));
    let runner = Arc::new(
        InProcessAgentRunner::new(
            handle.clone(),
            provider,
            resolved.agent.max_iterations,
            resolved.agent.config_dir.clone(),
            resolved.agent.enable_subagents,
        )
        .with_checkpoints_enabled(resolved.agent.enable_checkpoints)
        .with_pr_freshness_guard(pr_freshness_guard.clone()),
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
        .with_pr_freshness_guard(pr_freshness_guard)
        .with_progress_sink(Arc::new(InProcessProgressSink::new(
            handle.clone(),
            daemon.clone(),
            resolved.worker.worker_id.clone(),
        ))),
    );
    let transport = Arc::new(InProcessTransport::new(daemon.clone()));

    let worker_handle = handle.clone();
    handle.spawn_with_cx(move |_cx| async move {
        let _ = run_worker_with_transport(worker_handle, worker_config, executor, transport).await;
    });

    // §7 planes-up line (engine + worker + agent all on this loop) and the
    // worker capacity line (per-role concurrency, shared across all repos).
    emit_engine_status(banner::planes_up());
    emit_worker_status(banner::capacity(&role_names, per_role_capacity));

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
    server.begin_drain(std::time::Duration::from_secs(5));
    Ok(())
}

pub(super) fn standalone_worker_config(
    worker: &WorkerSettings,
    capabilities: Vec<CapabilitySpec>,
    role_identities: BTreeMap<String, RoleGitIdentity>,
) -> WorkerConfig {
    WorkerConfig {
        // Unused on the in-process transport, but the struct carries it.
        daemon_url: String::new(),
        worker_id: worker.worker_id.clone(),
        capabilities,
        role_identities,
        max_concurrent_jobs: worker.max_concurrent_jobs,
        poll_wait: Duration::from_secs(20),
        heartbeat_interval: Duration::from_secs(10),
        executor: ExecutorSelection::Stub, // not consulted: the executor is built directly
    }
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

const ENGINEER_WORKSTREAM_ROLE: &str = "engineer";

#[derive(Clone, Debug, Eq, PartialEq)]
enum MergedPullRequestWorkstream {
    Cleanup { correlation_key: String },
    SkipUnmerged,
    SkipMissingCorrelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StandaloneWorkstreamCleanupOutcome {
    SkippedUnmerged,
    SkippedMissingCorrelationKey,
    Workspace(temper_worker::ScopedWorkspaceCleanupOutcome),
}

struct StandaloneWorkstreamCleaner {
    daemon: Daemon,
    workspace_root: PathBuf,
}

impl StandaloneWorkstreamCleaner {
    fn new(daemon: Daemon, workspace_root: PathBuf) -> Self {
        Self {
            daemon,
            workspace_root,
        }
    }
}

#[async_trait::async_trait]
impl PullRequestMergeObserver for StandaloneWorkstreamCleaner {
    async fn pull_request_merged(&self, pull_request: &PullRequest) {
        match cleanup_engineer_workstream_for_merged_pr(
            &self.daemon,
            self.workspace_root.clone(),
            pull_request,
        )
        .await
        {
            Ok(outcome) => {
                tracing::debug!(
                    target: "temper::worker",
                    repo = pull_request.repo_id.as_str(),
                    pull_request = pull_request.number.get(),
                    ?outcome,
                    "standalone workstream cleanup after PR merge"
                );
            }
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    repo = pull_request.repo_id.as_str(),
                    pull_request = pull_request.number.get(),
                    %error,
                    "standalone workstream cleanup after PR merge failed"
                );
            }
        }
    }
}

async fn cleanup_engineer_workstream_for_merged_pr(
    daemon: &Daemon,
    workspace_root: PathBuf,
    pull_request: &PullRequest,
) -> Result<StandaloneWorkstreamCleanupOutcome, String> {
    let correlation_key = match merged_pull_request_workstream(pull_request)
        .map_err(|error| format!("parse pull request workflow metadata: {error}"))?
    {
        MergedPullRequestWorkstream::Cleanup { correlation_key } => correlation_key,
        MergedPullRequestWorkstream::SkipUnmerged => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedUnmerged);
        }
        MergedPullRequestWorkstream::SkipMissingCorrelationKey => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey);
        }
    };
    let active = daemon
        .workstream_active_by_correlation_key(&correlation_key)
        .await;
    let outcome = temper_worker::cleanup_scoped_workspace(
        workspace_root,
        ENGINEER_WORKSTREAM_ROLE.to_string(),
        correlation_key,
        active,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(StandaloneWorkstreamCleanupOutcome::Workspace(outcome))
}

#[cfg(test)]
fn cleanup_engineer_workstream_for_merged_pr_sync(
    workspace_root: &Path,
    pull_request: &PullRequest,
    active: bool,
) -> Result<StandaloneWorkstreamCleanupOutcome, String> {
    let correlation_key = match merged_pull_request_workstream(pull_request)
        .map_err(|error| format!("parse pull request workflow metadata: {error}"))?
    {
        MergedPullRequestWorkstream::Cleanup { correlation_key } => correlation_key,
        MergedPullRequestWorkstream::SkipUnmerged => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedUnmerged);
        }
        MergedPullRequestWorkstream::SkipMissingCorrelationKey => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey);
        }
    };
    let outcome = temper_worker::cleanup_scoped_workspace_sync(
        workspace_root,
        ENGINEER_WORKSTREAM_ROLE,
        &correlation_key,
        active,
    )
    .map_err(|error| error.to_string())?;
    Ok(StandaloneWorkstreamCleanupOutcome::Workspace(outcome))
}

fn merged_pull_request_workstream(
    pull_request: &PullRequest,
) -> Result<MergedPullRequestWorkstream, temper_workflow::MetadataError> {
    if pull_request.state != PullRequestState::Merged || pull_request.merge.is_none() {
        return Ok(MergedPullRequestWorkstream::SkipUnmerged);
    }
    let Some(metadata) = temper_workflow::parse_metadata_block(&pull_request.body)? else {
        return Ok(MergedPullRequestWorkstream::SkipMissingCorrelationKey);
    };
    let Some(correlation_key) = metadata
        .correlation_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
    else {
        return Ok(MergedPullRequestWorkstream::SkipMissingCorrelationKey);
    };
    Ok(MergedPullRequestWorkstream::Cleanup { correlation_key })
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
    fn report(&self, progress: temper_protocol_agent::StepProgress) {
        let message = temper_worker::progress_message(&self.worker_id, &progress);
        let daemon = self.daemon.clone();
        self.handle.spawn_with_cx(move |_cx| async move {
            let _ = daemon.deliver_protocol_message(message).await;
        });
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
    use chrono::{DateTime, Utc};
    use temper_forge::{
        BranchRef, ItemNumber, MergeMethod, MergeRecord, PullRequestId, RepositoryId, UserId,
        Version,
    };
    use temper_workflow::{WorkflowMetadata, render_metadata_block};

    #[test]
    fn serving_debug_message_uses_padded_engine_prefix() {
        let message = serving_debug_message("127.0.0.1:8314");

        assert_eq!(message, "engine:  serving on 127.0.0.1:8314");
        assert_eq!(&message[.."engine:  ".len()], "engine:  ");
    }

    #[test]
    fn standalone_cleanup_removes_merged_engineer_workstream() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temper_worker::scoped_workspace_root(
            temp.path(),
            ENGINEER_WORKSTREAM_ROLE,
            "pr-for-code-477",
        )
        .expect("scoped path");
        std::fs::create_dir_all(path.join("temper")).expect("workspace dir");

        let outcome = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request(PullRequestState::Merged, Some("pr-for-code-477"), true),
            false,
        )
        .expect("cleanup");

        assert_eq!(
            outcome,
            StandaloneWorkstreamCleanupOutcome::Workspace(
                temper_worker::ScopedWorkspaceCleanupOutcome::Removed { path: path.clone() }
            )
        );
        assert!(!path.exists());
    }

    #[test]
    fn standalone_cleanup_skips_open_and_closed_unmerged_pull_requests() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temper_worker::scoped_workspace_root(
            temp.path(),
            ENGINEER_WORKSTREAM_ROLE,
            "pr-for-code-477",
        )
        .expect("scoped path");
        std::fs::create_dir_all(&path).expect("workspace dir");

        for state in [PullRequestState::Open, PullRequestState::Closed] {
            let outcome = cleanup_engineer_workstream_for_merged_pr_sync(
                temp.path(),
                &pull_request(state, Some("pr-for-code-477"), false),
                false,
            )
            .expect("cleanup");
            assert_eq!(outcome, StandaloneWorkstreamCleanupOutcome::SkippedUnmerged);
            assert!(path.exists());
        }
    }

    #[test]
    fn standalone_cleanup_skips_missing_metadata_or_correlation_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temper_worker::scoped_workspace_root(
            temp.path(),
            ENGINEER_WORKSTREAM_ROLE,
            "pr-for-code-477",
        )
        .expect("scoped path");
        std::fs::create_dir_all(&path).expect("workspace dir");

        let no_metadata = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request_with_body(PullRequestState::Merged, "human body", true),
            false,
        )
        .expect("cleanup");
        assert_eq!(
            no_metadata,
            StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey
        );
        assert!(path.exists());

        let empty_key = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request(PullRequestState::Merged, Some("  "), true),
            false,
        )
        .expect("cleanup");
        assert_eq!(
            empty_key,
            StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey
        );
        assert!(path.exists());
    }

    #[test]
    fn standalone_cleanup_preserves_active_workstream() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temper_worker::scoped_workspace_root(
            temp.path(),
            ENGINEER_WORKSTREAM_ROLE,
            "pr-for-code-477",
        )
        .expect("scoped path");
        std::fs::create_dir_all(&path).expect("workspace dir");

        let outcome = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request(PullRequestState::Merged, Some("pr-for-code-477"), true),
            true,
        )
        .expect("cleanup");

        assert_eq!(
            outcome,
            StandaloneWorkstreamCleanupOutcome::Workspace(
                temper_worker::ScopedWorkspaceCleanupOutcome::SkippedActive { path: path.clone() }
            )
        );
        assert!(path.exists());
    }

    fn pull_request(
        state: PullRequestState,
        correlation_key: Option<&str>,
        merged: bool,
    ) -> PullRequest {
        let body = correlation_key
            .map(|correlation_key| {
                render_metadata_block(&WorkflowMetadata {
                    correlation_key: Some(correlation_key.to_string()),
                    ..WorkflowMetadata::default()
                })
            })
            .unwrap_or_default();
        pull_request_with_body(state, &body, merged)
    }

    fn pull_request_with_body(state: PullRequestState, body: &str, merged: bool) -> PullRequest {
        let repo_id = RepositoryId::new("forgejo://ai/temper");
        PullRequest {
            id: PullRequestId::new("pr-477"),
            repo_id: repo_id.clone(),
            number: ItemNumber::new(477),
            title: "Implement cleanup".to_string(),
            body: body.to_string(),
            state,
            author_id: UserId::new("engineer"),
            source: BranchRef {
                repository_id: repo_id.clone(),
                branch: "agent/pr-for-code-477".to_string(),
            },
            target: BranchRef {
                repository_id: repo_id,
                branch: "main".to_string(),
            },
            head_sha: Some("abc123".to_string()),
            base_sha: Some("def456".to_string()),
            labels: Vec::new(),
            assignees: Vec::new(),
            requested_reviewers: Vec::new(),
            dependencies: Vec::new(),
            merge: merged.then(|| MergeRecord {
                method: MergeMethod::Squash,
                commit_sha: "abc123def456".to_string(),
                merged_by: UserId::new("maintainer"),
                merged_at: ts("2026-06-01T00:00:00Z"),
            }),
            version: Version::default(),
            created_at: ts("2026-05-31T00:00:00Z"),
            updated_at: ts("2026-06-01T00:00:00Z"),
            closed_at: merged.then(|| ts("2026-06-01T00:00:00Z")),
        }
    }

    fn ts(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("timestamp parses")
            .with_timezone(&Utc)
    }
}
