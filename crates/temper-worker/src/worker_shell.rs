//! The worker's imperative shell.
//!
//! [`WorkerShell`] implements [`temper_worker_io_engine::Executor`] for
//! [`WorkerMachine`](crate::worker_machine::WorkerMachine): it performs the I/O
//! each [`WorkerRequest`] asks for — deliver worker-protocol messages to the
//! daemon over a [`Transport`](crate::transport::Transport), run dispatched jobs
//! through the [`JobExecutor`], and arm timers — and feeds every result back
//! into the completion queue as a [`WorkerCompletion`]. It never calls into the
//! machine.
//!
//! The shell is generic over the transport: [`HttpTransport`](crate::transport::HttpTransport)
//! for the split deployment, an in-process transport for the unified
//! single-process mode. The protocol crossing the seam is identical; only the
//! carrier differs.

use std::sync::Arc;

use skein::runtime::RuntimeHandle;
use temper_worker_io_engine::{CqSender, arm_timer};
use temper_worker_protocol::WorkerProtocolMessage;

use crate::executor::{JobExecutor, job_result};
use crate::transport::{HttpTransport, Transport};
use crate::worker_machine::{WorkerCompletion, WorkerMachine, WorkerRequest};

/// Performs the worker's I/O on the skein runtime.
pub struct WorkerShell<E: JobExecutor, T: Transport = HttpTransport> {
    handle: RuntimeHandle,
    cq: CqSender<WorkerCompletion>,
    transport: Arc<T>,
    worker_id: String,
    executor: Arc<E>,
}

impl<E: JobExecutor + Send + Sync + 'static> WorkerShell<E, HttpTransport> {
    /// Builds the shell with the HTTP transport (split deployment). `daemon_url`
    /// is the base daemon URL; the worker posts every message to
    /// `<daemon_url>/v1/message`.
    pub fn new(
        handle: RuntimeHandle,
        cq: CqSender<WorkerCompletion>,
        daemon_url: &str,
        worker_id: String,
        executor: Arc<E>,
    ) -> Self {
        Self::with_transport(
            handle,
            cq,
            Arc::new(HttpTransport::new(daemon_url)),
            worker_id,
            executor,
        )
    }
}

impl<E: JobExecutor + Send + Sync + 'static, T: Transport> WorkerShell<E, T> {
    /// Builds the shell over an arbitrary [`Transport`] (e.g. the unified
    /// in-process transport that delivers to a co-resident `DaemonCore`).
    pub fn with_transport(
        handle: RuntimeHandle,
        cq: CqSender<WorkerCompletion>,
        transport: Arc<T>,
        worker_id: String,
        executor: Arc<E>,
    ) -> Self {
        Self {
            handle,
            cq,
            transport,
            worker_id,
            executor,
        }
    }

    /// Deliver one message over the transport; map its reply into `completion`
    /// and enqueue.
    fn post<F>(&self, message: WorkerProtocolMessage, to_completion: F)
    where
        F: FnOnce(Result<Option<WorkerProtocolMessage>, String>) -> WorkerCompletion
            + Send
            + 'static,
    {
        let transport = Arc::clone(&self.transport);
        let cq = self.cq.clone();
        self.handle.spawn_with_cx(move |cx| async move {
            let decoded = transport.send(cx, message).await;
            let _ = cq.send(to_completion(decoded));
        });
    }
}

impl<E: JobExecutor + Send + Sync + 'static, T: Transport>
    temper_worker_io_engine::Executor<WorkerMachine> for WorkerShell<E, T>
{
    fn execute(&self, request: WorkerRequest) {
        match request {
            WorkerRequest::SendRegister(message) => {
                self.post(message, |reply| {
                    WorkerCompletion::Registered(reply.map(|_| ()))
                });
            }
            WorkerRequest::SendPoll(message) => {
                self.post(message, WorkerCompletion::PollReply);
            }
            WorkerRequest::SendResult { job_id, message } => {
                self.post(message, move |reply| WorkerCompletion::ResultDelivered {
                    job_id,
                    outcome: reply.map(|_| ()),
                });
            }
            WorkerRequest::SendHeartbeat(message) => {
                self.post(message, |reply| {
                    WorkerCompletion::HeartbeatDelivered(reply.map(|_| ()))
                });
            }
            WorkerRequest::RunJob(assign) => {
                let executor = Arc::clone(&self.executor);
                let cq = self.cq.clone();
                let worker_id = self.worker_id.clone();
                let job_id = assign.job_id.clone();
                self.handle.spawn(async move {
                    let outcome = executor.execute(assign).await;
                    let result = job_result(&worker_id, &job_id, outcome);
                    let _ = cq.send(WorkerCompletion::JobFinished { job_id, result });
                });
            }
            WorkerRequest::ArmPollTimer(delay) => {
                arm_timer(&self.handle, &self.cq, delay, || {
                    WorkerCompletion::PollTimer
                });
            }
            WorkerRequest::ArmHeartbeatTimer(delay) => {
                arm_timer(&self.handle, &self.cq, delay, || {
                    WorkerCompletion::HeartbeatTimer
                });
            }
            WorkerRequest::Log(line) => {
                eprintln!("{line}");
            }
        }
    }
}
