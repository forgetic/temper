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

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use skein::runtime::RuntimeHandle;
use temper_protocol_worker::{WorkerAuth, WorkerProtocolMessage};
use temper_worker_io::{CqSender, Spawner, arm_timer};

use crate::executor::{
    AttemptFence, JobAttempt, JobCancellation, JobExecutionContext, JobExecutor,
    job_result_for_attempt,
};
use crate::result_outbox::ResultOutbox;
use crate::transport::{HttpTransport, Transport};
use crate::worker_machine::{JobCleanup, WorkerCompletion, WorkerMachine, WorkerRequest};

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

    pub(crate) async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
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

#[derive(Clone)]
struct JobControl {
    attempt_id: String,
    generation: u64,
    fence: AttemptFence,
    cancellation: JobCancellation,
}

/// Performs the worker's I/O on a skein spawn capability (production runtime or lab).
pub struct WorkerShell<E: JobExecutor, T: Transport = HttpTransport, S: Spawner = RuntimeHandle> {
    spawner: S,
    cq: CqSender<WorkerCompletion>,
    transport: Arc<T>,
    worker_auth: Option<WorkerAuth>,
    worker_id: String,
    executor: Arc<E>,
    outbox: Arc<ResultOutbox>,
    cancellation: WorkerCancellation,
    job_controls: Arc<Mutex<BTreeMap<String, JobControl>>>,
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
        let outbox = Arc::new(ResultOutbox::new(
            std::env::temp_dir().join(format!("temper-worker-results-{worker_id}")),
        ));
        Self::with_transport_controlled(
            handle,
            cq,
            Arc::new(HttpTransport::new(daemon_url)),
            None,
            worker_id,
            executor,
            outbox,
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
        let outbox = Arc::new(ResultOutbox::new(
            std::env::temp_dir().join(format!("temper-worker-results-{worker_id}")),
        ));
        Self::with_transport_controlled(
            spawner,
            cq,
            transport,
            worker_auth,
            worker_id,
            executor,
            outbox,
            WorkerCancellation::default(),
        )
    }

    // This internal construction boundary keeps transport, durability, and
    // shared shutdown capabilities explicit; public factories remain compact.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_transport_controlled(
        spawner: S,
        cq: CqSender<WorkerCompletion>,
        transport: Arc<T>,
        worker_auth: Option<WorkerAuth>,
        worker_id: String,
        executor: Arc<E>,
        outbox: Arc<ResultOutbox>,
        cancellation: WorkerCancellation,
    ) -> Self {
        Self {
            spawner,
            cq,
            transport,
            worker_auth,
            worker_id,
            executor,
            outbox,
            cancellation,
            job_controls: Arc::new(Mutex::new(BTreeMap::new())),
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
            WorkerRequest::RecordResult {
                job_id,
                attempt_id,
                generation,
                result,
            } => {
                let outbox = Arc::clone(&self.outbox);
                let cq = self.cq.clone();
                self.spawner.spawn_task(async move {
                    let outcome = outbox.record(result).map_err(|error| error.to_string());
                    let _ = cq.send(WorkerCompletion::ResultRecorded {
                        job_id,
                        attempt_id,
                        generation,
                        outcome,
                    });
                });
            }
            WorkerRequest::SendResult { entry_id, message } => {
                self.post(message, move |outcome| WorkerCompletion::ResultDelivered {
                    entry_id,
                    outcome,
                });
            }
            WorkerRequest::AcknowledgeResult { entry, release } => {
                let outbox = Arc::clone(&self.outbox);
                let cq = self.cq.clone();
                let entry_id = entry.entry_id.clone();
                self.spawner.spawn_task(async move {
                    let outcome = outbox
                        .acknowledge(&entry, &release)
                        .map(|_| ())
                        .map_err(|error| error.to_string());
                    let _ = cq.send(WorkerCompletion::ResultFinalized { entry_id, outcome });
                });
            }
            WorkerRequest::RejectResult { entry, reason } => {
                let outbox = Arc::clone(&self.outbox);
                let cq = self.cq.clone();
                let entry_id = entry.entry_id.clone();
                self.spawner.spawn_task(async move {
                    let outcome = outbox
                        .reject(&entry, &reason)
                        .map_err(|error| error.to_string());
                    let _ = cq.send(WorkerCompletion::ResultFinalized { entry_id, outcome });
                });
            }
            WorkerRequest::SendHeartbeat(message) => {
                self.post(message, |reply| {
                    WorkerCompletion::HeartbeatDelivered(heartbeat_outcome(reply))
                });
            }
            WorkerRequest::RunJob { assign, generation } => {
                let executor = Arc::clone(&self.executor);
                let cq = self.cq.clone();
                let worker_id = self.worker_id.clone();
                let component_cancellation = self.cancellation.clone();
                let controls = Arc::clone(&self.job_controls);
                let job_id = assign.job_id.clone();
                let attempt_id = assign
                    .attempt_id
                    .clone()
                    .expect("machine dispatches only fenced assignments");
                let fence = AttemptFence::open();
                let job_cancellation = JobCancellation::default();
                let progress_cq = cq.clone();
                let progress_job_id = job_id.clone();
                let progress_attempt_id = attempt_id.clone();
                let progress_fence = fence.clone();
                let progress = crate::JobProgressReporter::with_attempt_guard(
                    attempt_id.clone(),
                    move |reported_attempt| {
                        progress_fence.is_open() && reported_attempt == progress_attempt_id
                    },
                    move |progress| {
                        let _ = progress_cq.send(WorkerCompletion::JobProgress {
                            job_id: progress_job_id.clone(),
                            attempt_id: progress.attempt_id.clone(),
                            generation,
                            progress,
                        });
                    },
                );
                controls.lock().expect("job controls lock").insert(
                    job_id.clone(),
                    JobControl {
                        attempt_id: attempt_id.clone(),
                        generation,
                        fence: fence.clone(),
                        cancellation: job_cancellation.clone(),
                    },
                );
                let execution = JobExecutionContext {
                    attempt: JobAttempt {
                        id: attempt_id.clone(),
                        generation,
                    },
                    fence: fence.clone(),
                    cancellation: job_cancellation.clone(),
                    progress,
                };
                self.spawner.spawn_task(async move {
                    let run = job_cancellation.run(executor.execute(assign, execution));
                    let component_result = component_cancellation.run(run).await;
                    let mut current_controls = controls.lock().expect("job controls lock");
                    if current_controls.get(&job_id).is_some_and(|control| {
                        control.generation == generation && control.attempt_id == attempt_id
                    }) {
                        current_controls.remove(&job_id);
                    }
                    drop(current_controls);
                    match component_result {
                        None => {}
                        Some(Some(outcome)) if fence.is_open() => {
                            let result = job_result_for_attempt(
                                &worker_id,
                                &job_id,
                                Some(attempt_id.clone()),
                                outcome,
                            );
                            if fence.is_open() {
                                let _ = cq.send(WorkerCompletion::JobFinished {
                                    job_id,
                                    attempt_id,
                                    generation,
                                    result,
                                });
                            }
                        }
                        Some(Some(_)) | Some(None) => {
                            let _ = cq.send(WorkerCompletion::JobQuiesced {
                                job_id,
                                attempt_id,
                                generation,
                                cleanup: JobCleanup {
                                    cancellation: "requested".to_string(),
                                    descendants: "joined".to_string(),
                                },
                            });
                        }
                    }
                });
            }
            WorkerRequest::CancelJob {
                job_id,
                attempt_id,
                generation,
                reason: _,
            }
            | WorkerRequest::EscalateJob {
                job_id,
                attempt_id,
                generation,
                hard: _,
            } => {
                let control = self
                    .job_controls
                    .lock()
                    .expect("job controls lock")
                    .get(&job_id)
                    .filter(|control| {
                        control.generation == generation && control.attempt_id == attempt_id
                    })
                    .cloned();
                if let Some(control) = control {
                    control.fence.close();
                    control.cancellation.cancel();
                }
            }
            WorkerRequest::ArmWatchdogTimer {
                job_id,
                attempt_id,
                generation,
                timer_generation,
                kind,
                delay,
            } => {
                arm_timer(&self.spawner, &self.cq, delay, move || {
                    WorkerCompletion::WatchdogTimer {
                        job_id,
                        attempt_id,
                        generation,
                        timer_generation,
                        kind,
                    }
                });
            }
            WorkerRequest::ArmResultRecordTimer {
                job_id,
                attempt_id,
                generation,
                delay,
            } => {
                arm_timer(&self.spawner, &self.cq, delay, move || {
                    WorkerCompletion::ResultRecordTimer {
                        job_id,
                        attempt_id,
                        generation,
                    }
                });
            }
            WorkerRequest::ArmResultReplayTimer { entry_id, delay } => {
                arm_timer(&self.spawner, &self.cq, delay, move || {
                    WorkerCompletion::ResultReplayTimer { entry_id }
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
            WorkerRequest::Observe(event) => event.emit(),
            WorkerRequest::Warn(line) => {
                tracing::warn!(target: "temper_worker", "{line}");
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
