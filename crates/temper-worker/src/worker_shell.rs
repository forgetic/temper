//! The worker's imperative shell.
//!
//! [`WorkerShell`] implements [`temper_worker_io::Executor`] for
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

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use skein::runtime::RuntimeHandle;
use temper_protocol_worker::{WorkerAuth, WorkerProtocolMessage};
use temper_worker_io::{CqSender, Spawner, arm_timer};

use crate::executor::{JobExecutor, job_result};
use crate::transport::{HttpTransport, Transport};
use crate::worker_machine::{WorkerCompletion, WorkerMachine, WorkerRequest};

/// Shared cancellation authority for a worker component and all job futures it
/// spawned. Dropping a cancelled job future prevents its later git/Forge
/// publication from outliving the component machine.
#[derive(Clone, Default)]
pub(crate) struct WorkerCancellation {
    cancelled: Arc<AtomicBool>,
    waiters: Arc<Mutex<Vec<Waker>>>,
}

impl WorkerCancellation {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        for waiter in std::mem::take(&mut *self.waiters.lock().expect("worker cancel lock")) {
            waiter.wake();
        }
    }

    async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
        let mut future = std::pin::pin!(future);
        std::future::poll_fn(|cx| {
            if self.cancelled.load(Ordering::SeqCst) {
                return Poll::Ready(None);
            }
            if let Poll::Ready(output) = Pin::new(&mut future).poll(cx) {
                return Poll::Ready(Some(output));
            }
            let mut waiters = self.waiters.lock().expect("worker cancel lock");
            if self.cancelled.load(Ordering::SeqCst) {
                return Poll::Ready(None);
            }
            waiters.push(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
}

/// Performs the worker's I/O on a skein spawn capability (production runtime or lab).
pub struct WorkerShell<E: JobExecutor, T: Transport = HttpTransport, S: Spawner = RuntimeHandle> {
    spawner: S,
    cq: CqSender<WorkerCompletion>,
    transport: Arc<T>,
    worker_auth: Option<WorkerAuth>,
    worker_id: String,
    executor: Arc<E>,
    cancellation: WorkerCancellation,
}

impl<E: JobExecutor + Send + Sync + 'static> WorkerShell<E, HttpTransport, RuntimeHandle> {
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
        Self::with_transport_controlled(
            handle,
            cq,
            Arc::new(HttpTransport::new(daemon_url)),
            None,
            worker_id,
            executor,
            WorkerCancellation::default(),
        )
    }
}

impl<E: JobExecutor + Send + Sync + 'static, T: Transport, S: Spawner> WorkerShell<E, T, S> {
    /// Builds the shell over an arbitrary [`Transport`] (e.g. the unified
    /// in-process transport that delivers to a co-resident `DaemonCore`).
    pub fn with_transport(
        spawner: S,
        cq: CqSender<WorkerCompletion>,
        transport: Arc<T>,
        worker_auth: Option<WorkerAuth>,
        worker_id: String,
        executor: Arc<E>,
    ) -> Self {
        Self::with_transport_controlled(
            spawner,
            cq,
            transport,
            worker_auth,
            worker_id,
            executor,
            WorkerCancellation::default(),
        )
    }

    pub(crate) fn with_transport_controlled(
        spawner: S,
        cq: CqSender<WorkerCompletion>,
        transport: Arc<T>,
        worker_auth: Option<WorkerAuth>,
        worker_id: String,
        executor: Arc<E>,
        cancellation: WorkerCancellation,
    ) -> Self {
        Self {
            spawner,
            cq,
            transport,
            worker_auth,
            worker_id,
            executor,
            cancellation,
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
        let auth = self.worker_auth.clone();
        self.spawner.spawn_task_with_cx(move |cx| async move {
            let decoded = transport.send(cx, message, auth).await;
            let _ = cq.send(to_completion(decoded));
        });
    }
}

impl<E: JobExecutor + Send + Sync + 'static, T: Transport, S: Spawner>
    temper_worker_io::Executor<WorkerMachine> for WorkerShell<E, T, S>
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
                    WorkerCompletion::HeartbeatDelivered(heartbeat_outcome(reply))
                });
            }
            WorkerRequest::RunJob(assign) => {
                let executor = Arc::clone(&self.executor);
                let cq = self.cq.clone();
                let worker_id = self.worker_id.clone();
                let cancellation = self.cancellation.clone();
                let job_id = assign.job_id.clone();
                self.spawner.spawn_task(async move {
                    let Some(outcome) = cancellation.run(executor.execute(assign)).await else {
                        return;
                    };
                    let result = job_result(&worker_id, &job_id, outcome);
                    let _ = cq.send(WorkerCompletion::JobFinished { job_id, result });
                });
            }
            WorkerRequest::ArmPollTimer(delay) => {
                arm_timer(&self.spawner, &self.cq, delay, || {
                    WorkerCompletion::PollTimer
                });
            }
            WorkerRequest::ArmHeartbeatTimer(delay) => {
                arm_timer(&self.spawner, &self.cq, delay, || {
                    WorkerCompletion::HeartbeatTimer
                });
            }
            WorkerRequest::Log(line) => {
                // Worker-side protocol traces (`worker: registered`, `worker:
                // assigned`, `worker: result sent`, refusals/failures). These are
                // per-job chatter, not the §7 info catalog (§5), so they sit at
                // debug to keep the default operator view to the §7 lines.
                tracing::debug!(target: "temper_worker", "{line}");
            }
        }
    }
}

fn heartbeat_outcome(reply: Result<Option<WorkerProtocolMessage>, String>) -> Result<(), String> {
    match reply {
        Ok(None) => Ok(()),
        Ok(Some(WorkerProtocolMessage::Error(error))) => {
            Err(format!("daemon rejected heartbeat: {error:?}"))
        }
        Ok(Some(other)) => Err(format!(
            "daemon returned unexpected heartbeat response: {other:?}"
        )),
        Err(error) => Err(error),
    }
}
