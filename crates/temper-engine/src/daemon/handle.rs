// SPDX-License-Identifier: MPL-2.0

//! The public [`Daemon`] handle: construction, job enqueue, in-process delivery,
//! scanned-role feeding, and the HTTP serving entry points.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_engine_io::http::{HttpRequestData, HttpResponder, HttpResponseData};
use temper_engine_io::{Spawner, channel, drive};
use temper_forge::{Forge, Repository, RepositoryId};
use temper_protocol_worker::{
    Artifact, PullRequestFreshness, PullRequestFreshnessResponse, WORKER_AUTHORIZATION_HEADER,
    WorkerAuth, WorkerProtocolMessage,
};
use temper_runner::WorkItem;
use temper_worker_registry::{WorkerPoolAuthConfig, WorkerPoolPolicy};
use temper_workflow::{CompiledWorkflow, RoleId, ValidatedWorkflow};

use crate::APPLY_GRACE;
use crate::applier::{NoopApplier, ResultApplier};
use crate::feed::{RoleFeedMode, job_from_work_item};

use super::Daemon;
use super::WakeScanner;
use super::executor::DaemonExecutor;
use super::machine::{DaemonCompletion, DaemonMachine};
use super::protocol::decode_in_process_reply;

impl Daemon {
    /// Create a daemon that discards applied results. The spawner is the
    /// engine's spawn capability — `Arc::new(handle)` for the production
    /// runtime, a lab spawner under simulation.
    pub fn new(spawner: Arc<dyn Spawner>) -> Self {
        Self::with_applier(spawner, Arc::new(NoopApplier))
    }

    /// Create a no-op-applier daemon with authoritative finite per-role limits.
    pub fn with_role_limits(spawner: Arc<dyn Spawner>, role_limits: BTreeMap<String, u32>) -> Self {
        Self::with_applier_and_role_limits(spawner, Arc::new(NoopApplier), role_limits)
    }

    pub fn with_applier(spawner: Arc<dyn Spawner>, applier: Arc<dyn ResultApplier>) -> Self {
        Self::with_applier_and_worker_pools(spawner, applier, Vec::new())
    }

    pub fn with_applier_and_role_limits(
        spawner: Arc<dyn Spawner>,
        applier: Arc<dyn ResultApplier>,
        role_limits: BTreeMap<String, u32>,
    ) -> Self {
        Self::with_applier_worker_pools_and_role_limits(spawner, applier, Vec::new(), role_limits)
    }

    pub fn with_applier_and_worker_pools(
        spawner: Arc<dyn Spawner>,
        applier: Arc<dyn ResultApplier>,
        worker_pools: Vec<WorkerPoolPolicy>,
    ) -> Self {
        Self::with_applier_worker_pools_and_role_limits(
            spawner,
            applier,
            worker_pools,
            BTreeMap::new(),
        )
    }

    /// Create a daemon with both worker-pool policies and authoritative finite
    /// per-role concurrency limits. Roles absent from `role_limits` remain
    /// unlimited.
    pub fn with_applier_worker_pools_and_role_limits(
        spawner: Arc<dyn Spawner>,
        applier: Arc<dyn ResultApplier>,
        worker_pools: Vec<WorkerPoolPolicy>,
        role_limits: BTreeMap<String, u32>,
    ) -> Self {
        Self::with_applier_worker_pools_role_limits_and_apply_grace(
            spawner,
            applier,
            worker_pools,
            role_limits,
            APPLY_GRACE,
        )
    }

    pub fn with_apply_grace(self, apply_grace: Duration) -> Self {
        let _ = self
            .cq
            .send(DaemonCompletion::SetApplyGrace { apply_grace });
        self
    }

    pub fn with_worker_pool_auth(self, config: WorkerPoolAuthConfig) -> Self {
        let _ = self
            .cq
            .send(DaemonCompletion::ConfigureWorkerPoolAuth { config });
        self
    }

    /// Installs the immutable configured-repository catalog used to authorize
    /// bounded worker context reads.
    pub fn with_artifact_context_catalog(
        mut self,
        catalog: crate::ConfiguredRepositoryCatalog,
    ) -> Self {
        self.artifact_catalog = Arc::new(catalog.clone());
        let _ = self
            .cq
            .send(DaemonCompletion::ConfigureArtifactContextCatalog { catalog });
        self
    }

    /// Installs the immutable artifact-context service shared by poll, webhook,
    /// direct scan, and durable-recovery enrichment. Its repository catalog is
    /// also used to authorize bounded worker context reads.
    pub fn with_artifact_context_service(
        mut self,
        service: Arc<crate::ArtifactContextBundleService>,
    ) -> Self {
        let catalog = service.catalog().clone();
        self.artifact_context = Some(service);
        self.with_artifact_context_catalog(catalog)
    }

    /// Installs the read-only Forge capability used for authenticated on-demand
    /// context requests. Call after [`with_artifact_context_catalog`](Self::with_artifact_context_catalog)
    /// or [`with_artifact_context_service`](Self::with_artifact_context_service)
    /// so the reader captures the authoritative startup catalog.
    pub fn with_forge_context_reader<F>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
    ) -> Self
    where
        F: Forge + Send + Sync + ?Sized + 'static,
    {
        let reader = super::context_reader::BoundedContextReader::new(
            forge,
            Arc::clone(&self.artifact_catalog),
            workflow,
        );
        *self
            .context_reader_slot
            .lock()
            .expect("context reader slot") = Some(Arc::new(reader));
        self
    }

    /// Returns the startup-constructed artifact-context service, when this
    /// daemon was configured for graph enrichment.
    pub fn artifact_context_service(&self) -> Option<Arc<crate::ArtifactContextBundleService>> {
        self.artifact_context.clone()
    }

    /// Closes the startup barrier. Completions are FIFO, so calling this on a
    /// newly constructed daemon guarantees subsequent enqueue/poll work cannot
    /// dispatch until [`complete_startup_recovery`](Self::complete_startup_recovery).
    pub fn begin_startup_recovery(self) -> Self {
        let _ = self.cq.send(DaemonCompletion::BeginStartupRecovery);
        self
    }

    /// Adds one deterministic job context reconstructed from durable metadata.
    pub async fn stage_recovered_job(
        &self,
        job: temper_worker_registry::RecoveredJob,
        prior_daemon_boot_id: impl Into<String>,
    ) -> Result<(), temper_worker_registry::RegistryError> {
        let (reply, rx) = temper_engine_io::oneshot();
        if self
            .cq
            .send(DaemonCompletion::StageRecoveredJob {
                job,
                daemon_boot_id: prior_daemon_boot_id.into(),
                reply,
            })
            .is_err()
        {
            return Err(temper_worker_registry::RegistryError::UnknownWorker(
                "daemon stopped during startup recovery".to_string(),
            ));
        }
        rx.recv().await.unwrap_or_else(|| {
            Err(temper_worker_registry::RegistryError::UnknownWorker(
                "daemon stopped during startup recovery".to_string(),
            ))
        })
    }

    /// Waits for the injected runtime timer while the startup barrier remains
    /// closed, giving prior workers a bounded heartbeat reattachment window.
    pub async fn wait_startup_recovery_grace(&self, delay: Duration) {
        if delay.is_zero() {
            return;
        }
        let (reply, rx) = temper_engine_io::oneshot();
        if self
            .cq
            .send(DaemonCompletion::ArmStartupRecoveryGrace { delay, reply })
            .is_ok()
        {
            let _ = rx.recv().await;
        }
    }

    /// Detaches and returns staged claims that did not receive a matching
    /// heartbeat. The startup barrier remains closed so callers can converge
    /// every returned claim in Forge before releasing dispatch.
    pub async fn collect_startup_orphans(&self) -> Vec<temper_worker_registry::RecoveredJob> {
        let (reply, rx) = temper_engine_io::oneshot();
        if self
            .cq
            .send(DaemonCompletion::CollectStartupOrphans { reply })
            .is_err()
        {
            return Vec::new();
        }
        rx.recv().await.unwrap_or_default()
    }

    /// Opens dispatch and releases deferred enqueues and long-poll waiters.
    /// Call this only after Forge convergence and startup reconciliation have
    /// completed successfully.
    pub async fn complete_startup_recovery(&self) {
        let (reply, rx) = temper_engine_io::oneshot();
        if self
            .cq
            .send(DaemonCompletion::CompleteStartupRecovery { reply })
            .is_ok()
        {
            let _ = rx.recv().await;
        }
    }

    fn with_applier_worker_pools_role_limits_and_apply_grace(
        spawner: Arc<dyn Spawner>,
        applier: Arc<dyn ResultApplier>,
        worker_pools: Vec<WorkerPoolPolicy>,
        role_limits: BTreeMap<String, u32>,
        apply_grace: Duration,
    ) -> Self {
        let (cq_tx, cq_rx) = channel();
        let scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let context_reader_slot: Arc<
            std::sync::Mutex<Option<Arc<dyn super::context_reader::ContextReader>>>,
        > = Arc::new(std::sync::Mutex::new(None));
        let executor = DaemonExecutor {
            spawner: Arc::clone(&spawner),
            cq: cq_tx.clone(),
            applier,
            scanner_slot: Arc::clone(&scanner_slot),
            context_reader_slot: Arc::clone(&context_reader_slot),
        };
        let machine = DaemonMachine::default_machine_with_worker_pools_and_role_limits(
            apply_grace,
            worker_pools,
            role_limits,
        );
        spawner.spawn_with_cx(move |cx| async move {
            let _ = drive(cx, machine, &executor, cq_rx).await;
        });

        Self {
            cq: cq_tx,
            scanner_slot,
            context_reader_slot,
            change_source_listeners: Arc::new(std::sync::Mutex::new(Vec::new())),
            artifact_catalog: Arc::new(crate::ConfiguredRepositoryCatalog::default()),
            artifact_context: None,
        }
    }

    /// Stops and joins the daemon machine without releasing its assignments.
    /// This is the abrupt-loss primitive used by deterministic restart tests;
    /// production shutdown should use [`release_assignments_for_shutdown`](Self::release_assignments_for_shutdown).
    pub async fn crash(&self) {
        let (reply, rx) = temper_engine_io::oneshot();
        if self.cq.send(DaemonCompletion::Crash { reply }).is_ok() {
            let _ = rx.recv().await;
        }
    }

    /// Signals clean shutdown by releasing every assignment still owned by this
    /// daemon boot. Crash recovery remains independent because this is only a
    /// best-effort fast path through the same conditional claim rollback.
    pub async fn release_assignments_for_shutdown(&self) {
        let (reply, rx) = temper_engine_io::oneshot();
        if self
            .cq
            .send(DaemonCompletion::ReleaseAssignmentsForShutdown { reply })
            .is_ok()
        {
            let _ = rx.recv().await;
        }
    }

    pub async fn enqueue_job(
        &self,
        job_id: impl Into<String>,
        role: impl Into<String>,
        repo: impl Into<String>,
        artifact: Artifact,
        job_payload: serde_json::Value,
    ) {
        let _ = self.cq.send(DaemonCompletion::Enqueue {
            job_id: job_id.into(),
            role: role.into(),
            repo: repo.into(),
            artifact,
            job_payload,
        });
    }

    pub(crate) async fn reconcile_pending_role_jobs(
        &self,
        repo: impl Into<String>,
        role: impl Into<String>,
        current_job_ids: BTreeSet<String>,
    ) {
        let _ = self.cq.send(DaemonCompletion::ReconcilePendingRoleJobs {
            repo: repo.into(),
            role: role.into(),
            current_job_ids,
        });
    }

    /// Deliver one worker-protocol message **in-process** and await the daemon's
    /// reply — the same path the HTTP listener drives, minus TCP and the h1 byte
    /// round-trip.
    ///
    /// This is the carrier the unified single-process worker uses: it hands the
    /// daemon machine the exact `DaemonCompletion::Http` it processes for an
    /// inbound `POST /v1/message`, carrying an `HttpResponder` backed by a
    /// oneshot we await. The machine runs `core.handle(...)` inside its serial
    /// transition and answers the oneshot — so the in-process worker gets the
    /// identical reply (and long-poll) semantics as an HTTP worker, with no
    /// lock on `DaemonCore` (the daemon loop is its sole owner) and no socket.
    ///
    /// The reply contract matches `crate::transport::Transport`:
    /// `Ok(None)` for 204/empty, `Ok(Some(_))` for a message, `Err` for a
    /// non-success status, malformed JSON, or a dropped responder.
    pub async fn deliver_protocol_message(
        &self,
        message: WorkerProtocolMessage,
    ) -> Result<Option<WorkerProtocolMessage>, String> {
        self.deliver_protocol_message_with_auth(message, None).await
    }

    /// Deliver one worker-protocol message in-process with the same auth
    /// metadata the split HTTP carrier would put in `Authorization: Bearer …`.
    pub async fn deliver_protocol_message_with_auth(
        &self,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> Result<Option<WorkerProtocolMessage>, String> {
        let (reply_tx, reply_rx) = temper_engine_io::oneshot::<HttpResponseData>();
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if let Some(auth) = auth {
            headers.push((
                WORKER_AUTHORIZATION_HEADER.to_string(),
                auth.authorization_header_value(),
            ));
        }
        let request = HttpRequestData {
            method: "POST".to_string(),
            uri: "/v1/message".to_string(),
            headers,
            body: serde_json::to_vec(&message)
                .map_err(|error| format!("serialize worker-protocol message: {error}"))?,
        };
        let _ = self.cq.send(DaemonCompletion::Http {
            request,
            responder: HttpResponder::from_oneshot(reply_tx),
        });
        match reply_rx.recv().await {
            Some(response) => decode_in_process_reply(response),
            None => Err("daemon dropped the in-process responder".to_string()),
        }
    }

    pub async fn check_pull_request_freshness(
        &self,
        check: PullRequestFreshness,
    ) -> Result<PullRequestFreshnessResponse, String> {
        let (reply_tx, reply_rx) = temper_engine_io::oneshot::<HttpResponseData>();
        let request = HttpRequestData {
            method: "POST".to_string(),
            uri: "/v1/pr-freshness".to_string(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(&check)
                .map_err(|error| format!("serialize PR freshness check: {error}"))?,
        };
        let _ = self.cq.send(DaemonCompletion::Http {
            request,
            responder: HttpResponder::from_oneshot(reply_tx),
        });
        let response = reply_rx
            .recv()
            .await
            .ok_or_else(|| "daemon dropped the PR freshness responder".to_string())?;
        if response.status != 200 {
            return Err(format!(
                "daemon PR freshness check returned HTTP {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            ));
        }
        serde_json::from_slice::<PullRequestFreshnessResponse>(&response.body)
            .map_err(|error| format!("parse PR freshness response: {error}"))
    }

    pub async fn workstream_active_by_correlation_key(&self, correlation_key: &str) -> bool {
        let correlation_key = correlation_key.trim();
        if correlation_key.is_empty() {
            return false;
        }
        let (reply, rx) = temper_engine_io::oneshot();
        if self
            .cq
            .send(DaemonCompletion::WorkstreamActive {
                correlation_key: correlation_key.to_string(),
                reply,
            })
            .is_err()
        {
            return true;
        }
        rx.recv().await.unwrap_or(true)
    }

    /// Submit one backend change hint to the daemon wake-scan path.
    ///
    /// The hint is lossy acceleration only: the installed wake scanner resolves
    /// the repository and re-runs the normal Forge-backed role scan before any
    /// work is enqueued.
    pub fn submit_change_hint(&self, hint: temper_forge::ChangeHint) {
        let _ = self.cq.send(DaemonCompletion::ChangeHint { hint });
    }

    /// Map a scanned `WorkItem` to a job and enqueue it.
    pub async fn enqueue_work_item(&self, repo: &str, item: &WorkItem) {
        let job = job_from_work_item(repo, item);
        self.enqueue_job(
            job.job_id,
            job.role,
            job.repo,
            job.artifact,
            job.job_payload,
        )
        .await;
    }

    /// Scans `repo` for `role`'s active queue work and enqueues each resulting
    /// `WorkItem` into the daemon for dispatch. Returns the number of
    /// successfully enriched and enqueued jobs; the daemon/registry dedupes
    /// already-pending or in-flight jobs by `job_id`, so repeated feeds for an
    /// unchanged ready artifact do not double-dispatch.
    ///
    /// The protocol `repo` label is the artifact repository's `owner/name` path,
    /// matching worker registered capability `repo` values.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_scanned_role_work<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repo: &RepositoryId,
        workflow: &ValidatedWorkflow,
        compiled: &CompiledWorkflow,
        now: DateTime<Utc>,
        role: &RoleId,
        mode: RoleFeedMode,
    ) -> Result<usize, temper_runner::ScanError> {
        crate::feed::enqueue_scanned_role_work(
            self, forge, repo, workflow, compiled, now, role, mode,
        )
        .await
    }

    /// Evaluates one exact artifact for a configured role set without running
    /// broad candidate discovery or repository-wide pending reconciliation.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_targeted_role_work<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repository: &Repository,
        workflow: &ValidatedWorkflow,
        compiled: &CompiledWorkflow,
        now: DateTime<Utc>,
        artifact: temper_runner::ArtifactAddress,
        roles: &[RoleId],
    ) -> Result<crate::feed::TargetedRoleFeedResult, temper_runner::ScanError> {
        crate::feed::enqueue_targeted_role_work(
            self, forge, repository, workflow, compiled, now, artifact, roles,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn queued_jobs(&self) -> Vec<crate::feed::WorkItemJob> {
        let (reply, rx) = temper_engine_io::oneshot();
        if self
            .cq
            .send(DaemonCompletion::QueuedJobs { reply })
            .is_err()
        {
            return Vec::new();
        }
        rx.recv()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|job| crate::feed::WorkItemJob {
                job_id: job.job_id,
                role: job.role,
                repo: job.repo,
                artifact: job.artifact,
                job_payload: job.job_payload,
            })
            .collect()
    }
}

/// Binds and serves the daemon's HTTP surface (`POST /v1/message`, plus
/// `POST /forgejo/webhook` when [`Daemon::with_webhook`] was used) on the
/// engine runtime. Returns once bound; the connections are served by engine
/// tasks. Use the returned server handle for the bound address and graceful
/// drain.
pub async fn serve(
    handle: &skein::runtime::RuntimeHandle,
    daemon: &Daemon,
    bind: SocketAddr,
) -> std::io::Result<temper_engine_io::http::EngineHttpServer> {
    let server = temper_engine_io::http::serve_http(
        handle,
        bind,
        daemon.cq.clone(),
        |request, responder| DaemonCompletion::Http { request, responder },
    )
    .await?;
    // §5: WI-3's `trigger: webhook listener up …` banner is the operator-facing
    // line; this raw bind line is redundant detail, kept at debug for the addr.
    let local_addr = server.local_addr();
    let message = serving_debug_message(local_addr);
    tracing::debug!(
        target: "temper::engine",
        service = "engine",
        addr = %local_addr,
        "{message}"
    );
    Ok(server)
}

fn serving_debug_message(addr: impl std::fmt::Display) -> String {
    format!(
        "{}serving on {addr}",
        temper_log::Service::Engine.human_prefix()
    )
}

/// The daemon's h1 request handler — the same request→completion conversion
/// [`serve`] installs on the TCP listener, exposed so in-memory simulation
/// gateways can serve connections against the daemon's queue.
pub fn h1_handler(daemon: &Daemon) -> temper_engine_io::http::H1CompletionHandler {
    temper_engine_io::http::h1_completion_handler(daemon.cq.clone(), |request, responder| {
        DaemonCompletion::Http { request, responder }
    })
}

#[cfg(test)]
mod tests {
    use super::serving_debug_message;

    #[test]
    fn serving_debug_message_uses_padded_engine_prefix() {
        let message = serving_debug_message("127.0.0.1:8314");

        assert_eq!(message, "engine:  serving on 127.0.0.1:8314");
        assert_eq!(&message[.."engine:  ".len()], "engine:  ");
    }
}
