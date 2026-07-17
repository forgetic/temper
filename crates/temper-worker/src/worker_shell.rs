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

use crate::executor::{
    AttemptFence, CancellationOutcome, JobAttempt, JobCancellation, JobCleanup,
    JobContainmentObservation, JobExecutionContext, JobExecutor, job_result_for_attempt,
};
use crate::lifecycle_hook::{
    NoopWorkerLifecycleHook, WorkerLifecycleCheckpoint, WorkerLifecycleHook,
};
use crate::result_outbox::ResultOutbox;
use crate::task_registry::{ActiveJobTask, WorkerComponentTasks, WorkerTaskRegistry};
use crate::transport::{HttpTransport, Transport};
use crate::worker_machine::{AttemptCompletion, WorkerCompletion, WorkerMachine, WorkerRequest};

/// Shared cancellation authority for a worker component and all job futures it
/// spawned. Component shutdown drops attempt futures only as an abrupt-owner
/// fallback; normal per-job cancellation uses the explicit `JobCancellation`
/// handshake.
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
    lifecycle_hook: Arc<dyn WorkerLifecycleHook>,
    task_registry: WorkerTaskRegistry,
    component_tasks: WorkerComponentTasks,
    containment_events: crate::observability::ContainmentEventThrottle,
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
            Arc::new(NoopWorkerLifecycleHook),
            WorkerTaskRegistry::new(),
            WorkerComponentTasks::default(),
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
            Arc::new(NoopWorkerLifecycleHook),
            WorkerTaskRegistry::new(),
            WorkerComponentTasks::default(),
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
        lifecycle_hook: Arc<dyn WorkerLifecycleHook>,
        task_registry: WorkerTaskRegistry,
        component_tasks: WorkerComponentTasks,
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
            lifecycle_hook,
            task_registry,
            component_tasks,
            containment_events: crate::observability::ContainmentEventThrottle::default(),
        }
    }

    fn spawn_component_task<Fut>(&self, future: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let Some(guard) = self.component_tasks.register() else {
            return;
        };
        self.spawner.spawn_task(async move {
            future.await;
            drop(guard);
        });
    }

    fn spawn_component_task_with_cx<F, Fut>(&self, task: F)
    where
        F: FnOnce(skein::cx::Cx) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let Some(guard) = self.component_tasks.register() else {
            return;
        };
        self.spawner.spawn_task_with_cx(move |cx| async move {
            task(cx).await;
            drop(guard);
        });
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
        let component_cancellation = self.cancellation.clone();
        self.spawn_component_task_with_cx(move |cx| async move {
            let Some(decoded) = component_cancellation
                .run(transport.send(cx, message, auth))
                .await
            else {
                return;
            };
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
                let lifecycle_hook = Arc::clone(&self.lifecycle_hook);
                let component_cancellation = self.cancellation.clone();
                self.spawn_component_task(async move {
                    if lifecycle_hook.enabled()
                        && component_cancellation
                            .run(lifecycle_hook.reached(WorkerLifecycleCheckpoint::Quiesced))
                            .await
                            .is_none()
                    {
                        return;
                    }
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
                let transport = Arc::clone(&self.transport);
                let cq = self.cq.clone();
                let auth = self.worker_auth.clone();
                let lifecycle_hook = Arc::clone(&self.lifecycle_hook);
                let component_cancellation = self.cancellation.clone();
                self.spawn_component_task_with_cx(move |cx| async move {
                    if lifecycle_hook.enabled()
                        && component_cancellation
                            .run(lifecycle_hook.reached(WorkerLifecycleCheckpoint::ResultRecorded))
                            .await
                            .is_none()
                    {
                        return;
                    }
                    let Some(outcome) = component_cancellation
                        .run(transport.send(cx, message, auth))
                        .await
                    else {
                        return;
                    };
                    if lifecycle_hook.enabled()
                        && component_cancellation
                            .run(
                                lifecycle_hook
                                    .reached(WorkerLifecycleCheckpoint::ResultDeliveryResolved),
                            )
                            .await
                            .is_none()
                    {
                        return;
                    }
                    let _ = cq.send(WorkerCompletion::ResultDelivered { entry_id, outcome });
                });
            }
            WorkerRequest::AcknowledgeResult { entry, release } => {
                let outbox = Arc::clone(&self.outbox);
                let cq = self.cq.clone();
                let entry_id = entry.entry_id.clone();
                let lifecycle_hook = Arc::clone(&self.lifecycle_hook);
                let component_cancellation = self.cancellation.clone();
                self.spawn_component_task(async move {
                    if lifecycle_hook.enabled()
                        && component_cancellation
                            .run(
                                lifecycle_hook
                                    .reached(WorkerLifecycleCheckpoint::ResultAcknowledged),
                            )
                            .await
                            .is_none()
                    {
                        return;
                    }
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
                self.spawn_component_task(async move {
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
                let registry = self.task_registry.clone();
                let job_id = assign.job_id.clone();
                let attempt_id = assign
                    .attempt_id
                    .clone()
                    .expect("machine dispatches only fenced assignments");
                let fence = AttemptFence::open();
                let job_cancellation = JobCancellation::default();
                let active = ActiveJobTask::new(
                    job_id.clone(),
                    attempt_id.clone(),
                    generation,
                    fence.clone(),
                    job_cancellation.clone(),
                );
                let cleanup_cq = cq.clone();
                let cleanup_registry = registry.clone();
                let cleanup_job_id = job_id.clone();
                let cleanup_attempt_id = attempt_id.clone();
                let containment_events = self.containment_events.clone();
                let containment_context = crate::observability::ContainmentEventContext::new(
                    &self.worker_id,
                    &job_id,
                    &attempt_id,
                );
                job_cancellation.set_cleanup_observer(move |observation| {
                    let blocked = match observation {
                        JobContainmentObservation::Cleanup(observation) => {
                            containment_events.cleanup(&containment_context, &observation);
                            match observation.snapshot() {
                                temper_process_containment::CleanupSnapshot::Blocked { .. } => {
                                    Some(observation.snapshot().clone())
                                }
                                _ => None,
                            }
                        }
                        JobContainmentObservation::Snapshot(snapshot) => matches!(
                            &snapshot,
                            temper_process_containment::CleanupSnapshot::Blocked { .. }
                        )
                        .then_some(snapshot),
                        JobContainmentObservation::Fallback(fallback) => {
                            containment_events.fallback(&containment_context, &fallback);
                            None
                        }
                        // The worker emits one startup diagnostic before it
                        // accepts jobs; per-factory copies are deliberately
                        // suppressed here.
                        JobContainmentObservation::Capability(_) => None,
                    };
                    let Some(snapshot) = blocked else {
                        return;
                    };
                    cleanup_registry.mark_cleanup_pending(
                        &cleanup_job_id,
                        &cleanup_attempt_id,
                        generation,
                    );
                    let _ = cleanup_cq.send(WorkerCompletion::AttemptCleanupBlocked {
                        job_id: cleanup_job_id.clone(),
                        attempt_id: cleanup_attempt_id.clone(),
                        generation,
                        snapshot,
                    });
                });
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
                let execution = JobExecutionContext {
                    attempt: JobAttempt {
                        id: attempt_id.clone(),
                        generation,
                    },
                    fence: fence.clone(),
                    cancellation: job_cancellation.clone(),
                    progress,
                };
                if !registry.register(active.clone()) {
                    fence.close();
                    job_cancellation.hard_kill();
                    return;
                }
                registry.mark_running(&active);
                self.spawner.spawn_task(async move {
                    // This attempt future is never raced against component
                    // teardown. Its own cancellation owner must report proven
                    // descendant and endpoint joins before the registry leaves.
                    let outcome = job_cancellation
                        .run_to_quiescence(executor.execute(assign, execution))
                        .await;
                    if job_cancellation
                        .cleanup()
                        .is_some_and(|cleanup| !cleanup.proves_quiescence())
                    {
                        registry.mark_cleanup_pending(&job_id, &attempt_id, generation);
                        std::future::pending::<()>().await;
                    }
                    let completion = match outcome {
                        Some(outcome) if fence.is_open() => {
                            let result = job_result_for_attempt(
                                &worker_id,
                                &job_id,
                                Some(attempt_id.clone()),
                                outcome,
                            );
                            let cleanup = job_cancellation
                                .cleanup()
                                .unwrap_or_else(|| JobCleanup::no_process(None));
                            AttemptCompletion {
                                result: Some(result),
                                cleanup,
                            }
                        }
                        Some(_) | None => {
                            let cleanup = job_cancellation.cleanup().unwrap_or_else(|| {
                                JobCleanup::no_process(Some(CancellationOutcome::Graceful))
                            });
                            AttemptCompletion {
                                result: None,
                                cleanup,
                            }
                        }
                    };
                    registry.finish_with(&active, |publish| {
                        if publish {
                            let _ = cq.send(WorkerCompletion::AttemptQuiesced {
                                job_id,
                                attempt_id,
                                generation,
                                completion,
                            });
                        }
                    });
                });
            }
            WorkerRequest::CancelJob {
                job_id,
                attempt_id,
                generation,
                reason: _,
            } => {
                if self.lifecycle_hook.enabled() {
                    let lifecycle_hook = Arc::clone(&self.lifecycle_hook);
                    let component_cancellation = self.cancellation.clone();
                    let registry = self.task_registry.clone();
                    let cq = self.cq.clone();
                    self.spawn_component_task(async move {
                        if component_cancellation
                            .run(lifecycle_hook.reached(WorkerLifecycleCheckpoint::CancelRequested))
                            .await
                            .is_none()
                        {
                            return;
                        }
                        cancel_job_control(registry, cq, job_id, attempt_id, generation);
                    });
                } else {
                    cancel_job_control(
                        self.task_registry.clone(),
                        self.cq.clone(),
                        job_id,
                        attempt_id,
                        generation,
                    );
                }
            }
            WorkerRequest::EscalateJob {
                job_id,
                attempt_id,
                generation,
                hard,
            } => {
                if let Some(task) = self.task_registry.task(&job_id, &attempt_id, generation) {
                    if hard {
                        task.cancellation().hard_kill();
                    } else {
                        task.cancellation().force_terminate();
                    }
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
            WorkerRequest::Observe(event) => {
                if matches!(
                    &event,
                    crate::observability::WorkerEvent::CapacityReleased { .. }
                ) && self.lifecycle_hook.enabled()
                {
                    let lifecycle_hook = Arc::clone(&self.lifecycle_hook);
                    let component_cancellation = self.cancellation.clone();
                    self.spawn_component_task(async move {
                        let _ = component_cancellation
                            .run(
                                lifecycle_hook.reached(WorkerLifecycleCheckpoint::CapacityReleased),
                            )
                            .await;
                    });
                }
                event.emit();
            }
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

fn cancel_job_control(
    registry: WorkerTaskRegistry,
    cq: CqSender<WorkerCompletion>,
    job_id: String,
    attempt_id: String,
    generation: u64,
) {
    if registry.cancel_attempt(&job_id, &attempt_id, generation) {
        return;
    }
    if !registry.is_shutting_down() {
        // The executor may have naturally quiesced between the machine
        // transition and this shell request. Complete that race cooperatively
        // so a timeout cannot wait forever for a disappeared control.
        let _ = cq.send(WorkerCompletion::AttemptQuiesced {
            job_id,
            attempt_id,
            generation,
            completion: AttemptCompletion {
                result: None,
                cleanup: JobCleanup::no_process(Some(CancellationOutcome::Graceful)),
            },
        });
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
