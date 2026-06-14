//! Shared harness for driving a **real** Temper daemon from worker e2e tests.
//!
//! The daemon now lives in this workspace, so tests run the real thing instead
//! of a fake. The worker reaches the daemon through its in-process carrier
//! (`deliver_protocol_message`) — the same path the unified single-process
//! worker uses in production. No tokio, no axum, no socket.
//!
//! Result observation taps the `Result` the worker posts over the transport,
//! rather than a recording applier: the daemon routes a *transient* failure to
//! `DropForRescan` and never invokes the applier, so an applier-based hook would
//! hang for the crash/transient tests. The transport tap fires for every posted
//! result regardless of the daemon's disposition.

#![allow(dead_code)]

use std::sync::Arc;

use skein::cx::Cx;
use temper_engine::Daemon;
use temper_worker::transport::Transport;
use temper_worker_protocol::{Assign, JobResult, WorkerProtocolMessage};

/// In-process transport: hands each worker→daemon message to the real daemon's
/// in-process carrier, and taps the `Result` the worker posts onto `result_tx`.
pub struct InProcessTransport {
    daemon: Arc<Daemon>,
    result_tx: temper_io_engine::CqSender<JobResult>,
}

impl Transport for InProcessTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
    ) -> impl std::future::Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send
    {
        let daemon = Arc::clone(&self.daemon);
        let result_tx = self.result_tx.clone();
        async move {
            if let WorkerProtocolMessage::Result(result) = &message {
                let _ = result_tx.send(result.clone());
            }
            daemon.deliver_protocol_message(message).await
        }
    }
}

/// A real daemon plus a channel that receives the result the worker posts.
pub struct DaemonHarness {
    pub daemon: Arc<Daemon>,
    result_tx: temper_io_engine::CqSender<JobResult>,
    result_rx: temper_io_engine::CqReceiver<JobResult>,
}

impl DaemonHarness {
    /// Build a real daemon on the given runtime handle.
    pub fn start(handle: &skein::runtime::RuntimeHandle) -> Self {
        let daemon = Arc::new(Daemon::new(Arc::new(handle.clone())));
        let (result_tx, result_rx) = temper_io_engine::channel();
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
    pub fn transport(&self) -> Arc<InProcessTransport> {
        Arc::new(InProcessTransport {
            daemon: Arc::clone(&self.daemon),
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
