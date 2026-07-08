//! Shared harness for driving a **real** Temper daemon from worker e2e tests.
//!
//! The daemon now lives in this workspace, so tests run the real thing instead
//! of a fake. The worker reaches the daemon through the reusable in-process
//! carrier (`temper-daemon-transport`, backed by `deliver_protocol_message`) —
//! the same path the unified single-process worker uses in production. No tokio,
//! no axum, no socket.
//!
//! Result observation taps the `Result` the worker posts over the transport,
//! rather than a recording applier: the daemon routes a *transient* failure to
//! `DropForRescan` and never invokes the applier, so an applier-based hook would
//! hang for the crash/transient tests. The transport tap fires for every posted
//! result regardless of the daemon's disposition.

#![allow(dead_code)]

use std::sync::Arc;

use skein::cx::Cx;
use temper_daemon_transport::InProcessTransport as DaemonInProcessTransport;
use temper_engine::{Daemon, NoopApplier, ResultApplier, WorkerPoolPolicy};
use temper_protocol_worker::{Assign, JobResult, WorkerAuth, WorkerProtocolMessage};
use temper_worker::Transport;

/// In-process transport wrapper: delegates worker→daemon delivery to the
/// reusable transport, and taps the `Result` the worker posts onto `result_tx`.
pub struct ResultTappingTransport {
    inner: DaemonInProcessTransport,
    result_tx: temper_engine_io::CqSender<JobResult>,
}

impl Transport for ResultTappingTransport {
    fn send(
        &self,
        cx: Cx,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> impl std::future::Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send
    {
        let inner = self.inner.clone();
        let result_tx = self.result_tx.clone();
        async move {
            if let WorkerProtocolMessage::Result(result) = &message {
                let _ = result_tx.send(result.clone());
            }
            inner.send(cx, message, auth).await
        }
    }
}

/// A real daemon plus a channel that receives the result the worker posts.
pub struct DaemonHarness {
    pub daemon: Arc<Daemon>,
    result_tx: temper_engine_io::CqSender<JobResult>,
    result_rx: temper_engine_io::CqReceiver<JobResult>,
}

impl DaemonHarness {
    /// Build a real daemon on the given runtime handle.
    pub fn start(handle: &skein::runtime::RuntimeHandle) -> Self {
        Self::start_with_daemon(Arc::new(Daemon::new(Arc::new(handle.clone()))))
    }

    /// Build a real daemon with worker-pool authentication configured.
    pub fn start_with_worker_auth(
        handle: &skein::runtime::RuntimeHandle,
        auth: temper_engine::WorkerPoolAuthConfig,
    ) -> Self {
        let daemon = Daemon::with_applier_and_worker_pools(
            Arc::new(handle.clone()),
            Arc::new(NoopApplier),
            vec![WorkerPoolPolicy::new(
                "builders",
                vec!["engineer".to_string()],
                vec!["ai/smith".to_string()],
                Some(1),
            )],
        )
        .with_worker_pool_auth(auth);
        Self::start_with_daemon(Arc::new(daemon))
    }

    /// Build a real daemon with a caller-provided result applier.
    pub fn start_with_applier(
        handle: &skein::runtime::RuntimeHandle,
        applier: Arc<dyn ResultApplier>,
    ) -> Self {
        Self::start_with_daemon(Arc::new(Daemon::with_applier(
            Arc::new(handle.clone()),
            applier,
        )))
    }

    fn start_with_daemon(daemon: Arc<Daemon>) -> Self {
        let (result_tx, result_rx) = temper_engine_io::channel();
        Self {
            daemon,
            result_tx,
            result_rx,
        }
    }

    /// Enqueue the job described by `assign` (role/repo/artifact/payload) so the
    /// daemon hands it to the next matching poll.
    pub async fn enqueue(&self, assign: &Assign) {
        self.daemon
            .enqueue_job(
                assign.job_id.clone(),
                assign.role.clone(),
                assign.repo.clone(),
                assign.artifact.clone(),
                assign.job_payload.clone(),
            )
            .await;
    }

    /// An in-process transport bound to this daemon that taps posted results.
    pub fn transport(&self) -> Arc<ResultTappingTransport> {
        Arc::new(ResultTappingTransport {
            inner: DaemonInProcessTransport::new(self.daemon.as_ref().clone()),
            result_tx: self.result_tx.clone(),
        })
    }

    /// Await the result the worker posts to the daemon.
    pub async fn await_result(&mut self) -> JobResult {
        self.result_rx
            .recv()
            .await
            .expect("worker posts a result to the daemon")
    }
}
