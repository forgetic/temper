// SPDX-License-Identifier: MPL-2.0

//! The public [`Daemon`] handle: construction, job enqueue, in-process delivery,
//! scanned-role feeding, and the HTTP serving entry points.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_engine_io::http::{HttpRequestData, HttpResponder, HttpResponseData};
use temper_engine_io::{Spawner, channel, drive};
use temper_forge::{Forge, RepositoryId};
use temper_runner::WorkItem;
use temper_worker_protocol::{Artifact, WorkerProtocolMessage};
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

    pub fn with_applier(spawner: Arc<dyn Spawner>, applier: Arc<dyn ResultApplier>) -> Self {
        Self::with_applier_and_apply_grace(spawner, applier, APPLY_GRACE)
    }

    pub fn with_apply_grace(self, apply_grace: Duration) -> Self {
        let _ = self
            .cq
            .send(DaemonCompletion::SetApplyGrace { apply_grace });
        self
    }

    fn with_applier_and_apply_grace(
        spawner: Arc<dyn Spawner>,
        applier: Arc<dyn ResultApplier>,
        apply_grace: Duration,
    ) -> Self {
        let (cq_tx, cq_rx) = channel();
        let scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let executor = DaemonExecutor {
            spawner: Arc::clone(&spawner),
            cq: cq_tx.clone(),
            applier,
            scanner_slot: Arc::clone(&scanner_slot),
        };
        let machine = DaemonMachine::default_machine(apply_grace);
        spawner.spawn_with_cx(move |cx| async move {
            let _ = drive(cx, machine, &executor, cq_rx).await;
        });

        Self {
            cq: cq_tx,
            scanner_slot,
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
        let (reply_tx, reply_rx) = temper_engine_io::oneshot::<HttpResponseData>();
        let request = HttpRequestData {
            method: "POST".to_string(),
            uri: "/v1/message".to_string(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
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
    tracing::info!(target: "engine", addr = %server.local_addr(), "serving");
    Ok(server)
}

/// The daemon's h1 request handler — the same request→completion conversion
/// [`serve`] installs on the TCP listener, exposed so in-memory simulation
/// gateways can serve connections against the daemon's queue.
pub fn h1_handler(daemon: &Daemon) -> temper_engine_io::http::H1CompletionHandler {
    temper_engine_io::http::h1_completion_handler(daemon.cq.clone(), |request, responder| {
        DaemonCompletion::Http { request, responder }
    })
}
