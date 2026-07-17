//! Worker wiring: build the sans-IO machine + shell and drive them.
//!
//! [`run_worker`] is the worker's entry point. It constructs the pure
//! [`WorkerMachine`](crate::worker_machine::WorkerMachine), the imperative
//! [`WorkerShell`](crate::worker_shell::WorkerShell), and a completion queue,
//! then hands them to [`temper_worker_io::drive`]. It must run inside an engine
//! task (the drive loop reads the runtime clock and the shell spawns I/O). The
//! HTTP entry point takes the [`RuntimeHandle`] from
//! [`temper_worker_io::block_on_with`]; the transport-generic entry point takes
//! any [`temper_worker_io::Spawner`], including the lab spawner used by
//! `temper-sim`.

use std::future::Future;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use skein::runtime::RuntimeHandle;
use temper_worker_io::{CqSender, OneshotReceiver, Spawner, channel, drive, oneshot};

use crate::client::WorkerError;
use crate::config::{WorkerConfig, WorkerParams};
use crate::executor::JobExecutor;
use crate::lifecycle_hook::{NoopWorkerLifecycleHook, WorkerLifecycleHook};
use crate::result_outbox::ResultOutbox;
use crate::task_registry::{
    WorkerComponentTasks, WorkerShutdown, WorkerTaskJoinNotification, WorkerTaskRegistry,
};
use crate::trace::spawn_activity_forwarder;
use crate::transport::{HttpTransport, Transport};
use crate::worker_machine::{WorkerCompletion, WorkerMachine};
use crate::worker_shell::{WorkerCancellation, WorkerShell};

/// Run the worker to (effective) completion over the **HTTP** transport (the
/// split deployment): register, then poll/dispatch/report/heartbeat forever,
/// driven by the completion queue. Returns only if the machine stops or every
/// completion sender is dropped — in normal operation it runs until the process
/// is signalled.
///
/// `handle` is the runtime's spawn capability, passed explicitly from the
/// `block_on_with` entry (no ambient handle lookup — skein removed
/// `Runtime::current_handle`).
pub async fn run_worker<E>(
    handle: RuntimeHandle,
    config: WorkerConfig,
    executor: Arc<E>,
) -> Result<(), WorkerError>
where
    E: JobExecutor + Send + Sync + 'static,
{
    let transport = Arc::new(HttpTransport::new(&config.daemon_url));
    run_worker_with_transport(handle, config, executor, transport).await
}

/// [`run_worker`] over an arbitrary [`Transport`] — the seam the unified
/// single-process mode and deterministic simulation use to point the worker at
/// a co-resident daemon over an in-memory channel instead of HTTP. The protocol
/// and the whole machine/shell loop are identical; only the carrier differs.
pub async fn run_worker_with_transport<E, T, S>(
    spawner: S,
    config: WorkerConfig,
    executor: Arc<E>,
    transport: Arc<T>,
) -> Result<(), WorkerError>
where
    E: JobExecutor + Send + Sync + 'static,
    T: Transport,
    S: Spawner,
{
    start_worker_with_transport(spawner, config, executor, transport)
        .join()
        .await;
    Ok(())
}

/// Control for a worker component started by [`start_worker_with_transport`].
///
/// `crash` closes the machine loop without publishing or releasing its current
/// jobs, then joins that loop. This is intentionally distinct from graceful
/// workflow cleanup so restart tests can retain durable assignment state.
pub struct WorkerComponentHandle {
    completions: CqSender<WorkerCompletion>,
    joined: Option<OneshotReceiver<()>>,
    forwarder_joined: Option<OneshotReceiver<()>>,
    cancellation: WorkerCancellation,
    task_registry: WorkerTaskRegistry,
    component_tasks: WorkerComponentTasks,
    graceful_cancellation_grace: Duration,
    forced_termination_grace: Duration,
}

impl WorkerComponentHandle {
    /// Gracefully stops intake, fences active attempts, applies configured
    /// escalation deadlines, and joins every worker-owned task.
    pub async fn shutdown(mut self) {
        let notification = self.task_registry.begin_shutdown(WorkerShutdown::Graceful);
        let _ = self.completions.send(WorkerCompletion::BeginShutdown);
        if !wait_until_or_timeout(notification, self.graceful_cancellation_grace).await {
            self.task_registry
                .request_all(crate::JobCancellationRequest::ForcedTermination);
            if !wait_until_or_timeout(
                self.task_registry.join_notification(),
                self.forced_termination_grace,
            )
            .await
            {
                self.task_registry
                    .request_all(crate::JobCancellationRequest::HardKill);
                self.task_registry.join_notification().wait().await;
            }
        }
        self.stop_background_and_machine().await;
    }

    /// Stops the component without publishing or releasing active claims. It
    /// immediately hard-escalates every attempt, but still waits indefinitely
    /// for recursive emptiness and resource joins before returning.
    pub async fn crash(mut self) {
        let notification = self.task_registry.begin_shutdown(WorkerShutdown::Crash);
        let _ = self.completions.send(WorkerCompletion::BeginShutdown);
        notification.wait().await;
        self.stop_background_and_machine().await;
    }

    /// Returns a snapshot of the registry used to prove local attempt absence.
    pub fn task_registry(&self) -> WorkerTaskRegistry {
        self.task_registry.clone()
    }

    /// Waits until the worker exits without requesting shutdown.
    pub async fn join(mut self) {
        if let Some(joined) = self.joined.take() {
            let _ = joined.recv().await;
        }
        self.component_tasks.stop_accepting();
        self.cancellation.cancel();
        self.component_tasks.wait_empty().await;
        if let Some(joined) = self.forwarder_joined.take() {
            let _ = joined.recv().await;
        }
    }

    async fn stop_background_and_machine(&mut self) {
        self.component_tasks.stop_accepting();
        self.cancellation.cancel();
        self.component_tasks.wait_empty().await;
        if let Some(joined) = self.forwarder_joined.take() {
            let _ = joined.recv().await;
        }
        let _ = self.completions.send(WorkerCompletion::Shutdown);
        if let Some(joined) = self.joined.take() {
            let _ = joined.recv().await;
        }
    }
}

async fn wait_until_or_timeout(
    notification: WorkerTaskJoinNotification,
    timeout: Duration,
) -> bool {
    if notification.is_ready() {
        return true;
    }
    let mut joined = std::pin::pin!(notification.wait());
    let mut timer = std::pin::pin!(temper_worker_io::sleep_for(timeout));
    std::future::poll_fn(|cx| {
        if joined.as_mut().poll(cx).is_ready() {
            return Poll::Ready(true);
        }
        if timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(false);
        }
        Poll::Pending
    })
    .await
}

/// Starts the production worker machine and returns explicit crash/join
/// controls. The durable workspace and transport are caller-owned, so a new
/// component may be created against the same state after this handle joins.
pub fn start_worker_with_transport<E, T, S>(
    spawner: S,
    config: WorkerConfig,
    executor: Arc<E>,
    transport: Arc<T>,
) -> WorkerComponentHandle
where
    E: JobExecutor + Send + Sync + 'static,
    T: Transport,
    S: Spawner,
{
    start_worker_with_transport_and_hook(
        spawner,
        config,
        executor,
        transport,
        Arc::new(NoopWorkerLifecycleHook),
    )
}

/// Starts the production worker with an optional lifecycle hook used by
/// deterministic restart acceptance fixtures. Product entry points call
/// [`start_worker_with_transport`] and therefore install the zero-cost no-op.
pub fn start_worker_with_transport_and_hook<E, T, S>(
    spawner: S,
    config: WorkerConfig,
    executor: Arc<E>,
    transport: Arc<T>,
    lifecycle_hook: Arc<dyn WorkerLifecycleHook>,
) -> WorkerComponentHandle
where
    E: JobExecutor + Send + Sync + 'static,
    T: Transport,
    S: Spawner,
{
    let params = WorkerParams::from_config(&config);
    let liveness_limits = params.liveness_limits;
    let outbox = Arc::new(ResultOutbox::new(params.result_root.clone()));
    let recovered = match outbox.load() {
        Ok(entries) => entries,
        Err(error) => {
            tracing::error!(
                target: "temper_worker",
                %error,
                "worker: result outbox startup scan failed"
            );
            Vec::new()
        }
    };
    let (cq_tx, cq_rx) = channel();

    let cancellation = WorkerCancellation::default();
    let task_registry = WorkerTaskRegistry::new();
    let component_tasks = WorkerComponentTasks::default();
    let forwarder_joined = spawn_activity_forwarder(
        spawner.clone(),
        config.agent_traces.clone(),
        Arc::clone(&transport),
        config.worker_id.clone(),
        config.worker_auth.clone(),
        cancellation.clone(),
    );
    let shell = WorkerShell::with_transport_controlled(
        spawner.clone(),
        cq_tx.clone(),
        transport,
        config.worker_auth.clone(),
        config.worker_id.clone(),
        executor,
        outbox,
        cancellation.clone(),
        lifecycle_hook,
        task_registry.clone(),
        component_tasks.clone(),
    );
    let machine = WorkerMachine::with_recovered_outbox(params, recovered);
    let (joined_tx, joined) = oneshot();
    spawner.spawn_task(async move {
        let _ = drive(machine, &shell, cq_rx).await;
        joined_tx.send(());
    });

    WorkerComponentHandle {
        completions: cq_tx,
        joined: Some(joined),
        forwarder_joined,
        cancellation,
        task_registry,
        component_tasks,
        graceful_cancellation_grace: liveness_limits.graceful_cancellation_grace,
        forced_termination_grace: liveness_limits.forced_termination_grace,
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
